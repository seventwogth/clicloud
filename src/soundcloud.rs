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

/// What yt-dlp tells of a track when it looks it up, which is more than the list of a
/// profile does: there a track has a title and a link, and its maker is a guess.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Details {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<f64>,
    /// Where the picture of the track is.
    pub artwork: Option<String>,
    pub genre: Option<String>,
    pub tags: Option<Vec<String>>,
    pub description: Option<String>,
    pub plays: Option<u64>,
    pub likes: Option<u64>,
    pub reposts: Option<u64>,
    pub comments: Option<u64>,
    /// The day it was published, as eight digits: year, month, day.
    pub date: Option<String>,
    pub license: Option<String>,
}

impl Details {
    /// The file that a download notes them in, in the directory it runs in.
    pub const FILE: &'static str = "details.json";
    // What yt-dlp is to write of a track: a line of JSON, with null for what it lacks.
    const TEMPLATE: &'static str = concat!(
        r#"{"title":%(title)j,"artist":%(uploader)j,"duration":%(duration)j,"#,
        r#""artwork":%(thumbnail)j,"genre":%(genres.0)j,"tags":%(tags)j,"#,
        r#""description":%(description)j,"plays":%(view_count)j,"likes":%(like_count)j,"#,
        r#""reposts":%(repost_count)j,"comments":%(comment_count)j,"#,
        r#""date":%(upload_date)j,"license":%(license)j}"#
    );
    /// What makes a download write that file.
    pub const ARGUMENTS: [&'static str; 5] = [
        "--output-na-placeholder",
        "null",
        "--print-to-file",
        Self::TEMPLATE,
        Self::FILE,
    ];

    /// The details in a line that yt-dlp wrote; None for one that tells nothing.
    pub fn parse(line: &[u8]) -> Option<Self> {
        let details: Self = serde_json::from_slice(line).ok()?;
        (details.title.is_some() || details.artist.is_some() || details.duration.is_some())
            .then_some(details)
    }

    /// The details noted in `directory`; None until yt-dlp has looked the track up.
    pub fn read(directory: &Path) -> Option<Self> {
        let text = std::fs::read(directory.join(Self::FILE)).ok()?;
        Self::parse(text.split(|byte| *byte == b'\n').next()?)
    }

    /// Puts what is known in place of what `track` had.
    pub fn apply(&self, track: &mut Track) {
        let named = |name: &Option<String>| name.clone().filter(|name| !name.trim().is_empty());
        if let Some(title) = named(&self.title) {
            track.title = title;
        }
        if let Some(artist) = named(&self.artist) {
            track.artist = artist;
        }
        if let Some(duration) = self.duration.filter(|d| d.is_finite() && *d > 0.0) {
            track.duration = Some(duration);
        }
    }
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

    /// Answers what was typed: a link to SoundCloud is opened, anything else is
    /// searched for. `limit` is the most that is taken of either.
    pub fn ask(&self, question: &str, limit: u16, found: impl FnMut(Track)) -> Result<()> {
        match page(question) {
            Some(_) => self.open(question, Some(limit), found),
            None => self.stream(question, limit.clamp(1, 50) as u8, found),
        }
    }

    /// Looks one track up: all that SoundCloud tells of it, but where its audio is.
    pub fn details(&self, url: &str) -> Result<Details> {
        validate_url(url)?;
        let mut command = self.extractor.command();
        let output = command
            .args([
                "--ignore-no-formats-error",
                "--extractor-args",
                "soundcloud:formats=none",
                "--skip-download",
                "--no-playlist",
                "--socket-timeout",
                if self.extractor.proxied() { "30" } else { "15" },
                "--retries",
                "2",
                "--output-na-placeholder",
                "null",
                "--print",
                Details::TEMPLATE,
                "--",
                url,
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|error| {
                t!(
                    "Не удалось запустить yt-dlp ({}): {}. Проверьте clicloud doctor.",
                    self.extractor.program,
                    error
                )
            })?;
        (output.stdout.split(|byte| *byte == b'\n'))
            .rev()
            .find_map(Details::parse)
            .ok_or_else(|| t!("Некорректный ответ JSON от yt-dlp: {}", "").into())
    }

    /// The tracks behind a link: those of a playlist, of a page of a profile, of the
    /// station of a track, or the one track that the link names. A page that lists
    /// playlists hands them over as tracks whose links lead to them.
    pub fn open(&self, link: &str, limit: Option<u16>, found: impl FnMut(Track)) -> Result<()> {
        let (page, link) = page(link).ok_or(t!("Ожидается HTTP(S)-ссылка SoundCloud."))?;
        if !self.quiet {
            eprintln!("{}", t!("Читаю список {}…", link));
        }
        // A page tells the title and the link of each track at once. A playlist tells
        // links alone, and not even those of all its tracks, so each is looked up.
        self.list(&link, page == Page::Listing, limit, found)
    }

    /// Searches, handing over each track as yt-dlp finds it, so that a list can fill
    /// while the rest of the results are still on their way.
    pub fn stream(&self, query: &str, limit: u8, found: impl FnMut(Track)) -> Result<()> {
        let query = query.trim();
        if query.is_empty() {
            return Err(t!("Поисковый запрос не должен быть пустым.").into());
        }
        if !self.quiet {
            eprintln!("{}", t!("Поиск в SoundCloud…"));
        }
        self.list(&format!("scsearch{limit}:{query}"), true, None, found)
    }

    /// The tracks that a profile has liked, the latest first, handed over as yt-dlp
    /// lists them. `profile` is what `profile` reads: a name or a link.
    ///
    /// Such a list names each track and its link, and nothing of who made it or how
    /// long it is. What is not a single track comes along too: playlists are liked
    /// as well.
    pub fn likes(&self, profile: &str, found: impl FnMut(Track)) -> Result<()> {
        let name = self::profile(profile)?;
        if !self.quiet {
            eprintln!("{}", t!("Читаю лайки профиля {}…", name));
        }
        self.list(
            &format!("https://soundcloud.com/{name}/likes"),
            true,
            None,
            found,
        )
    }

    // Asks yt-dlp for the tracks of `target`, a search or a page, one per line. `flat`
    // takes what the list itself tells of each; otherwise every track is looked up,
    // which costs a request apiece and tells all of it but where the audio is.
    fn list(
        &self,
        target: &str,
        flat: bool,
        limit: Option<u16>,
        mut found: impl FnMut(Track),
    ) -> Result<()> {
        let mut command = self.extractor.command();
        if flat {
            command.arg("--flat-playlist");
        } else {
            command.args([
                "--ignore-no-formats-error",
                "--extractor-args",
                "soundcloud:formats=none",
            ]);
        }
        if let Some(limit) = limit {
            command.args(["--playlist-end", &limit.max(1).to_string()]);
        }
        command
            .args([
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
            .arg(target)
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
                "yt-dlp завершился с {}:\n{}",
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
        // Where nothing says who made the track, its link still names the profile.
        artist: entry
            .artist
            .or(entry.uploader)
            .or_else(|| crate::cache::named(&url).map(|track| track.artist))
            // A playlist that a page lists is named after the profile it belongs to.
            .or_else(|| owner(&url).map(|(profile, _)| profile.replace(['-', '_'], " ")))
            .unwrap_or_else(|| t!("Неизвестный исполнитель").into()),
        duration: entry.duration,
        url,
    })
}

/// What a link to SoundCloud leads to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    /// One track, which is played.
    Track,
    /// A playlist or an album.
    Set,
    /// A list that SoundCloud hands out in pages: of a profile, of a station, of what
    /// goes with a track.
    Listing,
}

