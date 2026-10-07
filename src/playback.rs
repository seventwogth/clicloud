//! Managed mpv process and its local JSON IPC connection (Unix).
use crate::{Result, player, soundcloud};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::DirBuilderExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LOG_LINES: usize = 20;

pub struct Playback {
    child: Option<Child>,
    source: Option<Child>,
    socket: Option<UnixStream>,
    directory: PathBuf,
    buffer: Vec<u8>,
    started: Instant,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub volume: f64,
    pub loaded: bool,
    eof: bool,
    error: Option<String>,
    log: Vec<String>,
}

impl Playback {
    fn idle(directory: PathBuf, volume: f64) -> Self {
        Self {
            child: None,
            source: None,
            socket: None,
            directory,
            buffer: Vec::new(),
            started: Instant::now(),
            position: 0.0,
            duration: 0.0,
            paused: false,
            volume,
            loaded: false,
            eof: false,
            error: None,
            log: Vec::new(),
        }
    }

    pub fn start(
        mpv: &str,
        yt_dlp: &str,
        url: &str,
        proxy: Option<&str>,
        direct: bool,
        volume: f64,
    ) -> Result<Self> {
        soundcloud::validate_url(url)?;
        let directory = std::env::temp_dir().join(format!(
            "clicloud-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let mut player = Self::idle(directory, volume);
        let log = fs::File::create(player.directory.join("error.log"))?;
        let mut command = player::mpv(mpv);
        command
            .args([
                "--no-terminal",
                "--input-terminal=no",
                "--idle=no",
                "--volume-max=100",
            ])
            .arg(format!(
                "--input-ipc-server={}",
                player.directory.join("ipc").display()
            ))
            .arg(format!("--volume={volume}"))
            .stdout(Stdio::null())
            .stderr(log.try_clone()?);
        if let Some(proxy) = proxy {
            let mut source = player::downloader(yt_dlp, url, proxy, false)
                .stderr(log)
                .spawn()
                .map_err(|e| format!("yt-dlp: {e}"))?;
            player::from_pipe(&mut command, source.stdout.take().expect("piped stdout"));
            player.source = Some(source);
        } else {
            player::from_url(&mut command, yt_dlp, url, direct);
            command.stdin(Stdio::null());
        }
        player.child = Some(
            command
                .spawn()
                .map_err(|e| format!("mpv: {e}. Установите mpv и проверьте clicloud doctor."))?,
        );
        Ok(player)
    }

    pub fn send(&mut self, command: Value) -> Result<()> {
        let socket = self.socket.as_mut().ok_or("Плеер ещё подключается")?;
        let mut bytes = serde_json::to_vec(&json!({"command": command}))?;
        bytes.push(b'\n');
        socket.write_all(&bytes)?;
        Ok(())
    }

    /// Returns true only on successful end-of-file, for queue advancement.
    pub fn tick(&mut self) -> Result<bool> {
        // Checked before reading, so the last events of an exited mpv are not missed.
        let exited = match &mut self.child {
            Some(child) => child.try_wait()?,
            None => None,
        };
        if self.socket.is_none() {
            if let Ok(socket) = UnixStream::connect(self.directory.join("ipc")) {
                socket.set_nonblocking(true)?;
                self.socket = Some(socket);
                for (id, name) in ["time-pos", "duration", "pause", "volume"]
                    .iter()
                    .enumerate()
                {
                    self.send(json!(["observe_property", id + 1, name]))?;
                }
                self.send(json!(["request_log_messages", "error"]))?;
            } else if self.started.elapsed() > Duration::from_secs(5) {
                return Err("mpv не открыл IPC-соединение за 5 секунд".into());
            }
        }
        let mut bytes = [0; 8192];
        if let Some(socket) = &mut self.socket {
            loop {
                match socket.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => self.buffer.extend_from_slice(&bytes[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                }
            }
        }
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.buffer.drain(..=end).collect();
            if let Ok(event) = serde_json::from_slice::<Value>(&line) {
                self.event(event);
            }
        }
        if let Some(error) = self.error.take() {
            return Err(error.into());
        }
        if let Some(source) = &mut self.source
            && let Some(status) = source.try_wait()?
            && !status.success()
        {
            return Err(
                "yt-dlp не смог загрузить аудио. Проверьте прокси и доступность трека.".into(),
            );
        }
        if let Some(status) = exited {
            if !status.success() {
                return Err("Ошибка mpv: поток недоступен или отсутствует аудиоустройство.".into());
            }
            if !self.eof {
                return Err("Плеер завершился до окончания трека.".into());
            }
            return Ok(true);
        }
        Ok(self.eof)
    }

    fn event(&mut self, event: Value) {
        match event["event"].as_str() {
            Some("file-loaded") => self.loaded = true,
            Some("end-file") => {
                self.eof = event["reason"] == "eof";
                if event["reason"] == "error" {
                    self.error = Some(
                        "Не удалось проиграть трек. Проверьте сеть, прокси или выберите другой."
                            .into(),
                    );
                }
            }
            Some("log-message") => {
                let text = event["text"].as_str().unwrap_or_default().trim();
                if !text.is_empty() {
                    if self.log.len() == LOG_LINES {
                        self.log.remove(0);
                    }
                    let prefix = event["prefix"].as_str().unwrap_or("mpv");
                    self.log.push(format!("{prefix}: {text}"));
                }
            }
            Some("property-change") => match event["name"].as_str() {
                Some("time-pos") => self.position = event["data"].as_f64().unwrap_or(self.position),
                Some("duration") => self.duration = event["data"].as_f64().unwrap_or(self.duration),
                Some("pause") => self.paused = event["data"].as_bool().unwrap_or(false),
                Some("volume") => self.volume = event["data"].as_f64().unwrap_or(self.volume),
                _ => (),
            },
            _ => (),
        }
    }
}

impl Playback {
    /// Downloader output and mpv error messages behind a failed playback.
    pub fn details(&self) -> Option<String> {
        let mut lines = Vec::new();
        if let Ok(bytes) = fs::read(self.directory.join("error.log")) {
            let text = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(8192)..]);
            let tail: Vec<_> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            lines.extend(
                tail[tail.len().saturating_sub(LOG_LINES)..]
                    .iter()
                    .map(|line| line.trim_end().to_owned()),
            );
        }
        lines.extend(self.log.iter().cloned());
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        for child in [&mut self.child, &mut self.source].into_iter().flatten() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_details_combine_downloader_output_and_player_log() {
        let directory =
            std::env::temp_dir().join(format!("clicloud-playback-test-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let mut player = Playback::idle(directory.clone(), 70.0);
        assert!(player.details().is_none());
        fs::write(directory.join("error.log"), "ERROR: [soundcloud] 403\n\n").unwrap();
        player.event(json!({"event": "log-message", "prefix": "ytdl_hook", "level": "error", "text": "youtube-dl failed\n"}));
        player.event(json!({"event": "end-file", "reason": "error"}));
        assert!(player.tick().is_err());
        assert_eq!(
            player.details().unwrap(),
            "ERROR: [soundcloud] 403\nytdl_hook: youtube-dl failed"
        );
        for line in 0..LOG_LINES * 2 {
            player.event(json!({"event": "log-message", "text": format!("line {line}")}));
        }
        assert_eq!(player.log.len(), LOG_LINES);
        assert_eq!(player.log[0], format!("mpv: line {LOG_LINES}"));
        drop(player);
        assert!(!directory.exists());
    }
}
