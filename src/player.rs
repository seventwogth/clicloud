use crate::{Result, soundcloud};
use std::process::{ChildStdout, Command, Stdio};

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

pub fn play(
    executable: &str,
    yt_dlp: &str,
    url: &str,
    proxy: Option<&str>,
    no_proxy: bool,
) -> Result<()> {
    soundcloud::validate_url(url)?;
    if let Some(proxy) = proxy {
        return play_through_proxy(executable, yt_dlp, url, proxy);
    }
    let mut command = mpv(executable);
    from_url(&mut command, yt_dlp, url, no_proxy);
    let status = command
        .status()
        .map_err(|error| format!("Не удалось запустить mpv: {error}"))?;
    if !status.success() {
        return Err(format!(
            "mpv завершился с {status}. Трек может быть недоступен; подробности выше."
        )
        .into());
    }
    Ok(())
}

fn play_through_proxy(executable: &str, yt_dlp: &str, url: &str, proxy: &str) -> Result<()> {
    eprintln!("Прокси: аудио через yt-dlp; перемотка ограничена, воспроизводится один трек.");
    let mut source = downloader(yt_dlp, url, proxy, true)
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("Не удалось запустить yt-dlp: {error}"))?;
    let mut command = mpv(executable);
    from_pipe(&mut command, source.stdout.take().expect("piped stdout"));
    let played = command.status();
    // Always reap the downloader, including early player exit or failed spawn.
    let finished = source.try_wait();
    if !matches!(finished, Ok(Some(_))) {
        let _ = source.kill();
    }
    let _ = source.wait();
    let status = played.map_err(|error| format!("Не удалось запустить mpv: {error}"))?;
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
