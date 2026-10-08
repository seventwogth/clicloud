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

pub fn save(path: &PathBuf, library: &Library) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
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
