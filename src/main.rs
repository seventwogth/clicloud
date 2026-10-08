#[macro_use]
mod lang;

mod cache;
mod config;
mod cover;
mod ipc;
mod library;
mod playback;
mod player;
mod setup;
mod soundcloud;
mod theme;
mod ui;

use cache::Cache;
use clap::{Parser, Subcommand};
use soundcloud::{Extractor, SoundCloud, Track};
use std::io::{self, IsTerminal, Write};
use std::process::{Command, ExitCode};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(version, about = "Search and play music in the terminal")]
struct Cli {
    /// Path to yt-dlp
    #[arg(long, global = true, env = "CLICLOUD_YT_DLP", default_value_t = default_yt_dlp())]
    yt_dlp: String,
    /// Path to mpv
    #[arg(long, global = true, env = "CLICLOUD_MPV", default_value = "mpv")]
    mpv: String,
    /// Settings JSON file (default: ~/.config/clicloud/config.json)
    #[arg(long, global = true, env = "CLICLOUD_CONFIG")]
    config: Option<std::path::PathBuf>,
    /// HTTP(S) or SOCKS5 proxy
    #[arg(long, global = true, env = "CLICLOUD_PROXY", hide_env_values = true)]
    proxy: Option<String>,
    /// Use local Tor: socks5h://127.0.0.1:9050
    #[arg(long, global = true, conflicts_with = "no_proxy")]
    tor: bool,
    /// Disable proxies, including environment settings
    #[arg(long, global = true)]
    no_proxy: bool,
    /// Cache directory (default: ~/.cache/clicloud)
    #[arg(long, global = true, env = "CLICLOUD_CACHE_DIR")]
    cache_dir: Option<std::path::PathBuf>,
    /// Disable the audio and yt-dlp disk cache
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
    /// Open the terminal interface (also the default command)
    Ui {
        /// Path to the library JSON file
        #[arg(long)]
        library: Option<std::path::PathBuf>,
    },
    /// Search for tracks
    Search {
        query: String,
        #[arg(short, long, default_value_t = 10, value_parser = clap::value_parser!(u8).range(1..=50))]
        limit: u8,
        /// Print JSON instead of a list
        #[arg(long)]
        json: bool,
    },
    /// Find and select a track, or play a SoundCloud URL
    Play {
        query_or_url: String,
        #[arg(short, long, default_value_t = 10, value_parser = clap::value_parser!(u8).range(1..=50))]
        limit: u8,
        /// Play the first result without prompting
        #[arg(long)]
        first: bool,
    },
    /// Import favorites from a SoundCloud profile or list URL
    Import {
        /// Profile (name, @name, soundcloud.com/name), playlist, reposts or artist tracks URL
        profile: String,
        /// Path to the library JSON file
        #[arg(long)]
        library: Option<std::path::PathBuf>,
        /// Show what would be added without writing anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Show cache usage or clear it
    Cache {
        /// Remove cached audio and yt-dlp data
        #[arg(long)]
        clear: bool,
    },
    /// Check external programs
    Doctor,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if let Some(player::Interrupted(signal)) = error.downcast_ref() {
                return ExitCode::from(128 + *signal as u8);
            }
            // Written without a panic where nobody is left to read it: a terminal
            // that was closed takes the place for errors along.
            let said = t!("Ошибка: {}", clean_lines(&error.to_string()));
            let _ = writeln!(io::stderr(), "{said}");
            ExitCode::FAILURE
        }
    }
}

fn default_yt_dlp() -> String {
    if present("yt-dlp") {
        return "yt-dlp".into();
    }
    if let Some(local) = local_yt_dlp() {
        return local.to_string_lossy().into_owned();
    }
    // One that the interface fetched before, where it keeps what it fetches.
    if let Some(fetched) = setup::directory().map(|directory| directory.join(setup::name()))
        && executable(&fetched)
    {
        return fetched.to_string_lossy().into_owned();
    }
    "yt-dlp".into()
}

/// Whether `program` can be started: a path that names a file we may run, or a bare
/// name that one of the directories of PATH holds. Windows wants the extension spelled.
fn present(program: &str) -> bool {
    let path = std::path::Path::new(program);
    if path.components().count() > 1 {
        return executable(path) || cfg!(windows) && executable(&path.with_extension("exe"));
    }
    let named = |directory: std::path::PathBuf| {
        #[cfg(windows)]
        let names = [
            program.to_owned(),
            format!("{program}.exe"),
            format!("{program}.cmd"),
            format!("{program}.bat"),
        ];
        #[cfg(not(windows))]
        let names = [program.to_owned()];
        names.iter().any(|name| executable(&directory.join(name)))
    };
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(named))
}

// yt-dlp of the project whose Cargo target directory holds this binary. The
// current directory is never searched: it may belong to someone else.
fn local_yt_dlp() -> Option<std::path::PathBuf> {
    let binary = std::env::current_exe().ok()?;
    let target = binary.parent()?.parent()?;
    if target.file_name()? != "target" {
        return None;
    }
    // A virtual environment of Windows keeps its programs in another place, under
    // another name; the one of this project is where its README puts it.
    let tools = target.parent()?.join(".tools").join("venv");
    #[cfg(windows)]
    let local = tools.join("Scripts").join("yt-dlp.exe");
    #[cfg(not(windows))]
    let local = tools.join("bin").join("yt-dlp");
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
    t!("{} МБ", format!("{:.1}", bytes as f64 / (1024.0 * 1024.0)))
}

