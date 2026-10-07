mod cache;
mod config;
#[cfg(unix)]
mod playback;
mod player;
mod soundcloud;
mod theme;
#[cfg(unix)]
mod ui;

use cache::Cache;
use clap::{Parser, Subcommand};
use soundcloud::{Extractor, SoundCloud, Track};
use std::io::{self, IsTerminal, Write};
use std::process::{Command, ExitCode};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(version, about = "Поиск и прослушивание SoundCloud в терминале")]
struct Cli {
    /// Путь к yt-dlp
    #[arg(long, global = true, env = "CLICLOUD_YT_DLP", default_value_t = default_yt_dlp())]
    yt_dlp: String,
    /// Путь к mpv
    #[arg(long, global = true, env = "CLICLOUD_MPV", default_value = "mpv")]
    mpv: String,
    /// JSON-файл настроек (по умолчанию ~/.config/clicloud/config.json)
    #[arg(long, global = true, env = "CLICLOUD_CONFIG")]
    config: Option<std::path::PathBuf>,
    /// HTTP(S) или SOCKS5 прокси
    #[arg(long, global = true, env = "CLICLOUD_PROXY", hide_env_values = true)]
    proxy: Option<String>,
    /// Использовать локальный Tor: socks5h://127.0.0.1:9050
    #[arg(long, global = true, conflicts_with = "no_proxy")]
    tor: bool,
    /// Отключить прокси, включая настройки окружения
    #[arg(long, global = true)]
    no_proxy: bool,
    /// Каталог кеша (по умолчанию ~/.cache/clicloud)
    #[arg(long, global = true, env = "CLICLOUD_CACHE_DIR")]
    cache_dir: Option<std::path::PathBuf>,
    /// Ничего не хранить на диске: ни аудио, ни данные yt-dlp
    #[arg(
        long,
        global = true,
        env = "CLICLOUD_NO_CACHE",
        value_parser = clap::builder::FalseyValueParser::new()
    )]
    no_cache: bool,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Открыть терминальный интерфейс (также запускается без команды)
    Ui {
        /// Путь к JSON-библиотеке
        #[arg(long)]
        library: Option<std::path::PathBuf>,
    },
    /// Найти треки
    Search {
        query: String,
        #[arg(short, long, default_value_t = 10, value_parser = clap::value_parser!(u8).range(1..=50))]
        limit: u8,
        /// Вывести JSON вместо списка
        #[arg(long)]
        json: bool,
    },
    /// Найти и выбрать трек или проиграть ссылку SoundCloud
    Play {
        query_or_url: String,
        #[arg(short, long, default_value_t = 10, value_parser = clap::value_parser!(u8).range(1..=50))]
        limit: u8,
        /// Воспроизвести первый результат без выбора
        #[arg(long)]
        first: bool,
    },
    /// Показать, сколько занимает кеш, или очистить его
    Cache {
        /// Удалить сохранённое аудио и данные yt-dlp
        #[arg(long)]
        clear: bool,
    },
    /// Проверить наличие внешних программ
    Doctor,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if let Some(player::Interrupted(signal)) = error.downcast_ref() {
                return ExitCode::from(128 + *signal as u8);
            }
            eprintln!("Ошибка: {}", clean_lines(&error.to_string()));
            ExitCode::FAILURE
        }
    }
}

fn default_yt_dlp() -> String {
    let installed = std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| executable(&path.join("yt-dlp")))
    });
    if !installed && let Some(local) = local_yt_dlp() {
        return local.to_string_lossy().into_owned();
    }
    "yt-dlp".into()
}

// yt-dlp of the project whose Cargo target directory holds this binary. The
// current directory is never searched: it may belong to someone else.
fn local_yt_dlp() -> Option<std::path::PathBuf> {
    let binary = std::env::current_exe().ok()?;
    let target = binary.parent()?.parent()?;
    if target.file_name()? != "target" {
        return None;
    }
    let local = target.parent()?.join(".tools/venv/bin/yt-dlp");
    executable(&local).then_some(local)
}

fn executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

// Where the cache is, used or not. The flag outranks the settings file; without a
// home directory there is nowhere to put it.
fn cache_directory(cli: &Cli, settings: &config::Settings) -> Result<Option<std::path::PathBuf>> {
    let root = cli
        .cache_dir
        .clone()
        .or_else(|| settings.cache_dir.clone())
        .or_else(|| {
            config::directory("XDG_CACHE_HOME", ".cache").map(|path| path.join("clicloud"))
        });
    // yt-dlp runs in a directory of its own, so the path must not depend on ours.
    Ok(root.map(std::path::absolute).transpose()?)
}

fn open_cache(
    cli: &Cli,
    settings: &config::Settings,
    root: Option<&std::path::PathBuf>,
) -> Result<Option<Cache>> {
    let Some(root) = root else {
        return Ok(None);
    };
    if cli.no_cache || (cli.cache_dir.is_none() && !settings.cache_enabled) {
        return Ok(None);
    }
    let limit = settings.cache_limit_mb.saturating_mul(1024 * 1024);
    Cache::open(root.clone(), limit).map(Some)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} МБ", bytes as f64 / (1024.0 * 1024.0))
}

