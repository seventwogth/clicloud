//! What the user keeps: the favorites and the tracks that were played last, in one
//! file. The interface changes it, and so does an import from a profile.
use crate::{Result, cache, config, soundcloud::Track};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub favorites: Vec<Track>,
    pub recent: Vec<Track>,
    /// What was to play when the interface was last closed: the track it stopped at,
    /// then what was ahead of it.
    pub queue: Vec<Track>,
}

/// What became of the tracks that were offered to the favorites.
#[derive(Debug, Default, PartialEq)]
pub struct Merged {
    pub added: usize,
    /// Among the favorites already, or offered twice.
    pub known: usize,
    /// Not a single track: a playlist or an album.
    pub skipped: usize,
}

impl Library {
    /// Adds those of `tracks` that are not among the favorites yet, after them and in
    /// the order given. Two spellings of a link to the same track are one track.
    pub fn merge(&mut self, tracks: impl IntoIterator<Item = Track>) -> Merged {
        let mut kept: HashSet<String> = (self.favorites.iter())
            .filter_map(|track| cache::key(&track.url))
            .collect();
        let mut merged = Merged::default();
        for track in tracks {
            let Some(key) = cache::key(&track.url) else {
                merged.skipped += 1;
                continue;
            };
            if kept.insert(key) {
                self.favorites.push(track);
                merged.added += 1;
            } else {
                merged.known += 1;
            }
        }
        merged
    }
}

// What makes two entries the same track: its name in the cache, or its link as it is.
fn name(track: &Track) -> String {
    cache::key(&track.url).unwrap_or_else(|| track.url.clone())
}

impl Library {
    /// Takes over what another process did to the file: `base` is the file as this
    /// one last read or wrote it, `disk` is the file now. A favorite that was added
    /// there joins ours, one that was removed there leaves, and ours stay otherwise.
    /// Returns whether anything changed.
    pub fn adopt(&mut self, base: &Library, disk: &Library) -> bool {
        let names = |tracks: &[Track]| -> HashSet<String> { tracks.iter().map(name).collect() };
        let (before, now) = (names(&base.favorites), names(&disk.favorites));
        let count = self.favorites.len();
        self.favorites
            .retain(|track| !before.contains(&name(track)) || now.contains(&name(track)));
        let mut changed = self.favorites.len() != count;
        let mut ours = names(&self.favorites);
        for track in &disk.favorites {
            if !before.contains(&name(track)) && ours.insert(name(track)) {
                self.favorites.push(track.clone());
                changed = true;
            }
        }
        // The history is one list: whoever played last has the latest one.
        let links = |tracks: &[Track]| -> Vec<String> { tracks.iter().map(name).collect() };
        if links(&disk.recent) != links(&base.recent) && links(&self.recent) == links(&base.recent)
        {
            self.recent = disk.recent.clone();
            changed = true;
        }
        changed
    }
}

/// What tells that the file was written since it was last looked at.
pub fn stamp(path: &PathBuf) -> Option<(std::time::SystemTime, u64)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// The library file: the given one, or the default one in the user's directory.
/// `command` is the one whose `--library` names another place.
pub fn path(explicit: Option<PathBuf>, command: &str) -> Result<PathBuf> {
    explicit
        .or_else(|| {
            config::directory("XDG_DATA_HOME", ".local/share")
                .map(|p| p.join("clicloud/library.json"))
        })
        .ok_or_else(|| {
            t!(
                "Не удалось определить путь библиотеки; укажите {} --library PATH",
                command
            )
            .into()
        })
}

pub fn load(path: &PathBuf) -> Result<Library> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)
            .map_err(|_| t!("Файл библиотеки повреждён; он не был перезаписан."))?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Library::default()),
        Err(e) => Err(e.into()),
    }
}

/// How long a transaction waits for the one before it: the interface saves on its
/// own thread and must not stand still for longer.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Read, change and replace the library under one OS lock. The separate lock file
/// stays in place: locking the JSON itself would stop protecting it after rename.
/// Closing the handle releases the lock even if the process exits unexpectedly.
pub fn update<T>(path: &PathBuf, change: impl FnOnce(&mut Library) -> T) -> Result<(T, Library)> {
    update_within(path, LOCK_WAIT, change)
}

