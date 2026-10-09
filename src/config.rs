use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};
use url::Url;

pub const TOR: &str = "socks5h://127.0.0.1:9050";

/// What the settings file holds. The interface changes it too, so it is written back.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    pub proxy_enabled: bool,
    pub cache_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<PathBuf>,
    /// 0 lifts the limit.
    pub cache_limit_mb: u64,
    /// A color scheme of the interface; see `theme`.
    pub theme: String,
    /// The drawing behind the list of tracks: none, reaper or pentagram.
    pub backdrop: String,
    /// The language of the interface and the messages: ru, en or ja.
    pub language: String,
    /// How many tracks a search in the interface asks for.
    pub search_limit: u8,
    /// Seconds that the arrow keys seek by.
    pub seek_step: u16,
    /// The volume the interface starts with.
    pub volume: u8,
    /// How the picture of a track is drawn: blocks, braille, block-tones,
    /// braille-tones or none.
    pub cover: String,
    /// Whether tracks are brought to one loudness.
    pub normalize: bool,
    /// The device mpv plays on, by the name it knows it by; mpv chooses without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_device: Option<String>,
    /// Whether the media keys of the desktop reach the player: auto uses the plugin
    /// mpv-mpris where it is installed, off does not, anything else is its file.
    pub media_keys: String,
    /// Whether a list plays in an order of chance.
    pub shuffle: bool,
    /// What happens at the end of a list: off, all or one.
    pub repeat: String,
    /// The profile whose likes the interface was last asked to bring in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soundcloud_profile: Option<String>,
    /// Whether clicloud asks GitHub for a newer release of itself. None until the
    /// user has been asked, which the interface does once, at its first start: no
    /// request leaves the machine before there is an answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_check: Option<bool>,
    /// When the last look happened, in seconds since the epoch; 0 for never.
    pub update_checked: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            proxy: None,
            proxy_enabled: true,
            cache_enabled: true,
            cache_dir: None,
            cache_limit_mb: 1024,
            theme: crate::theme::TERMINAL.into(),
            backdrop: "reaper".into(),
            language: crate::lang::Lang::English.code().into(),
            // A track of a search costs about a second of yt-dlp's time, so a list of
            // thirty kept the search running long after the first screen was there.
            search_limit: 10,
            seek_step: 10,
            volume: 70,
            cover: "blocks".into(),
            normalize: false,
            audio_device: None,
            media_keys: "auto".into(),
            shuffle: false,
            repeat: "off".into(),
            soundcloud_profile: None,
            update_check: None,
            update_checked: 0,
        }
    }
}

/// The user's directory of one kind: `$variable`, or `fallback` in the home directory,
/// which Windows names differently.
pub fn directory(variable: &str, fallback: &str) -> Option<PathBuf> {
    let set = |name: &str| env::var_os(name).filter(|v| !v.is_empty());
    // A shell under Windows may set HOME to a path of a world of its own, such as
    // /home/user, which names nothing there; the profile is where the files belong.
    #[cfg(windows)]
    let homes = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let homes = ["HOME", "USERPROFILE"];
    set(variable).map(PathBuf::from).or_else(|| {
        homes
            .into_iter()
            .find_map(set)
            .map(|home| PathBuf::from(home).join(fallback))
    })
}

/// The settings file: the given one, or the default one in the user's directory.
pub fn path(explicit: Option<&PathBuf>) -> Option<PathBuf> {
    explicit
        .cloned()
        .or_else(|| directory("XDG_CONFIG_HOME", ".config").map(|v| v.join("clicloud/config.json")))
}

pub fn load(explicit: Option<&PathBuf>) -> Result<Settings> {
    let Some(file) = path(explicit) else {
        return Ok(Settings::default());
    };
    let data = match fs::read(&file) {
        Ok(data) => data,
        Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => {
            return Err(t!(
                "Не удалось прочитать настройки {}: {}",
                file.display(),
                error
            )
            .into());
        }
    };
    let mut settings: Settings = serde_json::from_slice(&data)
        .map_err(|error| t!("Некорректные настройки {}: {}", file.display(), error))?;
    settings.search_limit = settings.search_limit.clamp(1, 50);
    settings.seek_step = settings.seek_step.clamp(1, 600);
    settings.volume = settings.volume.min(100);
    Ok(settings)
}

impl Settings {
    /// How mpv is to sound.
    pub fn sound(&self) -> crate::player::Sound<'_> {
        crate::player::Sound {
            normalize: self.normalize,
            device: self.audio_device.as_deref(),
            plugin: None,
        }
    }

    pub fn proxy(
        &self,
        override_proxy: Option<&str>,
        tor: bool,
        disabled: bool,
    ) -> Result<Option<String>> {
        if disabled {
            return Ok(None);
        }
        if tor {
            return Ok(Some(TOR.into()));
        }
        if let Some(value) = override_proxy {
            return validate(value).map(Some);
        }
        if !self.proxy_enabled {
            return Ok(None);
        }
        self.proxy.as_deref().map(validate).transpose()
    }

    /// Writes the settings to `file`, replacing it in one step.
    pub fn save(&self, file: &Path) -> Result<()> {
        if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
        let written = (|| -> Result<()> {
            let mut output = fs::File::create(&temporary)?;
            output.write_all(&serde_json::to_vec_pretty(self)?)?;
            output.write_all(b"\n")?;
            output.sync_all()?;
            fs::rename(&temporary, file)?;
            Ok(())
        })();
        if written.is_err() {
            let _ = fs::remove_file(temporary);
        }
        written
    }
}

pub fn validate(value: &str) -> Result<String> {
    let parsed = Url::parse(value).map_err(|_| t!("Некорректный URL прокси."))?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h")
        || parsed.host_str().is_none()
        || parsed.port_or_known_default().is_none()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            t!("Прокси: используйте http(s)://host:port или socks5(h)://host:port.").into(),
        );
    }
    Ok(value.to_owned())
}