fn describe(cache: Option<&Cache>) -> String {
    let Some(cache) = cache else {
        return t!("Кеш: отключён").into();
    };
    let (tracks, size) = cache.usage();
    let limit = match cache.limit() {
        0 => t!("без ограничения").into(),
        limit => t!("из {}", megabytes(limit)),
    };
    t!(
        "Кеш: {}\nТреков в кеше: {}, {} {}",
        cache.root().display(),
        tracks,
        megabytes(size),
        limit
    )
}

fn run(cli: Cli) -> Result<()> {
    let settings = config::load(cli.config.as_ref())?;
    lang::set(lang::Lang::parse(&settings.language));
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
        Action::Ui { library } => ui::run(
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
        )?,
        Action::Search { query, limit, json } => {
            // A link to a playlist or to a page of a profile is opened, not searched for.
            let mut tracks = Vec::new();
            provider.ask(&query, limit.into(), |track| tracks.push(track))?;
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
                    return Err(t!(
                        "Для выбора нужен терминал. Используйте --first или ссылку SoundCloud."
                    )
                    .into());
                }
                let tracks = provider.search(target, limit)?;
                if tracks.is_empty() {
                    return Err(t!("Треки не найдены. Попробуйте другой запрос.").into());
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
                    "{}",
                    t!(
                        "Сейчас играет: {} — {}",
                        clean(&track.artist),
                        clean(&track.title)
                    )
                );
                let url = track.url.clone();
                found = Some(track);
                url
            };
            println!(
                "{}\n{}",
                clean(&url),
                t!("Пробел: пауза; ←/→: перемотка; 9/0: громкость; q: выход.")
            );
            player::play(
                &cli.mpv,
                extractor,
                &url,
                found.as_ref(),
                cache.as_ref(),
                player::Sound {
                    plugin: player::media_keys(&settings.media_keys).as_deref(),
                    ..settings.sound()
                },
            )?;
        }
        Action::Import {
            profile,
            library,
            dry_run,
        } => {
            // Checked before the network is asked: a bad name, a damaged library. A link
            // to another list than the likes of a profile is taken as that list.
            let list = match soundcloud::profile(&profile) {
                Ok(_) => None,
                Err(error) => match soundcloud::page(&profile) {
                    Some((soundcloud::Page::Set | soundcloud::Page::Listing, link)) => Some(link),
                    _ => return Err(error),
                },
            };
            let path = library::path(library, "import")?;
            library::load(&path)?;
            let counted = io::stderr().is_terminal();
            let mut liked = Vec::new();
            let mut count = |track| {
                liked.push(track);
                if counted {
                    eprint!("\r{}", t!("Получено: {}", liked.len()));
                }
            };
            let listed = match &list {
                Some(link) => provider.open(link, None, &mut count),
                None => provider.likes(&profile, &mut count),
            };
            if counted && !liked.is_empty() {
                eprintln!();
            }
            // A list that broke off is kept as far as it came: asking again adds the rest.
            if liked.is_empty() {
                listed?;
                println!("{}", t!("В списке нет треков."));
                return Ok(());
            }
            // Read again: the list took its time, and the interface may have written since.
            let (before, merged, kept) = if dry_run {
                let mut kept = library::load(&path)?;
                let before = kept.favorites.len();
                let merged = kept.merge(liked);
                (before, merged, kept)
            } else {
                let ((before, merged), kept) = library::update(&path, |kept| {
                    let before = kept.favorites.len();
                    (before, kept.merge(liked))
                })?;
                (before, merged, kept)
            };
            if dry_run {
                print_tracks(&kept.favorites[before..]);
                println!(
                    "{}",
                    t!(
                        "Было бы добавлено: {}, уже в избранном: {}, пропущено: {}",
                        merged.added,
                        merged.known,
                        merged.skipped
                    )
                );
            } else {
                println!(
                    "{}\n{}",
                    t!(
                        "Добавлено в избранное: {}, уже было: {}, пропущено: {}",
                        merged.added,
                        merged.known,
                        merged.skipped
                    ),
                    t!("Библиотека: {}", path.display())
                );
            }
            listed?;
        }
        Action::Cache { clear } => {
            if let Some(cache) = cache.as_ref().filter(|_| clear) {
                cache.clear()?;
            }
            println!("{}", describe(cache.as_ref()));
        }
        Action::Doctor => {
            println!(
                "{}",
                t!(
                    "Прокси: {}",
                    if proxy.is_some() {
                        t!("включён (аудио через yt-dlp)")
                    } else if cli.no_proxy {
                        t!("принудительно отключён")
                    } else {
                        t!("не задан в clicloud")
                    }
                )
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
                return Err(
                    t!("Установите отсутствующие программы; инструкции в README.md.").into(),
                );
            }
        }
    }
    Ok(())
}

fn version(binary: &str) -> Result<String> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|error| t!("Не удалось запустить {}: {}", binary, error))?;
    if !output.status.success() {
        return Err(t!("{} --version завершился с {}", binary, output.status).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("OK")
        .to_owned())
}

fn print_tracks(tracks: &[Track]) {
    if tracks.is_empty() {
        println!("{}", t!("Ничего не найдено."));
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
        print!("{}", t!("Номер трека (1–{}, q — выход): ", count));
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
        println!("{}", t!("Введите число от 1 до {} или q.", count));
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
