#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "clicloud-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self(root);
        fs::write(fixture.0.join("config.json"), "{}").unwrap();
        fixture.script(
            "yt-dlp",
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'test-yt-dlp\n'; exit 0; fi
printf '%s\n' "$@" > "$SEARCH_LOG"
pwd > "$SEARCH_LOG.cwd"
if [ "$FAIL_SEARCH" = "1" ]; then printf 'HTTP Error 429\n' >&2; exit 1; fi
printf '%s\n' "$SEARCH_RESPONSE"
if [ "$FAIL_SEARCH" = "late" ]; then exit 1; fi
"#,
        );
        fixture.script(
            "mpv",
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'test-mpv\n'; exit 0; fi
printf '%s\n' "$@" > "$PLAYER_LOG"
printf '%s\n' "${http_proxy-unset}" > "$PLAYER_PROXY"
for arg in "$@"; do last="$arg"; done
if [ "$last" = "-" ]; then cat > "$PLAYER_BYTES"; fi
if [ -f "$last" ]; then cat "$last" > "$PLAYER_BYTES"; fi
exit "${PLAYER_EXIT:-0}"
"#,
        );
        fixture
    }

    fn script(&self, name: &str, source: &str) {
        let path = self.0.join(name);
        fs::write(&path, source).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self) -> Command {
        self.run(env!("CARGO_BIN_EXE_clicloud"))
    }

    fn run(&self, binary: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(binary);
        command.env("CLICLOUD_YT_DLP", self.0.join("yt-dlp"))
            .env("CLICLOUD_CONFIG", self.0.join("config.json"))
            // No test may reach the cache in the home directory; most do without one.
            .env("XDG_CACHE_HOME", self.0.join("xdg-cache"))
            .env_remove("CLICLOUD_CACHE_DIR")
            .env("CLICLOUD_NO_CACHE", "1")
            .env_remove("CLICLOUD_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("HTTPS_PROXY")
            .env_remove("all_proxy")
            .env_remove("ALL_PROXY")
            .env("CLICLOUD_MPV", self.0.join("mpv"))
            .env("SEARCH_LOG", self.0.join("search.log"))
            .env("PLAYER_LOG", self.0.join("player.log"))
            .env("PLAYER_BYTES", self.0.join("player.bytes"))
            .env("PLAYER_PROXY", self.0.join("player.proxy"))
            .env("FAIL_SEARCH", "0")
            .env("PLAYER_EXIT", "0")
            .env("SEARCH_RESPONSE", r#"{"entries":[{"title":"Night","uploader":"Artist","duration":123,"webpage_url":"https://soundcloud.com/artist/night"}]}"#);
        command
    }

    // With the cache in the default place of the fixture's own home.
    fn cached(&self) -> Command {
        let mut command = self.command();
        command.env_remove("CLICLOUD_NO_CACHE");
        command
    }

    fn cache(&self) -> PathBuf {
        self.0.join("xdg-cache/clicloud")
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap()
    }
}

#[test]
fn played_track_is_stored_and_then_plays_without_the_downloader() {
    let fixture = Fixture::new();
    let stored = fixture.cache().join("audio/artist.night");
    let play = |url: &str| {
        let output = fixture
            .cached()
            .env("SEARCH_RESPONSE", "audio bytes")
            .args(["--tor", "play", url])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    play("https://soundcloud.com/artist/night");
    let downloader = fixture.read("search.log");
    assert!(downloader.contains("--proxy\nsocks5h://127.0.0.1:9050\n"));
    assert!(downloader.contains("--abort-on-unavailable-fragments\n"));
    assert!(downloader.contains(&format!(
        "--cache-dir\n{}\n",
        fixture.cache().join("yt-dlp").display()
    )));
    // The player was fed the very bytes that were stored.
    assert!(fixture.read("player.log").ends_with("--ytdl=no\n--\n-\n"));
    assert_eq!(fixture.read("player.bytes"), "audio bytes\n");
    assert_eq!(fs::read_to_string(&stored).unwrap(), "audio bytes\n");
    // Fragments are kept beside the download, and nothing of it is left afterwards.
    let directory = fixture.read("search.log.cwd");
    assert!(
        directory
            .trim()
            .starts_with(&*fixture.cache().join("partial").to_string_lossy())
    );
    assert_eq!(
        fs::read_dir(fixture.cache().join("partial"))
            .unwrap()
            .count(),
        0
    );

    for name in ["search.log", "player.bytes"] {
        fs::remove_file(fixture.0.join(name)).unwrap();
    }
    // Another spelling of the same link.
    play("https://www.soundcloud.com/artist/night?si=abc");
    assert!(!fixture.0.join("search.log").exists());
    assert!(
        fixture
            .read("player.log")
            .ends_with(&format!("--ytdl=no\n--\n{}\n", stored.display()))
    );
    assert_eq!(fixture.read("player.bytes"), "audio bytes\n");

    let report = |args: &[&str]| {
        let output = fixture.cached().args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    let listed = report(&["cache"]);
    assert!(
        listed.contains(&*fixture.cache().to_string_lossy()),
        "{listed}"
    );
    assert!(
        listed.contains("Треков в кеше: 1, 0.0 МБ из 1024.0 МБ"),
        "{listed}"
    );
    assert!(report(&["doctor"]).contains("Треков в кеше: 1"));
    assert!(report(&["cache", "--clear"]).contains("Треков в кеше: 0"));
    assert!(!stored.exists());
    assert!(report(&["--no-cache", "cache"]).contains("Кеш: отключён"));
}

#[test]
fn failed_or_interrupted_download_is_not_stored() {
    let fixture = Fixture::new();
    let audio = fixture.cache().join("audio");
    // yt-dlp gave up after a part of the audio.
    let output = fixture
        .cached()
        .env("FAIL_SEARCH", "late")
        .args(["play", "https://soundcloud.com/artist/night"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("yt-dlp завершился"), "{stderr}");
    assert!(!audio.join("artist.night").exists());

    // The player left while yt-dlp was still downloading.
    fixture.script(
        "yt-dlp",
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo mock; exit 0; fi
printf 'half'
echo $$ > "$SOURCE_PID"
exec sleep 30
"#,
    );
    fixture.script(
        "mpv",
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo mock; exit 0; fi
while [ ! -f "$SOURCE_PID" ]; do sleep 0.01; done
exit 0
"#,
    );
    let output = fixture
        .cached()
        .env("SOURCE_PID", fixture.0.join("source.pid"))
        .args(["play", "https://soundcloud.com/artist/night"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let pid = fixture.read("source.pid");
    let alive = Command::new("kill")
        .args(["-0", pid.trim()])
        .output()
        .unwrap();
    assert!(!alive.status.success());
    assert!(!audio.join("artist.night").exists());
    assert_eq!(
        fs::read_dir(fixture.cache().join("partial"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn cache_settings_flags_and_links_that_are_not_stored() {
    let fixture = Fixture::new();
    let elsewhere = fixture.0.join("elsewhere");
    fs::write(
        fixture.0.join("config.json"),
        format!(
            r#"{{"cache_dir":"{}","cache_limit_mb":0}}"#,
            elsewhere.display()
        ),
    )
    .unwrap();
    let play = |command: &mut Command, url: &str| {
        let output = command.args(["play", url]).output().unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    // A playlist is left to mpv, but yt-dlp still keeps its own cache.
    play(
        &mut fixture.cached(),
        "https://soundcloud.com/artist/sets/album",
    );
    let player = fixture.read("player.log");
    assert!(player.contains(&format!(
        "--ytdl-raw-options-append=cache-dir={}\n",
        elsewhere.join("yt-dlp").display()
    )));
    assert!(player.ends_with("--\nhttps://soundcloud.com/artist/sets/album\n"));
    assert!(!elsewhere.join("audio").exists() && !fixture.cache().exists());
    let listed = fixture.cached().arg("cache").output().unwrap();
    assert!(String::from_utf8_lossy(&listed.stdout).contains("без ограничения"));

    // The flag outranks the file; a relative path is taken from the current directory.
    play(
        fixture
            .cached()
            .current_dir(&fixture.0)
            .args(["--cache-dir", "relative"]),
        "https://soundcloud.com/artist/night",
    );
    assert!(fixture.0.join("relative/audio/artist.night").exists());
    assert!(!elsewhere.join("audio").exists());

    fs::write(fixture.0.join("config.json"), r#"{"cache_enabled":false}"#).unwrap();
    play(&mut fixture.cached(), "https://soundcloud.com/artist/night");
    let player = fixture.read("player.log");
    assert!(player.contains("--ytdl-raw-options-append=no-cache-dir=\n"));
    assert!(player.ends_with("--\nhttps://soundcloud.com/artist/night\n"));
    assert!(!fixture.cache().exists());

    // A directory with someone's files is never taken over.
    let music = fixture.0.join("music");
    fs::create_dir(&music).unwrap();
    fs::write(music.join("song.mp3"), "mine").unwrap();
    let refused = fixture
        .cached()
        .arg("--cache-dir")
        .arg(&music)
        .args(["cache", "--clear"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("не является кешем"));
    assert!(music.join("song.mp3").exists());
}

#[test]
fn tor_routes_audio_through_downloader_pipe() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["--tor", "play", "https://soundcloud.com/a/b"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let downloader = fs::read_to_string(fixture.0.join("search.log")).unwrap();
    assert!(downloader.contains("--proxy\nsocks5h://127.0.0.1:9050\n"));
    assert!(downloader.contains("--no-cache-dir\n"));
    assert!(downloader.contains("--downloader\nnative\n"));
    assert!(downloader.contains("--output\n-\n"));
    // Fragment files of the downloader must land in a private directory that is removed.
    let directory = fs::read_to_string(fixture.0.join("search.log.cwd")).unwrap();
    let directory = std::path::Path::new(directory.trim());
    assert!(directory.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert_ne!(directory, std::env::current_dir().unwrap());
    assert!(!directory.exists());
    let player = fs::read_to_string(fixture.0.join("player.log")).unwrap();
    assert!(player.contains("--ytdl=no\n"));
    assert!(!player.contains("soundcloud.com"));
    assert!(!fs::read(fixture.0.join("player.bytes")).unwrap().is_empty());
}

#[test]
fn proxy_config_environment_and_cli_precedence() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("config.json"),
        r#"{"proxy":"socks5h://127.0.0.1:9150"}"#,
    )
    .unwrap();
    for (args, expected) in [
        (vec!["search", "ambient"], "socks5h://127.0.0.1:9150"),
        (
            vec!["--proxy", "http://localhost:8080", "search", "ambient"],
            "http://localhost:8080",
        ),
        (vec!["--no-proxy", "search", "ambient"], ""),
    ] {
        let output = fixture.command().args(args).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let log = fs::read_to_string(fixture.0.join("search.log")).unwrap();
        assert!(log.contains(&format!("--proxy\n{expected}\n")), "{log}");
    }
    let output = fixture
        .command()
        .env("CLICLOUD_PROXY", "socks5://localhost:1080")
        .args(["search", "ambient"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        fs::read_to_string(fixture.0.join("search.log"))
            .unwrap()
            .contains("--proxy\nsocks5://localhost:1080\n")
    );
}

#[test]
fn proxy_download_failure_is_not_successful_playback() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("FAIL_SEARCH", "1")
        .env("PLAYER_EXIT", "2")
        .args(["--tor", "play", "https://soundcloud.com/a/b"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    // Real mpv also fails once its input ends; the downloader is still the cause.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("yt-dlp завершился"), "{stderr}");
    assert!(!stderr.contains("mpv завершился"), "{stderr}");
}

#[test]
fn rejects_invalid_proxy_without_exposing_password() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args([
            "--proxy",
            "ftp://user:secret@localhost:21",
            "search",
            "ambient",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
    assert!(!fixture.0.join("search.log").exists());
}

#[test]
fn disabled_setting_and_explicit_direct_playback() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("config.json"),
        r#"{"proxy_enabled":false,"proxy":"socks5h://127.0.0.1:9050"}"#,
    )
    .unwrap();
    let output = fixture
        .command()
        .args(["search", "ambient"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        !fs::read_to_string(fixture.0.join("search.log"))
            .unwrap()
            .contains("--proxy")
    );
    let output = fixture
        .command()
        .env("CLICLOUD_PROXY", "socks5h://127.0.0.1:9050")
        .args(["--no-proxy", "play", "https://soundcloud.com/a/b"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        fs::read_to_string(fixture.0.join("player.log"))
            .unwrap()
            .contains("--ytdl-raw-options-append=proxy=\n")
    );
}

#[test]
fn direct_playback_gives_mpv_the_proxy_that_yt_dlp_takes_from_the_environment() {
    let fixture = Fixture::new();
    let proxy = || fs::read_to_string(fixture.0.join("player.proxy")).unwrap();
    let play = |command: &mut Command| {
        let output = command
            .env("HTTPS_PROXY", "http://127.0.0.1:8118")
            .args(["play", "https://soundcloud.com/a/b"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    play(&mut fixture.command());
    assert_eq!(proxy(), "http://127.0.0.1:8118\n");
    play(fixture.command().arg("--no-proxy"));
    assert_eq!(proxy(), "unset\n");
}

#[test]
fn early_player_exit_reaps_downloader() {
    let fixture = Fixture::new();
    fixture.script(
        "yt-dlp",
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo mock; exit 0; fi
echo $$ > "$SOURCE_PID"
exec sleep 30
"#,
    );
    fixture.script(
        "mpv",
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo mock; exit 0; fi
while [ ! -f "$SOURCE_PID" ]; do sleep 0.01; done
exit 0
"#,
    );
    let output = fixture
        .command()
        .env("SOURCE_PID", fixture.0.join("source.pid"))
        .args(["--tor", "play", "https://soundcloud.com/a/b"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let pid = fs::read_to_string(fixture.0.join("source.pid")).unwrap();
    assert!(
        !Command::new("kill")
            .args(["-0", pid.trim()])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn termination_signal_stops_player_and_downloader() {
    let fixture = Fixture::new();
    fixture.script(
        "yt-dlp",
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo mock; exit 0; fi
echo $$ > "$SOURCE_PID"
exec sleep 30
"#,
    );
    // Like mpv, leaves on SIGTERM by itself; a SIGKILL would not write the marker.
    fixture.script(
        "mpv",
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo mock; exit 0; fi
trap 'echo restored > "$PLAYER_LOG"; exit 4' TERM
echo $$ > "$PLAYER_PID"
while :; do sleep 0.02; done
"#,
    );
    let pid = |name: &str| loop {
        match fs::read_to_string(fixture.0.join(name)) {
            Ok(pid) if pid.ends_with('\n') => break pid.trim().to_owned(),
            _ => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    };
    let alive = |pid: &str| {
        Command::new("kill")
            .args(["-0", pid])
            .output()
            .unwrap()
            .status
            .success()
    };
    let client = fixture
        .command()
        .env("SOURCE_PID", fixture.0.join("source.pid"))
        .env("PLAYER_PID", fixture.0.join("player.pid"))
        .args(["--tor", "play", "https://soundcloud.com/a/b"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (player, source) = (pid("player.pid"), pid("source.pid"));
    let killed = Command::new("kill")
        .args(["-TERM", &client.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let output = client.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(128 + 15), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Ошибка"));
    assert!(!alive(&player) && !alive(&source));
    assert_eq!(
        fs::read_to_string(fixture.0.join("player.log")).unwrap(),
        "restored\n"
    );
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn search_json_keeps_query_as_one_argument_and_stdout_machine_readable() {
    let fixture = Fixture::new();
    let query = "ambient; $(echo unexpected) ' mix";
    let output = fixture
        .command()
        .args(["search", query, "--limit", "3", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let tracks: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(tracks[0]["title"], "Night");
    let args = fs::read_to_string(fixture.0.join("search.log")).unwrap();
    assert_eq!(args.lines().last().unwrap(), format!("scsearch3:{query}"));
}

#[test]
fn play_first_passes_canonical_url_and_extractor_to_player() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["play", "night", "--first"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let args = fs::read_to_string(fixture.0.join("player.log")).unwrap();
    assert!(args.contains("--no-video\n"));
    assert!(args.contains(&format!(
        "--script-opts-append=ytdl_hook-ytdl_path={}\n",
        fixture.0.join("yt-dlp").display()
    )));
    assert!(args.contains("--script-opts-append=ytdl_hook-try_ytdl_first=yes\n"));
    assert!(args.contains("--network-timeout=15\n"));
    assert!(args.ends_with("--\nhttps://soundcloud.com/artist/night\n"));
}

#[test]
fn direct_link_skips_search_and_player_failure_propagates() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("PLAYER_EXIT", "2")
        .args(["play", "https://soundcloud.com/a/b"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!fixture.0.join("search.log").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mpv"));
}

#[test]
fn search_failure_reports_provider_error_without_starting_player() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("FAIL_SEARCH", "1")
        .args(["play", "night", "--first"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("429"));
    assert!(!fixture.0.join("player.log").exists());
}

#[test]
fn empty_search_and_malformed_json_do_not_start_player() {
    let fixture = Fixture::new();
    for response in [r#"{"entries":[]}"#, "broken json"] {
        let output = fixture
            .command()
            .env("SEARCH_RESPONSE", response)
            .args(["play", "night", "--first"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!fixture.0.join("player.log").exists());
    }
}

#[test]
fn doctor_detects_missing_dependency() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("CLICLOUD_MPV", fixture.0.join("missing-mpv"))
        .arg("doctor")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing-mpv"));
}

#[test]
fn invalid_input_fails_before_search() {
    let fixture = Fixture::new();
    for args in [
        vec!["search", "   "],
        vec!["search", "night", "--limit", "0"],
        vec!["play", "https://example.com/track"],
        vec!["play", "night"],
    ] {
        let output = fixture.command().args(args).output().unwrap();
        assert!(!output.status.success());
        assert!(!fixture.0.join("search.log").exists());
        assert!(!fixture.0.join("player.log").exists());
    }
}

// A freshly copied executable is briefly busy while another test thread forks.
fn output(command: &mut Command) -> std::process::Output {
    loop {
        match command.output() {
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => return result.unwrap(),
        }
    }
}

#[test]
fn discovers_extractor_of_own_project_but_not_of_current_directory() {
    let fixture = Fixture::new();
    let project = fixture.0.join("project");
    let elsewhere = fixture.0.join("elsewhere");
    for root in [&project, &elsewhere] {
        fs::create_dir_all(root.join(".tools/venv/bin")).unwrap();
    }
    fs::copy(
        fixture.0.join("yt-dlp"),
        project.join(".tools/venv/bin/yt-dlp"),
    )
    .unwrap();
    let planted = elsewhere.join(".tools/venv/bin/yt-dlp");
    fs::write(&planted, "#!/bin/sh\ntouch \"$PLANTED\"\n").unwrap();
    fs::set_permissions(&planted, fs::Permissions::from_mode(0o755)).unwrap();
    let built = project.join("target/debug/clicloud");
    let installed = elsewhere.join("clicloud");
    fs::create_dir_all(built.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_clicloud"), &built).unwrap();
    fs::hard_link(&built, &installed).unwrap();
    let command = |binary: &PathBuf| {
        let mut command = fixture.run(binary);
        command
            .current_dir(&elsewhere)
            .env_remove("CLICLOUD_YT_DLP")
            .env("PATH", "")
            .env("PLANTED", fixture.0.join("planted"));
        command
    };

    let found = output(command(&built).args(["search", "ambient", "--json"]));
    assert!(found.status.success(), "{found:?}");
    let explicit =
        output(command(&built).args(["--yt-dlp", "/missing/yt-dlp", "search", "ambient"]));
    assert!(!explicit.status.success());
    assert!(String::from_utf8_lossy(&explicit.stderr).contains("/missing/yt-dlp"));
    let outside = output(command(&installed).args(["search", "ambient"]));
    assert!(!outside.status.success());
    assert!(!fixture.0.join("planted").exists());
}

#[test]
fn proxy_search_has_longer_network_timeout() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["--tor", "search", "ambient"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        fs::read_to_string(fixture.0.join("search.log"))
            .unwrap()
            .contains("--socket-timeout\n45\n")
    );
    // A proxy from the environment is as slow as one configured here.
    for (flag, expected) in [(None, "45"), (Some("--no-proxy"), "15")] {
        let output = fixture
            .command()
            .env("HTTPS_PROXY", "http://127.0.0.1:8118")
            .args(flag)
            .args(["search", "ambient"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let log = fs::read_to_string(fixture.0.join("search.log")).unwrap();
        assert!(
            log.contains(&format!("--socket-timeout\n{expected}\n")),
            "{log}"
        );
    }
}
