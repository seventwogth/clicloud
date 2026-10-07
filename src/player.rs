use crate::{Result, soundcloud};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const PROXY_VARIABLES: [&str; 6] = [
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
];

/// mpv with the options shared by `play` and the TUI.
pub fn mpv(executable: &str) -> Command {
    let mut command = Command::new(executable);
    command.args(["--no-config", "--no-video", "--no-audio-display"]);
    command
}

/// Lets mpv fetch `url` itself; must be the last arguments of `command`.
pub fn from_url(command: &mut Command, yt_dlp: &str, url: &str, no_proxy: bool) {
    // mpv's built-in hook resolves streams and forwards their HTTP headers.
    // append passes one option without splitting commas in the executable path.
    command
        .args([
            "--ytdl=yes",
            "--ytdl-format=bestaudio/best",
            "--ytdl-raw-options=ignore-config=,no-cache-dir=,socket-timeout=15,retries=2",
        ])
        .arg(format!("--script-opts-append=ytdl_hook-ytdl_path={yt_dlp}"));
    if no_proxy {
        for name in PROXY_VARIABLES {
            command.env_remove(name);
        }
        command.arg("--ytdl-raw-options-append=proxy=");
    }
    command.arg("--").arg(url);
}

/// Feeds mpv the bytes of a `downloader`; must be the last arguments of `command`.
pub fn from_pipe(command: &mut Command, source: ChildStdout) {
    command.args(["--ytdl=no", "--", "-"]).stdin(source);
}

/// yt-dlp writing the audio of `url` to its stdout through `proxy`.
pub fn downloader(yt_dlp: &str, url: &str, proxy: &str, progress: bool) -> Command {
    let mut command = Command::new(yt_dlp);
    command.args([
        "--ignore-config",
        "--no-cache-dir",
        "--proxy",
        proxy,
        "--socket-timeout",
        "30",
        "--retries",
        "2",
        "--fragment-retries",
        "2",
        "--no-playlist",
        "--playlist-items",
        "1",
        "--downloader",
        "native",
        "--fixup",
        "never",
    ]);
    if !progress {
        command.arg("--no-progress");
    }
    command
        .args(["--format", "bestaudio/best", "--output", "-", "--", url])
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    command
}

/// Playback was cut short by this signal; the exit code follows the shell convention.
#[derive(Debug)]
pub struct Interrupted(pub i32);

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Воспроизведение прервано сигналом {}.", self.0)
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

fn run(player: &mut Command, signals: &Signals) -> Result<ExitStatus> {
    let mut child = player
        .spawn()
        .map_err(|error| format!("Не удалось запустить mpv: {error}"))?;
    loop {
        // Checked first: an interrupted mpv exits with an error status of its own.
        if let Some(signal) = signals.received() {
            stop(&mut child);
            return Err(Box::new(Interrupted(signal)));
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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

pub fn play(
    executable: &str,
    yt_dlp: &str,
    url: &str,
    proxy: Option<&str>,
    no_proxy: bool,
) -> Result<()> {
    soundcloud::validate_url(url)?;
    let signals = Signals::new()?;
    if let Some(proxy) = proxy {
        return play_through_proxy(executable, yt_dlp, url, proxy, &signals);
    }
    let mut command = mpv(executable);
    from_url(&mut command, yt_dlp, url, no_proxy);
    let status = run(&mut command, &signals)?;
    if !status.success() {
        return Err(format!(
            "mpv завершился с {status}. Трек может быть недоступен; подробности выше."
        )
        .into());
    }
    Ok(())
}

fn play_through_proxy(
    executable: &str,
    yt_dlp: &str,
    url: &str,
    proxy: &str,
    signals: &Signals,
) -> Result<()> {
    eprintln!("Прокси: аудио через yt-dlp; перемотка ограничена, воспроизводится один трек.");
    let mut source = downloader(yt_dlp, url, proxy, true)
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("Не удалось запустить yt-dlp: {error}"))?;
    let mut command = mpv(executable);
    from_pipe(&mut command, source.stdout.take().expect("piped stdout"));
    let played = run(&mut command, signals);
    // Always reap the downloader, including early player exit, signals or failed spawn.
    let finished = source.try_wait();
    if !matches!(finished, Ok(Some(_))) {
        let _ = source.kill();
    }
    let _ = source.wait();
    let status = played?;
    // A downloader that failed by itself is the cause; mpv then only ran out of input.
    if let Some(status) = finished?
        && !status.success()
    {
        return Err(
            format!("yt-dlp завершился с {status}; проверьте прокси и доступность трека.").into(),
        );
    }
    if !status.success() {
        return Err(format!("mpv завершился с {status}.").into());
    }
    Ok(())
}
