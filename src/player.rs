use crate::{
    Result,
    cache::{Cache, Partial},
    soundcloud::{self, Extractor, Track},
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROXY_VARIABLES: [&str; 6] = [
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
];

// The HTTP proxy that yt-dlp takes from the environment for HTTPS requests.
fn https_proxy(variable: impl Fn(&str) -> Option<String>) -> Option<String> {
    let proxy = ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
        .into_iter()
        .find_map(|name| variable(name).filter(|value| !value.is_empty()))?;
    match proxy.split_once("://") {
        None => Some(format!("http://{proxy}")),
        Some(("http", _)) => Some(proxy),
        // FFmpeg cannot use anything but a plain HTTP proxy.
        Some(_) => None,
    }
}

/// How mpv is to sound, as the settings say.
#[derive(Clone, Copy, Default)]
pub struct Sound<'a> {
    /// Whether tracks are brought to one loudness.
    pub normalize: bool,
    /// The device to play on, by the name mpv knows it by; None leaves it to mpv.
    pub device: Option<&'a str>,
    /// The plugin that lets the media keys of the desktop reach mpv, where it is used.
    pub plugin: Option<&'a Path>,
}

/// Where the plugin is that makes mpv known to the desktop as a player, so that its
/// media keys and its panel reach it: mpv-mpris, which the packages of Linux put in
/// one of a few places. `wanted` is the setting: `auto` looks there, `off` wants
/// none, anything else names the file.
pub fn media_keys(wanted: &str) -> Option<PathBuf> {
    let found = |path: PathBuf| path.is_file().then_some(path);
    match wanted {
        "off" | "none" | "" => None,
        "auto" => {
            let own = crate::config::directory("XDG_CONFIG_HOME", ".config")
                .map(|directory| directory.join("mpv/scripts/mpris.so"));
            [
                "/usr/lib/mpv-mpris/mpris.so",
                "/usr/lib64/mpv-mpris/mpris.so",
                "/usr/local/lib/mpv-mpris/mpris.so",
                "/usr/lib/x86_64-linux-gnu/mpv-mpris/mpris.so",
                "/usr/lib/aarch64-linux-gnu/mpv-mpris/mpris.so",
                "/etc/mpv/scripts/mpris.so",
            ]
            .into_iter()
            .map(PathBuf::from)
            .chain(own)
            .find_map(found)
        }
        file => found(PathBuf::from(file)),
    }
}

// One loudness for every track, by the measure that broadcasters use.
const LOUDNESS: &str = "--af=lavfi=[loudnorm=I=-16:TP=-1.5:LRA=11]";

/// mpv with the options shared by `play` and the TUI.
pub fn mpv(executable: &str, sound: Sound) -> Command {
    let mut command = Command::new(executable);
    command.args(["--no-config", "--no-video", "--no-audio-display"]);
    if sound.normalize {
        command.arg(LOUDNESS);
    }
    if let Some(device) = sound.device {
        command.arg(format!("--audio-device={device}"));
    }
    if let Some(plugin) = sound.plugin {
        let mut script = std::ffi::OsString::from("--script=");
        script.push(plugin);
        command.arg(script);
    }
    command
}

