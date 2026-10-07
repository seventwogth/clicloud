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
if [ "$FAIL_SEARCH" = "1" ]; then printf 'HTTP Error 429\n' >&2; exit 1; fi
printf '%s\n' "$SEARCH_RESPONSE"
"#,
        );
        fixture.script(
            "mpv",
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'test-mpv\n'; exit 0; fi
printf '%s\n' "$@" > "$PLAYER_LOG"
for arg in "$@"; do last="$arg"; done
if [ "$last" = "-" ]; then cat > "$PLAYER_BYTES"; fi
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
            .env_remove("CLICLOUD_PROXY")
            .env("CLICLOUD_MPV", self.0.join("mpv"))
            .env("SEARCH_LOG", self.0.join("search.log"))
            .env("PLAYER_LOG", self.0.join("player.log"))
            .env("PLAYER_BYTES", self.0.join("player.bytes"))
            .env("FAIL_SEARCH", "0")
            .env("PLAYER_EXIT", "0")
            .env("SEARCH_RESPONSE", r#"{"entries":[{"title":"Night","uploader":"Artist","duration":123,"webpage_url":"https://soundcloud.com/artist/night"}]}"#);
        command
    }
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
    assert!(downloader.contains("--downloader\nnative\n"));
    assert!(downloader.contains("--output\n-\n"));
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
}
