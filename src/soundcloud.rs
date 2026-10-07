use crate::Result;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read};
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
        let mut tracks = Vec::new();
        self.stream(query, limit, |track| tracks.push(track))?;
        Ok(tracks)
    }

    /// Searches, handing over each track as yt-dlp finds it, so that a list can fill
    /// while the rest of the results are still on their way.
    pub fn stream(&self, query: &str, limit: u8, mut found: impl FnMut(Track)) -> Result<()> {
        let query = query.trim();
        if query.is_empty() {
            return Err(t!("Поисковый запрос не должен быть пустым.").into());
        }
        if !self.quiet {
            eprintln!("{}", t!("Поиск в SoundCloud…"));
        }
        let mut command = self.extractor.command();
        command
            .args([
                "--flat-playlist",
                // One track per line, written as it is found instead of at the end.
                "--dump-json",
                "--lazy-playlist",
                "--skip-download",
                "--socket-timeout",
                if self.extractor.proxied() { "45" } else { "15" },
                "--retries",
                "2",
            ])
            .arg("--")
            .arg(format!("scsearch{limit}:{query}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            t!(
                "Не удалось запустить yt-dlp ({}): {}. Проверьте clicloud doctor.",
                self.extractor.program,
                error
            )
        })?;
        // Both pipes are drained while the search runs: a full one would stop yt-dlp.
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).split(b'\n') {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let errors = std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = stderr.read_to_end(&mut text);
            text
        });
        let (mut count, mut stray) = (0, None);
        let status = loop {
            while let Ok(line) = lines.try_recv() {
                take(&line, &mut count, &mut stray, &mut found);
            }
            if self
                .cancel
                .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
            {
                let _ = child.kill();
                break child.wait()?;
            }
            match child.try_wait()? {
                Some(status) => break status,
                None => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        };
        // What was written before the end is still on its way through the channel.
        for line in lines {
            take(&line, &mut count, &mut stray, &mut found);
        }
        let errors = errors
            .join()
            .map_err(|_| std::io::Error::other("stderr reader failed"))?;
        if !status.success() {
            return Err(t!(
                "Поиск yt-dlp завершился с {}:\n{}",
                status,
                String::from_utf8_lossy(&errors).trim()
            )
            .into());
        }
        verdict(count, stray)
    }
}

// Hands over the tracks of one line and remembers the first line that said nothing.
fn take(line: &[u8], count: &mut usize, stray: &mut Option<String>, found: &mut impl FnMut(Track)) {
    match parse_line(line) {
        Some(tracks) => {
            *count += tracks.len();
            for track in tracks {
                found(track);
            }
        }
        None if line.iter().all(u8::is_ascii_whitespace) => (),
        None => {
            if stray.is_none() {
                *stray = Some(String::from_utf8_lossy(line).chars().take(200).collect());
            }
        }
    }
}

// Output that told of no track and held nothing we could read is no answer at all.
fn verdict(count: usize, stray: Option<String>) -> Result<()> {
    match stray {
        Some(line) if count == 0 => Err(t!("Некорректный ответ JSON от yt-dlp: {}", line).into()),
        _ => Ok(()),
    }
}

/// The tracks that one line of yt-dlp's output tells of; None when it tells of none.
///
/// A line is one track. Asked for everything at once, as `--dump-single-json` does,
/// yt-dlp puts them all in one object instead, which is read here too.
fn parse_line(line: &[u8]) -> Option<Vec<Track>> {
    if let Ok(response) = serde_json::from_slice::<SearchResponse>(line) {
        return Some(
            response
                .entries
                .into_iter()
                .flatten()
                .filter_map(track)
                .collect(),
        );
    }
    let entry: Entry = serde_json::from_slice(line).ok()?;
    // Every field of an entry may be missing, and one without any of these is no entry.
    if entry.url.is_none() && entry.webpage_url.is_none() && entry.title.is_none() {
        return None;
    }
    Some(track(entry).into_iter().collect())
}

fn track(entry: Entry) -> Option<Track> {
    let url = [entry.webpage_url, entry.url]
        .into_iter()
        .flatten()
        .find(|url| validate_url(url).is_ok())?;
    Some(Track {
        title: entry.title.unwrap_or_else(|| t!("Без названия").into()),
        artist: entry
            .artist
            .or(entry.uploader)
            .unwrap_or_else(|| t!("Неизвестный исполнитель").into()),
        duration: entry.duration,
        url,
    })
}

pub fn validate_url(value: &str) -> Result<()> {
    let url = Url::parse(value)?;
    let host = url.host_str().unwrap_or_default();
    if !matches!(url.scheme(), "https" | "http")
        || !(host == "soundcloud.com" || host.ends_with(".soundcloud.com"))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(t!("Ожидается HTTP(S)-ссылка SoundCloud.").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_flat_results_missing_metadata_and_unavailable_entries() {
        let tracks = parse_line(br#"{"entries":[null,{"title":"Track","uploader":"Artist","duration":90.5,"webpage_url":"https://soundcloud.com/a/b","url":"https://api.soundcloud.com/tracks/123"},{"url":"https://api.soundcloud.com/tracks/456"},{"url":"file:///tmp/audio"}]}"#).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].url, "https://soundcloud.com/a/b");
        assert_eq!(tracks[0].artist, "Artist");
        assert_eq!(tracks[1].title, "Без названия");
        assert!(tracks[1].duration.is_none());
    }

    #[test]
    fn reads_one_track_of_a_line_as_it_arrives() {
        let line = br#"{"title":"Night","uploader":"Artist","duration":90.5,"url":"https://soundcloud.com/a/b"}"#;
        let tracks = parse_line(line).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(
            (tracks[0].title.as_str(), tracks[0].artist.as_str()),
            ("Night", "Artist")
        );
        // Windows ends a line with a return as well, which says nothing of the track.
        let mut ended = line.to_vec();
        ended.push(b'\r');
        assert_eq!(parse_line(&ended).unwrap().len(), 1);
        // A line that tells of a link outside SoundCloud is read and left out.
        assert!(
            parse_line(br#"{"url":"file:///tmp/audio"}"#)
                .unwrap()
                .is_empty()
        );

        let (mut count, mut stray) = (0, None);
        for blank in [&b""[..], b"\r", b"  "] {
            take(blank, &mut count, &mut stray, &mut |_| ());
        }
        assert!(count == 0 && stray.is_none(), "{stray:?}");
        take(line, &mut count, &mut stray, &mut |_| ());
        take(b"not json", &mut count, &mut stray, &mut |_| ());
        assert_eq!((count, stray.as_deref()), (1, Some("not json")));
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
        assert!(parse_line(br#"{"entries":[]}"#).unwrap().is_empty());
        assert!(parse_line(b"not json").is_none());
        assert!(parse_line(b"{}").is_none());
        // Output that told of no track and held nothing we could read is an error;
        // a line we could not read beside tracks that arrived is not.
        assert!(verdict(0, None).is_ok());
        assert!(verdict(0, Some("not json".into())).is_err());
        assert!(verdict(2, Some("not json".into())).is_ok());
    }
}
