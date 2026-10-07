use crate::Result;
use serde::Deserialize;
use std::{env, fs, path::PathBuf};
use url::Url;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    proxy: Option<String>,
    proxy_enabled: bool,
    pub cache_enabled: bool,
    pub cache_dir: Option<PathBuf>,
    /// 0 lifts the limit.
    pub cache_limit_mb: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            proxy: None,
            proxy_enabled: true,
            cache_enabled: true,
            cache_dir: None,
            cache_limit_mb: 1024,
        }
    }
}

/// The user's directory of one kind: `$variable`, or `fallback` in the home directory.
pub fn directory(variable: &str, fallback: &str) -> Option<PathBuf> {
    env::var_os(variable)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|v| PathBuf::from(v).join(fallback)))
}

pub fn load(path: Option<&PathBuf>) -> Result<Settings> {
    let default = directory("XDG_CONFIG_HOME", ".config").map(|v| v.join("clicloud/config.json"));
    let Some(file) = path.or(default.as_ref()) else {
        return Ok(Settings::default());
    };
    let data = match fs::read(file) {
        Ok(data) => data,
        Err(error) if path.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => {
            return Err(
                format!("Не удалось прочитать настройки {}: {error}", file.display()).into(),
            );
        }
    };
    Ok(serde_json::from_slice(&data).map_err(|_| {
        "Некорректные настройки: ожидается JSON с proxy, proxy_enabled, cache_enabled, cache_dir и cache_limit_mb."
    })?)
}

impl Settings {
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
            return Ok(Some("socks5h://127.0.0.1:9050".into()));
        }
        if let Some(value) = override_proxy {
            return validate(value).map(Some);
        }
        if !self.proxy_enabled {
            return Ok(None);
        }
        self.proxy.as_deref().map(validate).transpose()
    }
}

fn validate(value: &str) -> Result<String> {
    let parsed = Url::parse(value).map_err(|_| "Некорректный URL прокси.")?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h")
        || parsed.host_str().is_none()
        || parsed.port_or_known_default().is_none()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("Прокси: используйте http(s)://host:port или socks5(h)://host:port.".into());
    }
    Ok(value.to_owned())
}
