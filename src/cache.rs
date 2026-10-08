//! Audio kept on disk: a track that was played starts at once the next time and
//! needs no network. yt-dlp keeps what it learns about SoundCloud next to it.
use crate::{
    Result,
    soundcloud::{Details, Track},
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime},
};
use url::Url;

const TAG: &str = "CACHEDIR.TAG";
// The signature makes backup tools skip the directory; the comment marks it as ours.
const TAG_CONTENT: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
    # This directory is a cache of clicloud. It can be removed at any time.\n";
// An unfinished download this old was left behind by a killed process.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);
// Profile pages and site sections: links that look like `artist/track` but are lists.
pub const LISTS: [&str; 11] = [
    "sets",
    "tracks",
    "albums",
    "likes",
    "reposts",
    "popular-tracks",
    "toptracks",
    "spotlight",
    "followers",
    "following",
    "comments",
];
pub const SECTIONS: [&str; 10] = [
    "discover", "search", "you", "stream", "charts", "stations", "tags", "people", "pages",
    "upload",
];
// Windows keeps these names for devices, whatever the extension and the letter case,
// so a file cannot be called one of them; such a part of a link is escaped instead.
const DEVICES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

pub struct Cache {
    root: PathBuf,
    extractor: PathBuf,
    limit: u64,
}

