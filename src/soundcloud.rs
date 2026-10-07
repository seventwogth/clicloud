use crate::Result;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub duration: Option<f64>,
    pub url: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    entries: Vec<Option<Entry>>,
}

#[derive(Deserialize)]
struct Entry {
    title: Option<String>,
    uploader: Option<String>,
    artist: Option<String>,
    duration: Option<f64>,
    webpage_url: Option<String>,
    url: Option<String>,
}

pub struct SoundCloud<'a> {
    executable: &'a str,
    proxy: Option<&'a str>,
    no_proxy: bool,
    quiet: bool,
    cancel: Option<&'a AtomicBool>,
}

impl<'a> SoundCloud<'a> {
    pub fn new(executable: &'a str, proxy: Option<&'a str>, no_proxy: bool) -> Self {
        Self {
            executable,
            proxy,
            no_proxy,
            quiet: false,
            cancel: None,
        }
    }

    pub fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }

    pub fn cancellable(mut self, cancel: &'a AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn search(&self, query: &str, limit: u8) -> Result<Vec<Track>> {
        let query = query.trim();
        if query.is_empty() {
            return Err("Поисковый запрос не должен быть пустым.".into());
        }
        if !self.quiet {
            eprintln!("Поиск в SoundCloud…");
        }
        let mut command = Command::new(self.executable);
        command.args([
            "--ignore-config",
            "--no-cache-dir",
            "--flat-playlist",
            "--dump-single-json",
            "--skip-download",
            "--socket-timeout",
            if self.proxy.is_some() { "45" } else { "15" },
            "--retries",
            "2",
        ]);
        if let Some(proxy) = self.proxy {
            command.args(["--proxy", proxy]);
        } else if self.no_proxy {
            command.args(["--proxy", ""]);
        }
        command
            .arg("--")
            .arg(format!("scsearch{limit}:{query}"))
            .stdin(Stdio::null());
        let output = capture(&mut command, self.cancel).map_err(|error| {
            format!(
                "Не удалось запустить yt-dlp ({}): {error}. Проверьте clicloud doctor.",
                self.executable
            )
        })?;
        if !output.status.success() {
            return Err(format!(
                "Поиск yt-dlp завершился с {}:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        parse_tracks(&output.stdout)
    }
}

fn capture(
    command: &mut Command,
    cancel: Option<&AtomicBool>,
) -> std::io::Result<std::process::Output> {
    let Some(cancel) = cancel else {
        return command.output();
    };
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    };
    let stdout = out
        .join()
        .map_err(|_| std::io::Error::other("stdout reader failed"))?;
    let stderr = err
        .join()
        .map_err(|_| std::io::Error::other("stderr reader failed"))?;
    Ok(std::process::Output {
        status: status?,
        stdout: stdout?,
        stderr: stderr?,
    })
}

fn parse_tracks(bytes: &[u8]) -> Result<Vec<Track>> {
    let response: SearchResponse = serde_json::from_slice(bytes)
        .map_err(|error| format!("Некорректный ответ JSON от yt-dlp: {error}"))?;
    Ok(response
        .entries
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let url = [entry.webpage_url, entry.url]
                .into_iter()
                .flatten()
                .find(|url| validate_url(url).is_ok())?;
            Some(Track {
                title: entry.title.unwrap_or_else(|| "Без названия".into()),
                artist: entry
                    .artist
                    .or(entry.uploader)
                    .unwrap_or_else(|| "Неизвестный исполнитель".into()),
                duration: entry.duration,
                url,
            })
        })
        .collect())
}

pub fn validate_url(value: &str) -> Result<()> {
    let url = Url::parse(value)?;
    let host = url.host_str().unwrap_or_default();
    if !matches!(url.scheme(), "https" | "http")
        || !(host == "soundcloud.com" || host.ends_with(".soundcloud.com"))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Ожидается HTTP(S)-ссылка SoundCloud.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_flat_results_missing_metadata_and_unavailable_entries() {
        let tracks = parse_tracks(br#"{"entries":[null,{"title":"Track","uploader":"Artist","duration":90.5,"webpage_url":"https://soundcloud.com/a/b","url":"https://api.soundcloud.com/tracks/123"},{"url":"https://api.soundcloud.com/tracks/456"},{"url":"file:///tmp/audio"}]}"#).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].url, "https://soundcloud.com/a/b");
        assert_eq!(tracks[0].artist, "Artist");
        assert_eq!(tracks[1].title, "Без названия");
        assert!(tracks[1].duration.is_none());
    }

    #[test]
    fn rejects_non_soundcloud_urls() {
        for url in [
            "file:///etc/passwd",
            "https://soundcloud.com.evil.test/a",
            "https://soundcloud.com@evil.test/a",
            "--script=test",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
        assert!(validate_url("https://on.soundcloud.com/abc").is_ok());
    }

    #[test]
    fn distinguishes_empty_results_from_invalid_response() {
        assert!(parse_tracks(br#"{"entries":[]}"#).unwrap().is_empty());
        assert!(parse_tracks(b"not json").is_err());
        assert!(parse_tracks(b"{}").is_err());
    }
}
