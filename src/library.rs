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

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub favorites: Vec<Track>,
    pub recent: Vec<Track>,
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
    fn merging_adds_what_is_new_once_and_leaves_lists_out() {
        let mut library = Library {
            favorites: vec![track("https://soundcloud.com/a/kept")],
            recent: vec![track("https://soundcloud.com/a/played")],
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
