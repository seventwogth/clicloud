use crate::Result;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
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

/// How yt-dlp is run: the program, its route to SoundCloud and its own cache.
#[derive(Clone, Copy)]
pub struct Extractor<'a> {
    pub program: &'a str,
    pub proxy: Option<&'a str>,
    pub no_proxy: bool,
    /// Where yt-dlp keeps what it learns, such as the client id; None to keep nothing.
    pub cache: Option<&'a Path>,
}

impl Extractor<'_> {
    /// Whether requests take a detour, set up here or in the environment. Such a
    /// route, Tor above all, is slow to connect and gets more time.
    pub fn proxied(&self) -> bool {
        self.proxy.is_some()
            || (!self.no_proxy
                && ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
                    .iter()
                    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty())))
    }

    /// yt-dlp with the options shared by searching and downloading.
    pub fn command(&self) -> Command {
        // A relative path must keep pointing at the same file if the directory changes.
        let program = Path::new(self.program);
        let mut command = if program.components().count() > 1 {
            Command::new(std::path::absolute(program).unwrap_or_else(|_| program.into()))
        } else {
            Command::new(self.program)
        };
        command.arg("--ignore-config");
        match self.cache {
            Some(directory) => command.arg("--cache-dir").arg(directory),
            None => command.arg("--no-cache-dir"),
        };
        if let Some(proxy) = self.proxy {
            command.args(["--proxy", proxy]);
        } else if self.no_proxy {
            command.args(["--proxy", ""]);
        }
        command
    }
}

pub struct SoundCloud<'a> {
    extractor: Extractor<'a>,
    quiet: bool,
    cancel: Option<&'a AtomicBool>,
}

impl<'a> SoundCloud<'a> {
    pub fn new(extractor: Extractor<'a>) -> Self {
        Self {
            extractor,
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
        let mut command = self.extractor.command();
        command
            .args([
                "--flat-playlist",
                "--dump-single-json",
                "--skip-download",
                "--socket-timeout",
                if self.extractor.proxied() { "45" } else { "15" },
                "--retries",
                "2",
            ])
            .arg("--")
            .arg(format!("scsearch{limit}:{query}"))
            .stdin(Stdio::null());
        let output = capture(&mut command, self.cancel).map_err(|error| {
            format!(
                "Не удалось запустить yt-dlp ({}): {error}. Проверьте clicloud doctor.",
                self.extractor.program
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