/// The sections of a profile that list music, as its address names them.
pub const SECTIONS: [&str; 5] = ["tracks", "albums", "sets", "reposts", "likes"];

// The parts of the address of a page of SoundCloud; None for any other text.
fn parts(link: &str) -> Option<Vec<String>> {
    let link = link.trim();
    let link = match link.split_once("://") {
        Some(_) => link.to_owned(),
        None => format!("https://{link}"),
    };
    let url = Url::parse(&link).ok()?;
    let host = url.host_str()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(
            host,
            "soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com"
        )
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let parts: Vec<String> = (url.path_segments()?)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    (!parts.is_empty()).then_some(parts)
}

/// What `link` leads to, and the link as SoundCloud spells it; None for a text that is
/// no link to a track or to a list of them, which is then a question to search for.
pub fn page(link: &str) -> Option<(Page, String)> {
    let parts = parts(link)?;
    let names: Vec<&str> = parts.iter().map(String::as_str).collect();
    let page = match names[..] {
        ["stations", "track", _, _] => Page::Listing,
        [section, ..] if crate::cache::SECTIONS.contains(&section) => return None,
        [_] => Page::Listing,
        [_, "sets", _] | [_, "sets", _, _] => Page::Set,
        [_, list] if crate::cache::LISTS.contains(&list) => Page::Listing,
        [_, _] => Page::Track,
        [_, _, "recommended" | "albums" | "sets"] => Page::Listing,
        [_, _, secret] if secret.starts_with("s-") => Page::Track,
        _ => return None,
    };
    Some((page, format!("https://soundcloud.com/{}", parts.join("/"))))
}

