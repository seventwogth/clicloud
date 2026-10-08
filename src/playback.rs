//! Managed mpv process and its local JSON IPC connection (Unix).
use crate::{
    Result,
    cache::Cache,
    player::{self, Download},
    soundcloud::{self, Details, Extractor, Track},
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
    /// How far the audio that has arrived reaches, in seconds; 0 until mpv tells.
    pub buffered: f64,
    pub speed: f64,
    pub paused: bool,
    pub volume: f64,
    pub loaded: bool,
    eof: bool,
    error: Option<String>,
    log: Vec<String>,
    // Where the yt-dlp that feeds the player notes what it learns of the track.
    noted: Option<PathBuf>,
    // The stored file that mpv was told to play after the current one, and whether it
    // has gone on to it: the same player then plays the next track without a gap.
    appended: Option<PathBuf>,
    advanced: bool,
    // The file mpv is to name as the one it plays now, and whether it named another.
    expected: Option<PathBuf>,
    astray: bool,
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
            buffered: 0.0,
            speed: 1.0,
            paused: false,
            volume,
            loaded: false,
            eof: false,
            error: None,
            log: Vec::new(),
            noted: None,
            appended: None,
            advanced: false,
            expected: None,
            astray: false,
        }
    }

    pub fn start(
        mpv: &str,
        extractor: Extractor,
        track: &Track,
        cache: Option<&Cache>,
        sound: player::Sound,
        volume: f64,
        speed: f64,
    ) -> Result<Self> {
        let url = track.url.as_str();
        soundcloud::validate_url(url)?;
        let mut player = Self::idle(player::scratch_directory()?, volume);
        let log = fs::File::create(player.directory.join("error.log"))?;
        player.speed = speed;
        let mut command = player::mpv(mpv, sound);
        command
            .args([
                "--no-terminal",
                "--input-terminal=no",
                "--idle=no",
                "--volume-max=100",
                // The file that follows is opened before the current one ends.
                "--prefetch-playlist=yes",
            ])
            .arg(format!(
                "--input-ipc-server={}",
                player.directory.join("ipc").display()
            ))
            .arg(format!("--volume={volume}"))
            .arg(format!("--speed={speed}"))
            .stdout(Stdio::null())
            .stderr(log.try_clone()?);
        if let Some(file) = cache.and_then(|cache| cache.find(url)) {
            // Stored by its link alone, the track gets its name now.
            cache.inspect(|cache| cache.describe(track));
            player.origin = Origin::Cache;
            player::from_file(&mut command, &file);
            command.stdin(Stdio::null());
        } else if let Some(mut partial) = cache.map(|cache| cache.store(url)).transpose()?.flatten()
        {
            partial.describe(track);
            player.noted = Some(partial.directory().to_owned());
            player.origin = Origin::Download;
            let mut source = player::downloader(extractor, url, partial.directory(), false, true);
            source.stderr(log);
            player.download = Some(Download::start(source, partial)?);
            player::from_pipe(&mut command, Stdio::piped());
        } else if extractor.proxy.is_some() {
            player.origin = Origin::Pipe;
            player.noted = Some(player.directory.clone());
            let mut source = player::downloader(extractor, url, &player.directory, false, false)
                .stderr(log)
                .spawn()
                .map_err(|e| t!("Не удалось запустить yt-dlp: {}", e))?;
            player::from_pipe(&mut command, source.stdout.take().expect("piped stdout"));
            player.source = Some(source);
        } else {
            player::from_url(&mut command, extractor, url);
            command.stdin(Stdio::null());
        }
        let mut child = command
            .spawn()
            .map_err(|e| t!("mpv: {}. Установите mpv и проверьте clicloud doctor.", e))?;
        if let Some(download) = &mut player.download {
            download.feed(child.stdin.take().expect("piped stdin"));
        }
        player.child = Some(child);
        Ok(player)
    }

    /// Tells mpv which stored file plays after the current one, or that none does. It
    /// goes on to that file by itself, as the next track of an album should start.
    pub fn follow(&mut self, next: Option<&Path>) {
        if self.appended.as_deref() == next || self.socket.is_none() || self.eof {
            return;
        }
        // Clearing leaves the file that plays and takes what was to follow it.
        let cleared =
            self.appended.take().is_none() || self.send(json!(["playlist-clear"])).is_ok();
        if let (true, Some(next)) = (cleared, next)
            && (self.send(json!(["loadfile", next, "append"]))).is_ok()
        {
            self.appended = Some(next.to_owned());
        }
    }

    /// Whether mpv has gone on to the file that was to follow, told once.
    pub fn advanced(&mut self) -> bool {
        std::mem::take(&mut self.advanced)
    }

    /// Whether mpv plays another file than the one it was last told to go on to: what
    /// followed was changed in the instant it went on. Told once.
    pub fn astray(&mut self) -> bool {
        std::mem::take(&mut self.astray)
    }

    /// What yt-dlp has noted of the track it fetches for this player, told once.
    pub fn learn(&mut self) -> Option<Details> {
        let details = Details::read(self.noted.as_ref()?)?;
        self.noted = None;
        Some(details)
    }

    /// Audio is still arriving: mpv then only knows the length of what it has got.
    pub fn receiving(&self) -> bool {
        match &self.download {
            Some(download) => !download.finished(),
            None => self.source.is_some(),
        }
    }

    pub fn send(&mut self, command: Value) -> Result<()> {
        let socket = self.socket.as_mut().ok_or(t!("Плеер ещё подключается"))?;
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
                let observed = [
                    "time-pos",
                    "duration",
                    "pause",
                    "volume",
                    "speed",
                    "demuxer-cache-time",
                    "path",
                ];
                for (id, name) in observed.iter().enumerate() {
                    self.send(json!(["observe_property", id + 1, name]))?;
                }
                self.send(json!(["request_log_messages", "error"]))?;
            } else if self.started.elapsed() > Duration::from_secs(5) {
                return Err(t!("mpv не открыл IPC-соединение за 5 секунд").into());
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
            return Err(t!(
                "yt-dlp не смог загрузить аудио. Проверьте сеть, прокси и доступность трека."
            )
            .into());
        }
        if let Some(status) = exited {
            if !status.success() {
                return Err(
                    t!("Ошибка mpv: поток недоступен или отсутствует аудиоустройство.").into(),
                );
            }
            if !self.eof {
                return Err(t!("Плеер завершился до окончания трека.").into());
            }
            return Ok(true);
        }
        Ok(self.eof)
    }

    fn event(&mut self, event: Value) {
        match event["event"].as_str() {
            Some("file-loaded") => self.loaded = true,
            // The end of a file that another one follows is not the end of the player.
            Some("end-file") if event["reason"] == "eof" && self.appended.is_some() => {
                self.expected = self.appended.take();
                self.advanced = true;
                self.origin = Origin::Cache;
                self.noted = None;
                self.loaded = false;
                (self.position, self.duration, self.buffered) = (0.0, 0.0, 0.0);
            }
            Some("end-file") => {
                self.eof = event["reason"] == "eof";
                if event["reason"] == "error" {
                    self.error = Some(
                        t!(
                            "Не удалось проиграть трек. Проверьте сеть, прокси или выберите другой."
                        )
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
                Some("speed") => self.speed = event["data"].as_f64().unwrap_or(self.speed),
                Some("path") => {
                    if let (Some(path), Some(expected)) = (event["data"].as_str(), &self.expected) {
                        self.astray = Path::new(path) != expected;
                        self.expected = None;
                    }
                }
                Some("demuxer-cache-time") => {
                    self.buffered = event["data"].as_f64().unwrap_or(self.buffered)
                }
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
    fn the_end_of_a_file_that_another_follows_is_not_the_end() {
        let directory =
            std::env::temp_dir().join(format!("clicloud-follow-test-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let mut player = Playback::idle(directory.clone(), 70.0);
        let next = directory.join("next");
        let ended = json!({"event": "end-file", "reason": "eof"});
        // Not connected yet, mpv cannot be told what follows.
        player.follow(Some(&next));
        assert!(player.appended.is_none());

        player.appended = Some(next.clone());
        player.origin = Origin::Download;
        player.event(json!({"event": "property-change", "name": "time-pos", "data": 90.0}));
        player
            .event(json!({"event": "property-change", "name": "demuxer-cache-time", "data": 95.0}));
        player.event(json!({"event": "property-change", "name": "speed", "data": 1.5}));
        assert_eq!((player.buffered, player.speed), (95.0, 1.5));
        player.event(ended.clone());
        assert!(player.advanced() && !player.advanced());
        assert!(!player.eof && !player.loaded && player.origin == Origin::Cache);
        assert_eq!((player.position, player.buffered), (0.0, 0.0));
        // mpv names the file it went on to: the expected one, and nothing is amiss.
        player.event(json!({"event": "property-change", "name": "path", "data": next.to_str()}));
        assert!(!player.astray());
        // Nothing follows that one, so its end is the end.
        player.event(ended.clone());
        assert!(player.eof && !player.advanced());

        // Another file than the one it was last told of: what followed was changed in
        // the instant it went on, and it plays what was to follow before.
        player.eof = false;
        player.appended = Some(next.clone());
        player.event(ended);
        player.event(json!({"event": "property-change", "name": "path", "data": "/other"}));
        assert!(player.advanced() && player.astray() && !player.astray());
        // A failure is still a failure, whatever follows.
        player.appended = Some(next);
        player.event(json!({"event": "end-file", "reason": "error"}));
        assert!(!player.advanced() && player.error.is_some());
        drop(player);
        assert!(!directory.exists());
    }

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