impl Cache {
    /// Uses `root`, which must be empty or a cache of ours: files in it get removed.
    /// `limit` is the size in bytes the audio is kept under; 0 means no limit.
    pub fn open(root: PathBuf, limit: u64) -> Result<Self> {
        let claim = || -> io::Result<bool> {
            private_directory(&root)?;
            let tag = root.join(TAG);
            match fs::read(&tag) {
                Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).contains("clicloud")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if fs::read_dir(&root)?.next().is_some() {
                        return Ok(false);
                    }
                    fs::write(tag, TAG_CONTENT)?;
                    Ok(true)
                }
                Err(error) => Err(error),
            }
        };
        match claim() {
            Ok(true) => Ok(Self {
                extractor: root.join("yt-dlp"),
                root,
                limit,
            }),
            Ok(false) => Err(t!(
                "Каталог {} не пуст и не является кешем clicloud. Укажите другой --cache-dir или --no-cache.",
                root.display()
            )
            .into()),
            Err(error) => Err(t!(
                "Не удалось подготовить кеш {}: {}. Укажите --cache-dir или --no-cache.",
                root.display(),
                error
            )
            .into()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// The directory for yt-dlp's own cache, apart from the one in the user's home.
    pub fn extractor(&self) -> &Path {
        &self.extractor
    }

    fn audio(&self) -> PathBuf {
        self.root.join("audio")
    }

    /// All that was looked up of the track at `url`, kept for its screen.
    pub fn info(&self, url: &str) -> Option<Details> {
        let text = fs::read(self.root.join("info").join(key(url)?)).ok()?;
        Details::parse(&text)
    }

    /// Keeps what was looked up of the track at `url`.
    pub fn note(&self, url: &str, details: &Details) {
        if let Some(key) = key(url) {
            let _ = write_info(&self.root.join("info"), &key, details);
        }
    }

    /// The pictures of tracks as pixels, each in a file named like the audio.
    pub fn art(&self) -> PathBuf {
        self.root.join("art")
    }

    // What is known about each track, in a file named like its audio.
    fn details(&self) -> PathBuf {
        self.root.join("tracks")
    }

    fn path(&self, url: &str) -> Option<PathBuf> {
        key(url).map(|key| self.audio().join(key))
    }

    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }

    /// Whether `url` names a single track, the only kind of link that is stored.
    pub fn accepts(&self, url: &str) -> bool {
        key(url).is_some()
    }

    pub fn contains(&self, url: &str) -> bool {
        self.path(url).is_some_and(|path| path.is_file())
    }

    /// The stored audio of `url`, without noting a use of it.
    pub fn stored(&self, url: &str) -> Option<PathBuf> {
        self.path(url).filter(|path| path.is_file())
    }

    /// The stored audio of `url`, noted as used just now.
    pub fn find(&self, url: &str) -> Option<PathBuf> {
        let path = self.path(url).filter(|path| path.is_file())?;
        // The time of the last use decides what is evicted first.
        if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(path)
    }

    /// A place to download `url` into; None when the link is not a single track.
    pub fn store(&self, url: &str) -> Result<Option<Partial>> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let Some(target) = self.path(url) else {
            return Ok(None);
        };
        let partial = self.root.join("partial");
        sweep(&partial);
        let directory = partial.join(format!(
            "{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        // A killed process with the same id may have left one behind.
        let _ = fs::remove_dir_all(&directory);
        private_directory(&directory)?;
        Ok(Some(Partial {
            directory,
            target,
            details: self.details(),
            url: url.to_owned(),
            track: None,
            limit: self.limit,
        }))
    }

    /// Records the title and artist of a stored track that was known by its link only.
    pub fn describe(&self, track: &Track) {
        if let Some(key) = key(&track.url).filter(|_| self.contains(&track.url))
            && !self.details().join(&key).exists()
        {
            let _ = write_details(&self.details(), &key, track);
        }
    }

    /// Records what has become known of a stored track in place of what was recorded.
    pub fn correct(&self, track: &Track) {
        if let Some(key) = key(&track.url).filter(|_| self.contains(&track.url)) {
            let _ = write_details(&self.details(), &key, track);
        }
    }

    /// The stored tracks by artist and title. One stored by its link alone is named after it.
    pub fn tracks(&self) -> Vec<Track> {
        let mut tracks: Vec<Track> = files(&self.audio())
            .into_iter()
            .filter_map(|(_, _, path)| {
                let name = path.file_name()?.to_str()?;
                fs::read(self.details().join(name))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Track>(&bytes).ok())
                    .filter(|track| key(&track.url).as_deref() == Some(name))
                    .or_else(|| named_after(name))
            })
            .collect();
        tracks
            .sort_by_cached_key(|track| (track.artist.to_lowercase(), track.title.to_lowercase()));
        tracks
    }

    /// Removes the stored audio of `url`; false if there was none.
    pub fn remove(&self, url: &str) -> io::Result<bool> {
        let Some(key) = key(url) else {
            return Ok(false);
        };
        let _ = fs::remove_file(self.details().join(&key));
        match fs::remove_file(self.audio().join(key)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The number of stored tracks and their size in bytes.
    pub fn usage(&self) -> (usize, u64) {
        let files = files(&self.audio());
        (files.len(), files.iter().map(|file| file.1).sum())
    }

    /// Removes the audio, unfinished downloads and what yt-dlp has stored.
    pub fn clear(&self) -> io::Result<()> {
        for name in ["audio", "tracks", "art", "info", "partial", "yt-dlp"] {
            match fs::remove_dir_all(self.root.join(name)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => (),
            }
        }
        Ok(())
    }
}

/// A download in progress: a directory of its own, where yt-dlp also keeps its fragments.
pub struct Partial {
    directory: PathBuf,
    target: PathBuf,
    details: PathBuf,
    url: String,
    track: Option<Track>,
    limit: u64,
}

impl Partial {
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The title and artist to keep with the audio, for the list of stored tracks.
    pub fn describe(&mut self, track: &Track) {
        self.track = Some(track.clone());
    }

    /// The file that receives the audio.
    pub fn path(&self) -> PathBuf {
        self.directory.join("audio")
    }

    /// Makes the finished download a part of the cache, evicting what was used longest ago.
    pub fn commit(mut self) -> io::Result<()> {
        // What yt-dlp noted of the track while fetching it outranks what was known
        // before, and names a track that was asked for by its link alone.
        if let Some(noted) = Details::read(&self.directory) {
            let mut track = (self.track.take())
                .or_else(|| named(&self.url))
                .unwrap_or_else(|| Track {
                    title: String::new(),
                    artist: String::new(),
                    duration: None,
                    url: self.url.clone(),
                });
            noted.apply(&mut track);
            self.track = Some(track).filter(|track| !track.title.is_empty());
            // The rest of it is kept too, for the screen of the track.
            if let (Some(root), Some(key)) = (self.details.parent(), key(&self.url)) {
                let _ = write_info(&root.join("info"), &key, &noted);
            }
        }
        let path = self.path();
        if fs::metadata(&path)?.len() == 0 {
            return Err(io::Error::other("empty download"));
        }
        let audio = self.target.parent().expect("file in the audio directory");
        private_directory(audio)?;
        fs::rename(path, &self.target)?;
        if let Some(track) = &self.track
            && let Some(key) = self.target.file_name().and_then(|name| name.to_str())
        {
            let _ = write_details(&self.details, key, track);
        }
        if self.limit > 0 {
            let mut files = files(audio);
            let mut total: u64 = files.iter().map(|file| file.1).sum();
            files.sort();
            for (_, size, path) in files {
                if total <= self.limit {
                    break;
                }
                if path != self.target && fs::remove_file(&path).is_ok() {
                    total -= size;
                    if let Some(name) = path.file_name() {
                        let _ = fs::remove_file(self.details.join(name));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// A directory that only its owner may look into, made with its parents.
pub fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    // What was listened to is nobody else's business.
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

// Files of a directory with the time of their last use and their size.
fn files(directory: &Path) -> Vec<(SystemTime, u64, PathBuf)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let metadata = entry
                .metadata()
                .ok()
                .filter(|metadata| metadata.is_file())?;
            Some((metadata.modified().ok()?, metadata.len(), entry.path()))
        })
        .collect()
}

fn write_info(directory: &Path, key: &str, details: &Details) -> io::Result<()> {
    private_directory(directory)?;
    fs::write(directory.join(key), serde_json::to_vec(details)?)
}

fn write_details(directory: &Path, key: &str, track: &Track) -> io::Result<()> {
    private_directory(directory)?;
    fs::write(directory.join(key), serde_json::to_vec(track)?)
}

/// The track at `url` as far as its link tells: who made it and what it is called.
pub fn named(url: &str) -> Option<Track> {
    key(url).and_then(|key| named_after(&key))
}

// The track behind a file name made by `key`, as far as the link tells.
fn named_after(key: &str) -> Option<Track> {
    let mut parts = Vec::new();
    for part in key.split('.') {
        let mut bytes = Vec::new();
        let mut rest = part.bytes();
        while let Some(byte) = rest.next() {
            if byte == b'~' {
                let code = [rest.next()?, rest.next()?];
                bytes.push(u8::from_str_radix(std::str::from_utf8(&code).ok()?, 16).ok()?);
            } else {
                bytes.push(byte);
            }
        }
        parts.push(String::from_utf8(bytes).ok()?);
    }
    let words = |part: &String| part.replace(['-', '_'], " ");
    Some(Track {
        title: words(parts.get(1)?),
        artist: words(&parts[0]),
        duration: None,
        url: format!("https://soundcloud.com/{}", parts.join("/")),
    })
}

fn sweep(partial: &Path) {
    let Ok(entries) = fs::read_dir(partial) else {
        return;
    };
    for entry in entries.flatten() {
        let age = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok());
        if age.is_some_and(|age| age > STALE) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// A file name for the track at `url`; None unless the link names exactly one track.
/// Distinct tracks get distinct names: the parts are escaped and joined by a dot.
/// Two spellings of a link to the same track give the same name. Upper case is escaped
/// as well, so that the names also differ where the file system ignores it.
pub fn key(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    let host = url.host_str()?;
    let host = ["www.", "m."]
        .iter()
        .find_map(|prefix| host.strip_prefix(prefix))
        .unwrap_or(host);
    if host != "soundcloud.com" {
        return None;
    }
    let parts: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    match parts[..] {
        [artist, track] if !SECTIONS.contains(&artist) && !LISTS.contains(&track) => (),
        // A private track carries its secret token after the name.
        [artist, track, secret]
            if !SECTIONS.contains(&artist)
                && !LISTS.contains(&track)
                && secret.starts_with("s-") => {}
        _ => return None,
    }
    let mut key = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            key.push('.');
        }
        // A device name counts up to the first dot, so only the first part can spell one.
        let device = index == 0 && DEVICES.iter().any(|name| part.eq_ignore_ascii_case(name));
        for byte in part.bytes() {
            let plain =
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_');
            if plain && !device {
                key.push(char::from(byte));
            } else {
                key.push_str(&format!("~{byte:02x}"));
            }
        }
    }
    // File names are limited to 255 bytes; such a link is merely not stored.
    (key.len() <= 200).then_some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("clicloud-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    // Windows opens a directory only for a program that says it handles backups.
    fn age(path: &Path, when: SystemTime) {
        let mut options = fs::OpenOptions::new();
        // Unix opens a directory for reading only, which is enough to set its time.
        options.read(true).write(cfg!(windows) || !path.is_dir());
        #[cfg(windows)]
        if path.is_dir() {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            options
                .access_mode(FILE_WRITE_ATTRIBUTES)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
        }
        options.open(path).unwrap().set_modified(when).unwrap();
    }

    fn download(cache: &Cache, url: &str, bytes: &[u8]) {
        let partial = cache.store(url).unwrap().unwrap();
        fs::write(partial.path(), bytes).unwrap();
        partial.commit().unwrap();
    }

    #[test]
    fn stored_tracks_are_listed_with_what_is_known_about_them() {
        let root = directory("list");
        let cache = Cache::open(root.clone(), 15).unwrap();
        let known = Track {
            title: "Zebra".into(),
            artist: "beta".into(),
            duration: Some(61.5),
            url: "https://soundcloud.com/b/zebra?si=1".into(),
        };
        let mut partial = cache.store(&known.url).unwrap().unwrap();
        partial.describe(&known);
        fs::write(partial.path(), [0; 5]).unwrap();
        partial.commit().unwrap();
        // Stored by its link alone, as `play` with a link does.
        download(
            &cache,
            "https://soundcloud.com/Alpha_x/night-drive%20a",
            &[0; 5],
        );

        let tracks = cache.tracks();
        assert_eq!(tracks.len(), 2);
        assert_eq!(
            (
                tracks[0].artist.as_str(),
                tracks[0].title.as_str(),
                tracks[0].duration
            ),
            ("Alpha x", "night drive%20a", None)
        );
        assert_eq!(
            tracks[0].url,
            "https://soundcloud.com/Alpha_x/night-drive%20a"
        );
        assert!(cache.contains(&tracks[0].url));
        assert_eq!(
            (tracks[1].title.as_str(), tracks[1].duration),
            ("Zebra", Some(61.5))
        );

        // What becomes known later is kept, but never overwrites.
        let learned = Track {
            title: "Night Drive".into(),
            ..tracks[0].clone()
        };
        cache.describe(&learned);
        cache.describe(&tracks[0]);
        assert_eq!(cache.tracks()[0].title, "Night Drive");
        // Details of some other track do not pass for this one.
        fs::write(
            root.join("tracks/b.zebra"),
            serde_json::to_vec(&learned).unwrap(),
        )
        .unwrap();
        assert_eq!(cache.tracks()[1].title, "zebra");

        assert!(cache.remove(&learned.url).unwrap() && !cache.remove(&learned.url).unwrap());
        assert!(!cache.remove("https://soundcloud.com/a/sets/b").unwrap());
        assert!(!root.join("tracks/~41lpha_x.night-drive~2520a").exists());
        assert_eq!(cache.tracks().len(), 1);
        // Eviction takes the details along.
        download(&cache, "https://soundcloud.com/c/big", &[0; 12]);
        assert_eq!(cache.tracks().len(), 1);
        assert!(!root.join("tracks/b.zebra").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn what_the_download_noted_of_a_track_is_kept_with_it() {
        let root = directory("noted");
        let cache = Cache::open(root.clone(), 0).unwrap();
        let note = |partial: &Partial, line: &str| {
            fs::write(partial.directory().join(Details::FILE), line).unwrap();
            fs::write(partial.path(), b"audio").unwrap();
        };
        // Asked for by its link alone, as `play` with a link does.
        let partial = cache
            .store("https://soundcloud.com/low-sea/one")
            .unwrap()
            .unwrap();
        note(
            &partial,
            r#"{"title":"One","artist":"low_sea","duration":61.5}"#,
        );
        partial.commit().unwrap();
        // Known from the likes of a profile: a title, and a maker guessed from the link.
        let mut partial = cache
            .store("https://soundcloud.com/low-sea/two")
            .unwrap()
            .unwrap();
        partial.describe(&named("https://soundcloud.com/low-sea/two").unwrap());
        note(
            &partial,
            r#"{"title":"Two","artist":"low_sea","duration":null}"#,
        );
        partial.commit().unwrap();

        let tracks = cache.tracks();
        let told: Vec<_> = (tracks.iter())
            .map(|track| (track.title.as_str(), track.artist.as_str(), track.duration))
            .collect();
        assert_eq!(
            told,
            [("One", "low_sea", Some(61.5)), ("Two", "low_sea", None)]
        );

        // What becomes known later replaces what was recorded, for a stored track only.
        let longer = Track {
            duration: Some(90.0),
            ..tracks[1].clone()
        };
        cache.correct(&longer);
        assert_eq!(cache.tracks()[1].duration, Some(90.0));
        cache.correct(&Track {
            url: "https://soundcloud.com/low-sea/three".into(),
            ..longer
        });
        assert_eq!(cache.tracks().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_links_to_a_single_track_get_a_name() {
        for (url, expected) in [
            ("https://soundcloud.com/artist/night", Some("artist.night")),
            (
                "http://www.soundcloud.com/artist/night/?si=1&utm_source=x#t=10",
                Some("artist.night"),
            ),
            // Upper case is escaped: a file system that ignores it tells these apart.
            (
                "https://m.soundcloud.com/Artist_1/a-b",
                Some("~41rtist_1.a-b"),
            ),
            (
                "https://soundcloud.com/artist/night/s-AbC12",
                Some("artist.night.s-~41b~4312"),
            ),
            ("https://soundcloud.com/test/night", Some("test.night")),
            ("https://soundcloud.com/Test/Night", Some("~54est.~4eight")),
            // A name that Windows keeps for a device is escaped where it would count.
            ("https://soundcloud.com/con/radio", Some("~63~6f~6e.radio")),
            ("https://soundcloud.com/NUL/x", Some("~4e~55~4c.x")),
            ("https://soundcloud.com/com1/z", Some("~63~6f~6d~31.z")),
            ("https://soundcloud.com/conx/radio", Some("conx.radio")),
            // Only the first part can spell one: the rest follow a dot.
            ("https://soundcloud.com/artist/nul", Some("artist.nul")),
            // No two links share a name, whatever their characters.
            ("https://soundcloud.com/a.b/c", Some("a~2eb.c")),
            ("https://soundcloud.com/a/b.c", Some("a.b~2ec")),
            ("https://soundcloud.com/a/%D0%B0", Some("a.~25~440~25~420")),
            ("https://soundcloud.com/artist", None),
            ("https://soundcloud.com/artist/sets/album", None),
            ("https://soundcloud.com/artist/sets", None),
            ("https://soundcloud.com/artist/likes", None),
            ("https://soundcloud.com/discover/sets", None),
            ("https://soundcloud.com/artist/night/recommended", None),
            ("https://on.soundcloud.com/abc", None),
            ("https://api.soundcloud.com/tracks/123", None),
            ("not a link", None),
        ] {
            assert_eq!(key(url).as_deref(), expected, "{url}");
        }
        let long = format!("https://soundcloud.com/a/{}", "b".repeat(250));
        assert!(key(&long).is_none());
        // Every name a link gets is one that every file system of ours can hold.
        for url in [
            "https://soundcloud.com/Test/Night",
            "https://soundcloud.com/con/radio",
            "https://soundcloud.com/NUL/x",
        ] {
            let name = key(url).unwrap();
            assert_eq!(name, name.to_lowercase(), "{url}");
            let stem = name.split('.').next().unwrap();
            assert!(!DEVICES.contains(&stem), "{url}");
        }
    }

    #[test]
    fn finished_downloads_are_kept_and_unfinished_ones_removed() {
        let root = directory("store");
        let cache = Cache::open(root.clone(), 0).unwrap();
        let url = "https://soundcloud.com/artist/night";
        assert!(cache.accepts(url) && !cache.contains(url) && cache.find(url).is_none());
        assert!(
            cache
                .store("https://soundcloud.com/artist/sets/a")
                .unwrap()
                .is_none()
        );

        let abandoned = cache.store(url).unwrap().unwrap();
        fs::write(abandoned.path(), b"half").unwrap();
        let left = abandoned.directory().to_owned();
        drop(abandoned);
        assert!(!left.exists() && !cache.contains(url));

        let empty = cache.store(url).unwrap().unwrap();
        fs::write(empty.path(), b"").unwrap();
        assert!(empty.commit().is_err() && !cache.contains(url));

        download(&cache, url, b"audio");
        assert_eq!(fs::read(cache.find(url).unwrap()).unwrap(), b"audio");
        // The same track under another spelling of its link.
        assert!(cache.contains("https://www.soundcloud.com/artist/night?si=1"));
        assert_eq!(cache.usage(), (1, 5));
        assert_eq!(fs::read_dir(root.join("partial")).unwrap().count(), 0);

        // Reopening finds the same cache; clearing leaves the directory usable.
        let cache = Cache::open(root.clone(), 0).unwrap();
        assert!(cache.contains(url));
        fs::create_dir(cache.extractor()).unwrap();
        cache.clear().unwrap();
        assert_eq!(cache.usage(), (0, 0));
        assert!(!cache.extractor().exists() && root.join(TAG).exists());
        download(&cache, url, b"again");
        assert!(cache.contains(url));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tracks_used_longest_ago_are_evicted_first() {
        let root = directory("evict");
        let cache = Cache::open(root.clone(), 25).unwrap();
        let url = |name: &str| format!("https://soundcloud.com/artist/{name}");
        let older = |name: &str, seconds: u64| {
            age(
                &root.join("audio").join(format!("artist.{name}")),
                SystemTime::now() - Duration::from_secs(seconds),
            )
        };
        download(&cache, &url("old"), &[0; 10]);
        older("old", 300);
        download(&cache, &url("older"), &[0; 10]);
        older("older", 600);
        // Playing a track makes it the most recent one.
        assert!(cache.find(&url("older")).is_some());
        download(&cache, &url("new"), &[0; 10]);
        assert!(!cache.contains(&url("old")));
        assert!(cache.contains(&url("older")) && cache.contains(&url("new")));
        // A track larger than the whole limit still plays from the cache once.
        download(&cache, &url("huge"), &[0; 40]);
        assert_eq!(cache.usage(), (1, 40));
        assert!(cache.contains(&url("huge")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn foreign_directories_are_left_alone() {
        let root = directory("foreign");
        fs::create_dir_all(root.join("audio")).unwrap();
        fs::write(root.join("audio/song.mp3"), b"mine").unwrap();
        let error = Cache::open(root.clone(), 1).err().unwrap().to_string();
        assert!(error.contains("не является кешем"), "{error}");
        assert!(root.join("audio/song.mp3").exists() && !root.join(TAG).exists());
        // Someone else's cache is not ours either.
        fs::write(
            root.join(TAG),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        assert!(Cache::open(root.clone(), 1).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn downloads_abandoned_long_ago_are_swept() {
        let root = directory("sweep");
        let cache = Cache::open(root.clone(), 0).unwrap();
        let stale = root.join("partial/1-0");
        let fresh = root.join("partial/1-1");
        for path in [&stale, &fresh] {
            fs::create_dir_all(path).unwrap();
        }
        age(&stale, SystemTime::now() - STALE * 2);
        let partial = cache.store("https://soundcloud.com/a/b").unwrap().unwrap();
        assert!(!stale.exists() && fresh.exists() && partial.directory().exists());
        drop(partial);
        fs::remove_dir_all(root).unwrap();
    }
}