/// The devices mpv can play on: the name it knows each by, and what to call it.
pub fn devices(executable: &str) -> Vec<(String, String)> {
    let listed = Command::new(executable)
        .args(["--no-config", "--audio-device=help"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match listed {
        Ok(output) => listing(&String::from_utf8_lossy(&output.stdout)),
        Err(_) => Vec::new(),
    }
}

// What `--audio-device=help` prints: a line for each device, `'name' (what it is)`.
fn listing(text: &str) -> Vec<(String, String)> {
    (text.lines())
        .filter_map(|line| {
            let (name, rest) = line.trim().strip_prefix('\'')?.split_once('\'')?;
            let rest = rest.trim();
            let described = (rest.strip_prefix('('))
                .and_then(|rest| rest.strip_suffix(')'))
                .unwrap_or(rest);
            Some((name.to_owned(), described.to_owned()))
        })
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

/// Lets mpv fetch `url` itself; must be the last arguments of `command`.
pub fn from_url(command: &mut Command, extractor: Extractor, url: &str) {
    // mpv's built-in hook resolves streams and forwards their HTTP headers.
    // append passes one option without splitting commas in a path.
    let mut cache = std::ffi::OsString::from("--ytdl-raw-options-append=");
    match extractor.cache {
        Some(directory) => {
            cache.push("cache-dir=");
            cache.push(directory);
        }
        None => cache.push("no-cache-dir="),
    }
    command
        .args([
            "--ytdl=yes",
            "--ytdl-format=bestaudio/best",
            // Same limit as yt-dlp below; mpv's default stalls for a minute.
            "--network-timeout=15",
            "--ytdl-raw-options=ignore-config=,socket-timeout=15,retries=2",
        ])
        .arg(cache)
        .arg(format!(
            "--script-opts-append=ytdl_hook-ytdl_path={}",
            extractor.program
        ))
        // Otherwise mpv first opens the track page itself and only then asks yt-dlp;
        // where soundcloud.com is unreachable that attempt hangs for a minute.
        .arg("--script-opts-append=ytdl_hook-try_ytdl_first=yes");
    if extractor.no_proxy {
        for name in PROXY_VARIABLES {
            command.env_remove(name);
        }
        command.arg("--ytdl-raw-options-append=proxy=");
    } else if let Some(proxy) = https_proxy(|name| std::env::var(name).ok()) {
        // FFmpeg inside mpv only reads the lowercase http_proxy; without it the stream
        // would bypass the proxy through which yt-dlp resolved it.
        command.env("http_proxy", proxy);
    }
    command.arg("--").arg(url);
}

/// Feeds mpv the bytes of a `downloader`; must be the last arguments of `command`.
pub fn from_pipe(command: &mut Command, source: impl Into<Stdio>) {
    command.args(["--ytdl=no", "--", "-"]).stdin(source);
}

/// Plays audio stored in the cache; must be the last arguments of `command`.
pub fn from_file(command: &mut Command, file: &Path) {
    command.args(["--ytdl=no", "--"]).arg(file);
}

/// A private directory for a player's log, its socket where it has one, and the
/// fragments of a downloader.
pub fn scratch_directory() -> Result<PathBuf> {
    let directory = std::env::temp_dir().join(format!(
        "clicloud-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    crate::cache::private_directory(&directory)?;
    Ok(directory)
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// yt-dlp writing the audio of `url` to its stdout.
///
/// It keeps the fragment being downloaded in a file in its working directory and
/// leaves it behind when stopped, so it runs in `directory`, which the caller removes.
/// `whole` is for audio that is kept: a lost fragment is then an error, not a gap.
pub fn downloader(
    extractor: Extractor,
    url: &str,
    directory: &Path,
    progress: bool,
    whole: bool,
) -> Command {
    let mut command = extractor.command();
    command.current_dir(directory).args([
        "--socket-timeout",
        if extractor.proxied() { "30" } else { "15" },
        "--retries",
        "2",
        "--fragment-retries",
        if whole { "5" } else { "2" },
        "--no-playlist",
        "--playlist-items",
        "1",
        "--downloader",
        "native",
        "--fixup",
        "never",
    ]);
    if whole {
        command.arg("--abort-on-unavailable-fragments");
    }
    if !progress {
        command.arg("--no-progress");
    }
    command
        // What it learns of the track on the way is noted beside the audio.
        .args(crate::soundcloud::Details::ARGUMENTS)
        .args(["--format", "bestaudio/best", "--output", "-", "--", url])
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    command
}

/// yt-dlp fetching a track into the cache. A player can be fed what has arrived so
/// far, so the track is heard while it is stored and is downloaded only once.
pub struct Download {
    child: Child,
    partial: Option<Partial>,
    source: Option<std::fs::File>,
    status: Option<ExitStatus>,
    over: Arc<AtomicBool>,
}

impl Download {
    /// Starts `command`, a `downloader` running in the directory of `partial`.
    pub fn start(mut command: Command, partial: Partial) -> Result<Self> {
        let file = std::fs::File::create(partial.path())?;
        let source = std::fs::File::open(partial.path())?;
        let child = command
            .stdout(file)
            .spawn()
            .map_err(|error| t!("Не удалось запустить yt-dlp: {}", error))?;
        Ok(Self {
            child,
            partial: Some(partial),
            source: Some(source),
            status: None,
            over: Default::default(),
        })
    }

    /// Copies the audio into `sink` as it arrives and closes it when the download is over.
    ///
    /// The download itself does not wait for the player, which reads at its own pace.
    pub fn feed(&mut self, mut sink: ChildStdin) {
        let Some(mut source) = self.source.take() else {
            return;
        };
        let over = self.over.clone();
        std::thread::spawn(move || {
            let mut buffer = vec![0; 64 * 1024];
            loop {
                // Read before the file: what was written before the end is then not missed.
                let last = over.load(Ordering::Acquire);
                match source.read(&mut buffer) {
                    Ok(0) if !last => std::thread::sleep(Duration::from_millis(50)),
                    // A player that is gone closes its end, which fails the write.
                    Ok(count) if count > 0 && sink.write_all(&buffer[..count]).is_ok() => (),
                    _ => break,
                }
            }
        });
    }

    /// The exit status of yt-dlp once it has one. A complete download joins the cache.
    pub fn poll(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if self.status.is_none()
            && let Some(status) = self.child.try_wait()?
        {
            self.status = Some(status);
            if status.success()
                && let Some(partial) = self.partial.take()
            {
                // The player goes on reading the open file even if it could not be kept.
                let _ = partial.commit();
            }
            self.over.store(true, Ordering::Release);
        }
        Ok(self.status)
    }

    pub fn finished(&self) -> bool {
        self.status.is_some()
    }

    /// The directory yt-dlp runs in, until the download has joined the cache.
    pub fn directory(&self) -> Option<&Path> {
        self.partial.as_ref().map(Partial::directory)
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.over.store(true, Ordering::Release);
    }
}

/// Playback was cut short by this signal; the exit code follows the shell convention.
#[derive(Debug)]
pub struct Interrupted(pub i32);

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", t!("Воспроизведение прервано сигналом {}.", self.0))
    }
}

impl std::error::Error for Interrupted {}

/// SIGTERM, SIGHUP and SIGINT caught while a player runs, so it is not left behind.
#[cfg(unix)]
struct Signals {
    received: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    handlers: Vec<signal_hook::SigId>,
}

#[cfg(unix)]
impl Signals {
    fn new() -> std::io::Result<Self> {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        let mut signals = Self {
            received: Default::default(),
            handlers: Vec::new(),
        };
        for signal in [SIGTERM, SIGHUP, SIGINT] {
            signals.handlers.push(signal_hook::flag::register_usize(
                signal,
                signals.received.clone(),
                signal as usize,
            )?);
        }
        Ok(signals)
    }

    fn received(&self) -> Option<i32> {
        match self.received.load(std::sync::atomic::Ordering::Relaxed) {
            0 => None,
            signal => Some(signal as i32),
        }
    }
}

#[cfg(unix)]
impl Drop for Signals {
    fn drop(&mut self) {
        for handler in self.handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
    }
}

#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    fn received(&self) -> Option<i32> {
        None
    }
}

fn spawn(player: &mut Command) -> Result<Child> {
    Ok(player
        .spawn()
        .map_err(|error| t!("Не удалось запустить mpv: {}", error))?)
}

// `tick` runs about twenty times a second while the player does.
fn wait(child: &mut Child, signals: &Signals, mut tick: impl FnMut()) -> Result<ExitStatus> {
    loop {
        // Checked first: an interrupted mpv exits with an error status of its own.
        if let Some(signal) = signals.received() {
            stop(child);
            return Err(Box::new(Interrupted(signal)));
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        tick();
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn run(player: &mut Command, signals: &Signals) -> Result<ExitStatus> {
    wait(&mut spawn(player)?, signals, || ())
}

// mpv owns the terminal and restores it on SIGTERM; SIGKILL is only the fallback.
fn stop(child: &mut Child) {
    #[cfg(unix)]
    let _ = rustix::process::kill_process(
        rustix::process::Pid::from_child(child),
        rustix::process::Signal::TERM,
    );
    #[cfg(unix)]
    let deadline = Instant::now() + Duration::from_secs(2);
    #[cfg(not(unix))]
    let deadline = Instant::now();
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// `track` is what a search told about `url`; the cache keeps it for its list.
pub fn play(
    executable: &str,
    extractor: Extractor,
    url: &str,
    track: Option<&Track>,
    cache: Option<&Cache>,
    sound: Sound,
) -> Result<()> {
    soundcloud::validate_url(url)?;
    let signals = Signals::new()?;
    let mut command = mpv(executable, sound);
    if let Some(cache) = cache {
        if let Some(file) = cache.find(url) {
            if let Some(track) = track {
                cache.describe(track);
            }
            eprintln!("{}", t!("Трек из кеша."));
            from_file(&mut command, &file);
            let status = run(&mut command, &signals)?;
            if !status.success() {
                return Err(t!("mpv завершился с {}.", status).into());
            }
            return Ok(());
        }
        if let Some(mut partial) = cache.store(url)? {
            if let Some(track) = track {
                partial.describe(track);
            }
            eprintln!(
                "{}",
                t!("Трек загружается в кеш; перемотка - в пределах загруженного.")
            );
            let mut source = downloader(extractor, url, partial.directory(), true, true);
            source.stderr(Stdio::inherit());
            let mut download = Download::start(source, partial)?;
            from_pipe(&mut command, Stdio::piped());
            let mut child = spawn(&mut command)?;
            download.feed(child.stdin.take().expect("piped stdin"));
            let played = wait(&mut child, &signals, || {
                let _ = download.poll();
            });
            // Dropping it then stops a download that the player did not wait for.
            let finished = download.poll();
            drop(download);
            return outcome(played, finished);
        }
    }
    if extractor.proxy.is_some() {
        return play_through_proxy(executable, extractor, url, &signals, sound);
    }
    from_url(&mut command, extractor, url);
    let status = run(&mut command, &signals)?;
    if !status.success() {
        return Err(t!(
            "mpv завершился с {}. Трек может быть недоступен; подробности выше.",
            status
        )
        .into());
    }
    Ok(())
}

fn play_through_proxy(
    executable: &str,
    extractor: Extractor,
    url: &str,
    signals: &Signals,
    sound: Sound,
) -> Result<()> {
    eprintln!(
        "{}",
        t!("Прокси: аудио через yt-dlp; перемотка ограничена, воспроизводится один трек.")
    );
    let scratch = Scratch(scratch_directory()?);
    let mut source = downloader(extractor, url, &scratch.0, true, false)
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| t!("Не удалось запустить yt-dlp: {}", error))?;
    let mut command = mpv(executable, sound);
    from_pipe(&mut command, source.stdout.take().expect("piped stdout"));
    let played = run(&mut command, signals);
    // Always reap the downloader, including early player exit, signals or failed spawn.
    let finished = source.try_wait();
    if !matches!(finished, Ok(Some(_))) {
        let _ = source.kill();
    }
    let _ = source.wait();
    outcome(played, finished)
}

// `finished` is the status of a downloader that ended by itself.
fn outcome(
    played: Result<ExitStatus>,
    finished: std::io::Result<Option<ExitStatus>>,
) -> Result<()> {
    let status = played?;
    // A downloader that failed by itself is the cause; mpv then only ran out of input.
    if let Some(status) = finished?
        && !status.success()
    {
        return Err(t!(
            "yt-dlp завершился с {}; проверьте сеть, прокси и доступность трека.",
            status
        )
        .into());
    }
    if !status.success() {
        return Err(t!("mpv завершился с {}.", status).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_devices_that_mpv_lists() {
        let text = "List of detected audio devices:\n  'auto' (Autoselect device)\n  \
            'pipewire/alsa_output.pci-0000_06_00.6' (Ryzen HD Audio (Pro))\n  'alsa' ()\n\nnoise\n";
        assert_eq!(
            listing(text),
            [
                ("auto".to_owned(), "Autoselect device".to_owned()),
                (
                    "pipewire/alsa_output.pci-0000_06_00.6".to_owned(),
                    "Ryzen HD Audio (Pro)".to_owned()
                ),
                ("alsa".to_owned(), String::new()),
            ]
        );
        assert!(listing("mpv: unknown option").is_empty());
        // A program that cannot be started lists nothing.
        assert!(devices("clicloud-no-such-program").is_empty());
        let loud = mpv(
            "mpv",
            Sound {
                normalize: true,
                device: Some("alsa"),
                plugin: Some(Path::new("/lib/mpris.so")),
            },
        );
        let args: Vec<_> = loud
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args[3..],
            [LOUDNESS, "--audio-device=alsa", "--script=/lib/mpris.so"]
        );
        // The plugin for the media keys: none where it is not wanted or not there.
        let here = std::env::current_exe().unwrap();
        assert_eq!(media_keys(&here.to_string_lossy()), Some(here));
        for wanted in ["off", "none", "", "/clicloud/no/such/mpris.so"] {
            assert!(media_keys(wanted).is_none(), "{wanted}");
        }
        assert_eq!(mpv("mpv", Sound::default()).get_args().count(), 3);
    }

    #[test]
    fn picks_the_http_proxy_that_yt_dlp_would_use_for_https() {
        let environment = |pairs: &'static [(&str, &str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        for (pairs, expected) in [
            (&[][..], None),
            (
                &[("HTTPS_PROXY", "http://127.0.0.1:8118")][..],
                Some("http://127.0.0.1:8118"),
            ),
            (
                &[
                    ("https_proxy", "localhost:3128"),
                    ("HTTPS_PROXY", "http://other:1"),
                ][..],
                Some("http://localhost:3128"),
            ),
            (
                &[("https_proxy", ""), ("ALL_PROXY", "http://all:8080")][..],
                Some("http://all:8080"),
            ),
            (&[("ALL_PROXY", "socks5h://127.0.0.1:9050")][..], None),
            (&[("http_proxy", "http://plain-http-only:1")][..], None),
        ] {
            assert_eq!(
                https_proxy(environment(pairs)).as_deref(),
                expected,
                "{pairs:?}"
            );
        }
    }
}