fn describe(cache: Option<&Cache>) -> String {
    let Some(cache) = cache else {
        return "Кеш: отключён".into();
    };
    let (tracks, size) = cache.usage();
    let limit = match cache.limit() {
        0 => "без ограничения".into(),
        limit => format!("из {}", megabytes(limit)),
    };
    format!(
        "Кеш: {}\nТреков в кеше: {tracks}, {} {limit}",
        cache.root().display(),
        megabytes(size)
    )
}

fn run(cli: Cli) -> Result<()> {
    let settings = config::load(cli.config.as_ref())?;
    let proxy = settings.proxy(cli.proxy.as_deref(), cli.tor, cli.no_proxy)?;
    let cache_dir = cache_directory(&cli, &settings)?;
    let cache = open_cache(&cli, &settings, cache_dir.as_ref())?;
    let extractor = Extractor {
        program: &cli.yt_dlp,
        proxy: proxy.as_deref(),
        no_proxy: cli.no_proxy,
        cache: cache.as_ref().map(Cache::extractor),
    };
    let provider = SoundCloud::new(extractor);
    match cli.command.unwrap_or(Action::Ui { library: None }) {
        Action::Ui { library } => {
            #[cfg(unix)]
            ui::run(
                ui::Session {
                    yt_dlp: cli.yt_dlp,
                    mpv: cli.mpv,
                    proxy,
                    direct: cli.no_proxy,
                    cache,
                    cache_dir,
                    settings,
                    config: config::path(cli.config.as_ref()),
                },
                library,
            )?;
            #[cfg(not(unix))]
            {
                let _ = library;
                return Err("TUI пока поддерживается на Unix.".into());
            }
        }
        Action::Search { query, limit, json } => {
            let tracks = provider.search(&query, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&tracks)?);
            } else {
                print_tracks(&tracks);
            }
        }
        Action::Play {
            query_or_url,
            limit,
            first,
        } => {
            // Check before making network requests or prompting.
            version(&cli.mpv)?;
            version(&cli.yt_dlp)?;
            let target = query_or_url.trim();
            let mut found = None;
            let url = if target.contains("://") {
                soundcloud::validate_url(target)?;
                target.to_owned()
            } else {
                if !first && !io::stdin().is_terminal() {
                    return Err(
                        "Для выбора нужен терминал. Используйте --first или ссылку SoundCloud."
                            .into(),
                    );
                }
                let tracks = provider.search(target, limit)?;
                if tracks.is_empty() {
                    return Err("Треки не найдены. Попробуйте другой запрос.".into());
                }
                let index = if first {
                    0
                } else {
                    print_tracks(&tracks);
                    match choose(tracks.len())? {
                        Some(index) => index,
                        None => return Ok(()),
                    }
                };
                let track = tracks
                    .into_iter()
                    .nth(index)
                    .expect("index within the results");
                println!(
                    "Сейчас играет: {} — {}",
                    clean(&track.artist),
                    clean(&track.title)
                );
                let url = track.url.clone();
                found = Some(track);
                url
            };
            println!(
                "{}\nПробел: пауза; ←/→: перемотка; 9/0: громкость; q: выход.",
                clean(&url)
            );
            player::play(&cli.mpv, extractor, &url, found.as_ref(), cache.as_ref())?;
        }
        Action::Cache { clear } => {
            if let Some(cache) = cache.as_ref().filter(|_| clear) {
                cache.clear()?;
            }
            println!("{}", describe(cache.as_ref()));
        }
        Action::Doctor => {
            println!(
                "Прокси: {}",
                if proxy.is_some() {
                    "включён (аудио через yt-dlp)"
                } else if cli.no_proxy {
                    "принудительно отключён"
                } else {
                    "не задан в clicloud"
                }
            );
            println!("{}", describe(cache.as_ref()));
            let mut missing = false;
            for binary in [&cli.yt_dlp, &cli.mpv] {
                match version(binary) {
                    Ok(value) => println!("{}: {}", clean(binary), clean(&value)),
                    Err(error) => {
                        eprintln!("{}", clean(&error.to_string()));
                        missing = true;
                    }
                }
            }
            if missing {
                return Err("Установите отсутствующие программы; инструкции в README.md.".into());
            }
        }
    }
    Ok(())
}

fn version(binary: &str) -> Result<String> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|error| format!("Не удалось запустить {binary}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{binary} --version завершился с {}", output.status).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("OK")
        .to_owned())
}

fn print_tracks(tracks: &[Track]) {
    if tracks.is_empty() {
        println!("Ничего не найдено.");
    }
    for (index, track) in tracks.iter().enumerate() {
        let duration = track
            .duration
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| {
                let s = v as u64;
                format!("{}:{:02}", s / 60, s % 60)
            })
            .unwrap_or_else(|| "?:??".into());
        println!(
            "{:>2}. {} — {} [{}]\n    {}",
            index + 1,
            clean(&track.artist),
            clean(&track.title),
            duration,
            clean(&track.url)
        );
    }
}

fn choose(count: usize) -> Result<Option<usize>> {
    loop {
        print!("Номер трека (1–{count}, q — выход): ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 || input.trim().eq_ignore_ascii_case("q") {
            return Ok(None);
        }
        if let Ok(number) = input.trim().parse::<usize>()
            && (1..=count).contains(&number)
        {
            return Ok(Some(number - 1));
        }
        println!("Введите число от 1 до {count} или q.");
    }
}

// Remote metadata must not inject terminal control sequences.
fn clean(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

// Same for multi-line error text: line breaks are kept.
fn clean_lines(value: &str) -> String {
    value
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}