fn update_within<T>(
    path: &PathBuf,
    wait: std::time::Duration,
    change: impl FnOnce(&mut Library) -> T,
) -> Result<(T, Library)> {
    if path.file_name().is_none() {
        return Err(
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid library file path").into(),
        );
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match fs2::FileExt::try_lock_exclusive(&lock) {
            Ok(()) => break,
            Err(error)
                if error.kind() == fs2::lock_contended_error().kind()
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    // A read/parse error must abort the transaction, preserving the original bytes.
    let mut library = load(path)?;
    let result = change(&mut library);
    write(path, &library)?;
    Ok((result, library))
}

#[cfg(test)]
pub fn save(path: &PathBuf, library: &Library) -> Result<()> {
    update(path, |disk| *disk = library.clone()).map(|_| ())
}

fn write(path: &PathBuf, library: &Library) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(library)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_process() {
        let Some(path) = std::env::var_os("CLICLOUD_TEST_LIBRARY") else {
            return;
        };
        let path = PathBuf::from(path);
        if let Some(marker) = std::env::var_os("CLICLOUD_TEST_LOCK_READY") {
            update(&path, |_| {
                fs::write(marker, b"locked").unwrap();
                std::thread::sleep(std::time::Duration::from_secs(30));
            })
            .unwrap();
            return;
        }
        // Writers that never pause take the lock from one another unfairly, and on a
        // slow disk the last of them waits longer than a transaction is given. What is
        // held here is that no addition is lost, not how long one waits for its turn.
        for index in 0..20 {
            update_within(&path, std::time::Duration::from_secs(60), |library| {
                library.merge([track(&format!(
                    "https://soundcloud.com/process-{}/track-{index}",
                    std::process::id()
                ))]);
                // Widen the race window: without the transaction lock writers lose
                // each other's additions even though every rename is atomic.
                std::thread::sleep(std::time::Duration::from_millis(2));
            })
            .unwrap();
        }
    }

    #[test]
    fn a_killed_writer_does_not_leave_the_library_locked() {
        let root = std::env::temp_dir().join(format!("clicloud-lock-death-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("library.json");
        let marker = root.join("ready");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "library::tests::writer_process"])
            .env("CLICLOUD_TEST_LIBRARY", &path)
            .env("CLICLOUD_TEST_LOCK_READY", &marker)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !marker.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = marker.exists();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ready, "child did not acquire the lock");
        update(&path, |library| {
            library.merge([track("https://soundcloud.com/a/after-crash")]);
        })
        .unwrap();
        assert_eq!(load(&path).unwrap().favorites.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_processes_keep_every_addition() {
        let root = std::env::temp_dir().join(format!("clicloud-writers-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("library.json");
        let mut children: Vec<_> = (0..4)
            .map(|_| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    // Uncaptured, what a writer fails with reaches the log of this test.
                    .args(["--exact", "library::tests::writer_process", "--nocapture"])
                    .env("CLICLOUD_TEST_LIBRARY", &path)
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(load(&path).unwrap().favorites.len(), 80);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn damaged_library_is_preserved_and_lock_is_released_on_error() {
        let root = std::env::temp_dir().join(format!("clicloud-damaged-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("library.json");
        fs::write(&path, b"{broken").unwrap();
        assert!(update(&path, |_| panic!("must not change damaged data")).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{broken");
        fs::write(&path, b"{}").unwrap();
        update(&path, |library| {
            library.merge([track("https://soundcloud.com/a/recovered")]);
        })
        .unwrap();
        assert_eq!(load(&path).unwrap().favorites.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    fn track(url: &str) -> Track {
        Track {
            title: "Night".into(),
            artist: "Artist".into(),
            duration: None,
            url: url.into(),
        }
    }

    #[test]
    fn what_another_process_wrote_is_taken_over() {
        let url = |name: &str| format!("https://soundcloud.com/a/{name}");
        let library = |favorites: &[&str], recent: &[&str]| Library {
            favorites: favorites.iter().map(|name| track(&url(name))).collect(),
            recent: recent.iter().map(|name| track(&url(name))).collect(),
            queue: vec![],
        };
        let listed = |tracks: &[Track]| -> Vec<String> {
            (tracks.iter())
                .map(|track| track.url.rsplit('/').next().unwrap().to_owned())
                .collect()
        };
        let base = library(&["one", "two", "three"], &["one"]);
        // Here `two` was removed and `ours` added; there `three` went and `theirs` came.
        let mut ours = library(&["one", "three", "ours"], &["one"]);
        let disk = library(&["one", "two", "theirs"], &["theirs", "one"]);
        assert!(ours.adopt(&base, &disk));
        assert_eq!(listed(&ours.favorites), ["one", "ours", "theirs"]);
        assert_eq!(listed(&ours.recent), ["theirs", "one"]);
        // Nothing more to take from the same file, and what was played here stays.
        let mut played = Library {
            recent: library(&[], &["ours", "one"]).recent,
            ..ours.clone()
        };
        assert!(!ours.adopt(&disk, &disk));
        assert!(!played.adopt(&base, &disk));
        assert_eq!(listed(&played.recent), ["ours", "one"]);
    }

    #[test]
    fn merging_adds_what_is_new_once_and_leaves_lists_out() {
        let mut library = Library {
            favorites: vec![track("https://soundcloud.com/a/kept")],
            recent: vec![track("https://soundcloud.com/a/played")],
            queue: vec![],
        };
        let merged = library.merge([
            track("https://soundcloud.com/b/new"),
            // The favorite under another spelling of its link.
            track("https://www.soundcloud.com/a/kept?si=1"),
            track("https://soundcloud.com/b/sets/album"),
            track("https://soundcloud.com/c/newer"),
            track("https://m.soundcloud.com/b/new/"),
            // Played before, but not a favorite.
            track("https://soundcloud.com/a/played"),
        ]);
        assert_eq!(
            merged,
            Merged {
                added: 3,
                known: 2,
                skipped: 1
            }
        );
        let links: Vec<&str> = (library.favorites.iter())
            .map(|track| track.url.as_str())
            .collect();
        assert_eq!(
            links,
            [
                "https://soundcloud.com/a/kept",
                "https://soundcloud.com/b/new",
                "https://soundcloud.com/c/newer",
                "https://soundcloud.com/a/played",
            ]
        );
        assert_eq!(library.recent.len(), 1);
        // Offered again, nothing is new.
        let again = library.merge(library.favorites.clone());
        assert_eq!((again.added, again.known), (0, 4));
    }
}
