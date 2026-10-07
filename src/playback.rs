//! Managed mpv process and its local JSON IPC connection (Unix).
use crate::{
    Result,
    cache::Cache,
    player::{self, Download},
    soundcloud::{self, Extractor},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

const LOG_LINES: usize = 20;

/// Where the audio of a playback comes from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Origin {
    /// A file stored earlier: no network involved.
    Cache,
    /// yt-dlp, which stores the track while it plays.
    Download,
    /// yt-dlp through a pipe, nothing is kept.
    Pipe,
    /// mpv fetches the stream by itself.
    Url,
}

pub struct Playback {
    child: Option<Child>,
    source: Option<Child>,
    download: Option<Download>,
    pub origin: Origin,
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
            download: None,
            origin: Origin::Url,
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
        extractor: Extractor,
        url: &str,
        cache: Option<&Cache>,
        volume: f64,
    ) -> Result<Self> {
        soundcloud::validate_url(url)?;
        let mut player = Self::idle(player::scratch_directory()?, volume);
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
        if let Some(file) = cache.and_then(|cache| cache.find(url)) {
            player.origin = Origin::Cache;
            player::from_file(&mut command, &file);
            command.stdin(Stdio::null());
        } else if let Some(partial) = cache.map(|cache| cache.store(url)).transpose()?.flatten() {
            player.origin = Origin::Download;
            let mut source = player::downloader(extractor, url, partial.directory(), false, true);
            source.stderr(log);
            player.download = Some(Download::start(source, partial)?);
            player::from_pipe(&mut command, Stdio::piped());
        } else if extractor.proxy.is_some() {
            player.origin = Origin::Pipe;
            let mut source = player::downloader(extractor, url, &player.directory, false, false)
                .stderr(log)
                .spawn()
                .map_err(|e| format!("Не удалось запустить yt-dlp: {e}"))?;
            player::from_pipe(&mut command, source.stdout.take().expect("piped stdout"));
            player.source = Some(source);
        } else {
            player::from_url(&mut command, extractor, url);
            command.stdin(Stdio::null());
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("mpv: {e}. Установите mpv и проверьте clicloud doctor."))?;
        if let Some(download) = &mut player.download {
            download.feed(child.stdin.take().expect("piped stdin"));
        }
        player.child = Some(child);
        Ok(player)
    }

    /// Audio is still arriving: mpv then only knows the length of what it has got.
    pub fn receiving(&self) -> bool {
        match &self.download {
            Some(download) => !download.finished(),
            None => self.source.is_some(),
        }
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
        let downloaded = match (&mut self.source, &mut self.download) {
            (Some(source), _) => source.try_wait()?,
            (_, Some(download)) => download.poll()?,
            _ => None,
        };
        if downloaded.is_some_and(|status| !status.success()) {
            return Err(
                "yt-dlp не смог загрузить аудио. Проверьте сеть, прокси и доступность трека."
                    .into(),
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
                Some("time-pos") => {
                    if let Some(position) = event["data"].as_f64() {
                        self.position = position;
                        // A stored file is loaded before the connection is there to
                        // receive file-loaded; a position says the same.
                        self.loaded = true;
                    }
                }
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
        let mut lines = tail(&self.directory.join("error.log"));
        lines.extend(self.log.iter().cloned());
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

/// The last lines that yt-dlp or mpv wrote to the log file at `path`.
pub fn tail(path: &Path) -> Vec<String> {
    let bytes = fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(8192)..]);
    let lines: Vec<_> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(LOG_LINES)..]
        .iter()
        .map(|line| line.trim_end().to_owned())
        .collect()
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
        player.event(json!({"event": "property-change", "name": "time-pos", "data": null}));
        assert!(!player.loaded);
        player.event(json!({"event": "property-change", "name": "time-pos", "data": 1.5}));
        assert!(player.loaded && player.position == 1.5);
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