/// The station of the track at `url`: that track and the ones SoundCloud plays after it.
pub fn station(url: &str) -> Option<String> {
    match (page(url)?, parts(url)?) {
        ((Page::Track, _), parts) => Some(format!(
            "https://soundcloud.com/stations/track/{}/{}",
            parts[0], parts[1]
        )),
        _ => None,
    }
}

/// The profile that the page at `url` belongs to and the section of it that the page
/// is, by its place in `SECTIONS`; None for a page that belongs to no profile.
pub fn owner(url: &str) -> Option<(String, Option<usize>)> {
    let parts = parts(url)?;
    page(url)?;
    if parts[0] == "stations" {
        return None;
    }
    let section = match &parts[1..] {
        [section] => SECTIONS.iter().position(|name| name == section),
        _ => None,
    };
    Some((parts[0].clone(), section))
}

/// The link to a section of a profile.
pub fn section(profile: &str, section: usize) -> String {
    format!(
        "https://soundcloud.com/{profile}/{}",
        SECTIONS[section % SECTIONS.len()]
    )
}

/// The name a profile has in its address, read from the name itself, `@name`, or a
/// link to the profile or to its likes.
pub fn profile(input: &str) -> Result<String> {
    let input = input.trim();
    let name = if input.contains('/') {
        let link = if input.contains("://") {
            input.to_owned()
        } else {
            format!("https://{input}")
        };
        let url = Url::parse(&link).ok().filter(|url| {
            matches!(url.scheme(), "https" | "http")
                && matches!(
                    url.host_str(),
                    Some("soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com")
                )
        });
        let parts: Vec<&str> = (url.iter())
            .flat_map(|url| url.path_segments().into_iter().flatten())
            .filter(|part| !part.is_empty())
            .collect();
        match parts[..] {
            [name] | [name, "likes"] => name.to_owned(),
            _ => String::new(),
        }
    } else {
        input.strip_prefix('@').unwrap_or(input).to_owned()
    };
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
    if name.is_empty() || !name.chars().all(plain) {
        return Err(t!(
            "Ожидается имя профиля SoundCloud или ссылка на него: name или soundcloud.com/name."
        )
        .into());
    }
    // After signing in the site shows one's own pages under `you`, which names nobody.
    if crate::cache::SECTIONS.contains(&name.as_str()) {
        return Err(t!(
            "{} - раздел сайта, а не профиль. Имя профиля стоит в адресе его страницы: soundcloud.com/name.",
            name
        )
        .into());
    }
    Ok(name)
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
        crate::lang::set(crate::lang::Lang::Russian);
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
    fn a_liked_track_is_named_after_the_profile_in_its_link() {
        crate::lang::set(crate::lang::Lang::Russian);
        // A line of the likes of a profile, as yt-dlp 2026.08 writes it.
        let line = br#"{"_type":"url","ie_key":"Soundcloud","id":"157407329","title":"Bitter Sweet Tears","url":"https://soundcloud.com/the-propolis/bitter-sweet-tears","webpage_url":"https://soundcloud.com/the-propolis/bitter-sweet-tears","duration":null,"uploader":null}"#;
        let tracks = parse_line(line).unwrap();
        assert_eq!(
            (tracks[0].artist.as_str(), tracks[0].title.as_str()),
            ("the propolis", "Bitter Sweet Tears")
        );
        assert!(tracks[0].duration.is_none());
        // A liked playlist is read as well; whoever keeps tracks leaves it out.
        let list =
            br#"{"_type":"url","title":"Album","url":"https://soundcloud.com/a/sets/album"}"#;
        assert_eq!(parse_line(list).unwrap()[0].artist, "a");
        let station = br#"{"title":"Station","url":"https://soundcloud.com/stations/track/a/b"}"#;
        assert_eq!(
            parse_line(station).unwrap()[0].artist,
            "Неизвестный исполнитель"
        );
    }

    #[test]
    fn reads_the_name_of_a_profile_from_a_name_or_a_link() {
        crate::lang::set(crate::lang::Lang::Russian);
        for input in [
            "night_owl-1",
            " @night_owl-1 ",
            "soundcloud.com/night_owl-1",
            "https://soundcloud.com/night_owl-1/",
            "https://soundcloud.com/night_owl-1/likes",
            "http://www.soundcloud.com/night_owl-1/likes/?si=1#x",
            "m.soundcloud.com/night_owl-1",
        ] {
            assert_eq!(profile(input).unwrap(), "night_owl-1", "{input}");
        }
        for input in [
            "",
            "@",
            "two words",
            "--proxy=x",
            "name?x=1",
            "https://soundcloud.com/",
            // A track, a playlist or a page of some other kind is not a profile.
            "https://soundcloud.com/artist/night",
            "https://soundcloud.com/artist/sets/album",
            "https://soundcloud.com/artist/tracks",
            "https://on.soundcloud.com/abc",
            "https://soundcloud.com.evil.test/name",
            "https://evil.test/soundcloud.com/name",
            "file:///name",
        ] {
            let error = profile(input).expect_err(input).to_string();
            assert!(error.contains("имя профиля"), "{input}: {error}");
        }
        for input in ["you", "https://soundcloud.com/you/likes", "discover"] {
            let error = profile(input).expect_err(input).to_string();
            assert!(error.contains("раздел сайта"), "{input}: {error}");
        }
    }

    #[test]
    fn details_of_a_download_replace_what_a_link_told() {
        let directory =
            std::env::temp_dir().join(format!("clicloud-details-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        assert!(Details::read(&directory).is_none());
        let file = directory.join(Details::FILE);
        // Nothing is known while the line is being written, or where yt-dlp knew nothing.
        for unknown in [
            "",
            "{\"title\":\"Rem",
            r#"{"title":null,"artist":null,"duration":null}"#,
        ] {
            std::fs::write(&file, unknown).unwrap();
            assert!(Details::read(&directory).is_none(), "{unknown}");
        }
        std::fs::write(
            &file,
            "{\"title\":\"Remote Viewing\",\"artist\":\"low_sea\",\"duration\":196.645}\nmore\n",
        )
        .unwrap();
        let details = Details::read(&directory).unwrap();
        let mut track = Track {
            title: "Remote Viewing".into(),
            artist: "low sea".into(),
            duration: None,
            url: "https://soundcloud.com/low-sea/remote-viewing".into(),
        };
        details.apply(&mut track);
        assert_eq!(
            (track.artist.as_str(), track.duration),
            ("low_sea", Some(196.645))
        );
        // What yt-dlp lacks leaves what was there.
        let partly = Details {
            title: Some(" ".into()),
            duration: Some(0.0),
            ..Details::default()
        };
        partly.apply(&mut track);
        assert_eq!(
            (track.title.as_str(), track.artist.as_str(), track.duration),
            ("Remote Viewing", "low_sea", Some(196.645))
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn tells_what_a_link_leads_to() {
        let at = |link: &str| page(link).map(|(page, _)| page);
        for (link, expected) in [
            (
                "https://soundcloud.com/low-sea/remote-viewing",
                Some(Page::Track),
            ),
            (
                "https://soundcloud.com/low-sea/remote-viewing/s-AbC12",
                Some(Page::Track),
            ),
            ("soundcloud.com/low-sea/sets/an-album", Some(Page::Set)),
            (
                "https://soundcloud.com/low-sea/sets/private/s-AbC12",
                Some(Page::Set),
            ),
            ("https://soundcloud.com/low-sea", Some(Page::Listing)),
            (
                "https://m.soundcloud.com/low-sea/tracks",
                Some(Page::Listing),
            ),
            ("https://soundcloud.com/low-sea/sets", Some(Page::Listing)),
            ("https://soundcloud.com/low-sea/likes", Some(Page::Listing)),
            (
                "https://soundcloud.com/low-sea/remote-viewing/recommended",
                Some(Page::Listing),
            ),
            (
                "https://soundcloud.com/stations/track/low-sea/remote-viewing",
                Some(Page::Listing),
            ),
            // A question to search for, whatever it looks like.
            ("low sea", None),
            ("low-sea/remote-viewing", None),
            ("https://example.com/low-sea/remote-viewing", None),
            ("https://soundcloud.com.evil.test/a/b", None),
            ("https://soundcloud.com/", None),
            ("https://soundcloud.com/discover", None),
            ("https://soundcloud.com/you/likes", None),
            ("https://soundcloud.com/a/b/c/d/e", None),
            ("https://on.soundcloud.com/abc", None),
        ] {
            assert_eq!(at(link), expected, "{link}");
        }
        // The link is spelled one way, without what a browser added to it.
        assert_eq!(
            page(" http://www.soundcloud.com/low-sea/sets/an-album/?si=1#t=3 ")
                .unwrap()
                .1,
            "https://soundcloud.com/low-sea/sets/an-album"
        );
        assert_eq!(
            station("https://soundcloud.com/low-sea/remote-viewing?si=1").as_deref(),
            Some("https://soundcloud.com/stations/track/low-sea/remote-viewing")
        );
        assert!(station("https://soundcloud.com/low-sea/sets/an-album").is_none());
        assert!(station("https://soundcloud.com/low-sea").is_none());
        // Whose page it is, and which of the sections of the profile.
        for (link, expected) in [
            (
                "https://soundcloud.com/low-sea/remote-viewing",
                Some(("low-sea", None)),
            ),
            (
                "https://soundcloud.com/low-sea/sets/an-album",
                Some(("low-sea", None)),
            ),
            ("https://soundcloud.com/low-sea", Some(("low-sea", None))),
            (
                "https://soundcloud.com/low-sea/tracks",
                Some(("low-sea", Some(0))),
            ),
            (
                "https://soundcloud.com/low-sea/likes",
                Some(("low-sea", Some(4))),
            ),
            (
                "https://soundcloud.com/stations/track/low-sea/remote-viewing",
                None,
            ),
            ("low sea", None),
        ] {
            let found = owner(link);
            let found = found
                .as_ref()
                .map(|(name, section)| (name.as_str(), *section));
            assert_eq!(found, expected, "{link}");
        }
        assert_eq!(
            section("low-sea", 1),
            "https://soundcloud.com/low-sea/albums"
        );
        assert_eq!(
            section("low-sea", 5),
            "https://soundcloud.com/low-sea/tracks"
        );
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
