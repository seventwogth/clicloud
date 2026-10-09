//! Keeping clicloud itself current.
//!
//! A release publishes one archive per platform and a `SHA256SUMS` listing all of
//! them. That one file answers both questions at once: its names carry the version
//! of the latest release, and its sums let an archive be checked before anything in
//! it is run. So a look costs a single small request, and the same file guards the
//! download that may follow it.
//!
//! Only a build that came from a release replaces itself. A build from a package
//! manager is that manager's to update: a binary that overwrote itself behind its
//! back would leave the package database describing a file that is no longer there.
//! `CLICLOUD_RELEASE` is set when the release workflow builds, and by nothing else.
use crate::{Result, setup};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const RELEASES: &str = "https://github.com/seventwogth/clicloud/releases";
/// Set by the release workflow, so a build of our own making knows it is one.
const RELEASED: Option<&str> = option_env!("CLICLOUD_RELEASE");
/// A look a day: a new release is not worth a request at every start.
pub const BETWEEN_LOOKS: u64 = 24 * 60 * 60;

/// A release, as `SHA256SUMS` describes the part of it that belongs here.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    /// The archive for this platform, as the release names it.
    pub asset: String,
    /// Its SHA-256, in lower case.
    pub sum: String,
}

/// Whether this build may replace itself: only one that came from a release does.
pub fn installable() -> bool {
    RELEASED.is_some() && target().is_some()
}

/// The platform this build runs on, named as the release names it; None where no
/// release is built, and such a build is updated however it was installed.
pub fn target() -> Option<&'static str> {
    Some(
        if cfg!(all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc"
        )) {
            "x86_64-pc-windows-msvc"
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            "x86_64-unknown-linux-gnu"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "aarch64-apple-darwin"
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            "x86_64-apple-darwin"
        } else {
            return None;
        },
    )
}

/// What the archive of this platform ends in.
fn extension() -> &'static str {
    if cfg!(windows) { "zip" } else { "tar.gz" }
}

/// What the binary is called inside the archive, and on disk.
fn name() -> &'static str {
    if cfg!(windows) {
        "clicloud.exe"
    } else {
        "clicloud"
    }
}

/// Seconds since the epoch, for noting when the last look happened.
pub fn now() -> u64 {
    (SystemTime::now().duration_since(UNIX_EPOCH))
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Whether `theirs` is a later version than `ours`. Both are counted part by part,
/// so 0.1.10 is read as later than 0.1.9 and not as earlier.
pub fn newer(ours: &str, theirs: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        (version.split('.'))
            .map(|part| part.trim().parse().unwrap_or(0))
            .collect()
    };
    let (ours, theirs) = (parts(ours), parts(theirs));
    for place in 0..ours.len().max(theirs.len()) {
        let part = |from: &[u64]| from.get(place).copied().unwrap_or(0);
        if part(&ours) != part(&theirs) {
            return part(&theirs) > part(&ours);
        }
    }
    false
}

// The release this platform's archive belongs to, read out of a `SHA256SUMS` file.
//
// The version comes out of a name the release wrote, and is used to build a URL and
// a path, so nothing but digits and dots is accepted as one.
fn listed(sums: &str, target: &str, extension: &str) -> Option<Release> {
    let tail = format!("-{target}.{extension}");
    sums.lines().find_map(|line| {
        let (sum, asset) = line.split_once(char::is_whitespace)?;
        let (sum, asset) = (sum.trim().to_ascii_lowercase(), asset.trim());
        let version = asset.strip_prefix("clicloud-")?.strip_suffix(&tail)?;
        (sum.len() == 64
            && sum.bytes().all(|byte| byte.is_ascii_hexdigit())
            && !version.is_empty()
            && (version.bytes()).all(|byte| byte.is_ascii_digit() || byte == b'.'))
        .then(|| Release {
            version: version.to_owned(),
            asset: asset.to_owned(),
            sum,
        })
    })
}

/// The latest release, as far as it concerns this platform.
pub fn latest(proxy: Option<&str>) -> Result<Release> {
    let target = target().ok_or_else(|| t!("Для этой платформы релизов нет."))?;
    let file = std::env::temp_dir().join(format!("clicloud-sums-{}", std::process::id()));
    let sums = setup::fetch(
        proxy,
        &format!("{RELEASES}/latest/download/SHA256SUMS"),
        &file,
        120,
    )
    .and_then(|()| Ok(fs::read_to_string(&file)?));
    let _ = fs::remove_file(&file);
    listed(&sums?, target, extension())
        .ok_or_else(|| t!("В релизе нет файла для {}", target).into())
}

/// Puts `release` in the place of the running binary.
///
/// The archive is checked against the sum the release published before anything in
/// it is unpacked, and the binary already running is kept until the new one is in
/// place, so a failure half way leaves the old one working.
pub fn install(proxy: Option<&str>, release: &Release) -> Result<()> {
    if !installable() {
        return Err(
            t!("Этот клиент поставлен не из релиза; обновите его так же, как ставили.").into(),
        );
    }
    let target = target().ok_or_else(|| t!("Для этой платформы релизов нет."))?;
    let current = std::env::current_exe()?;
    let beside = (current.parent())
        .ok_or_else(|| t!("Не удалось понять, где лежит clicloud."))?
        .to_owned();
    // The new binary is unpacked beside the old one: a rename within one directory
    // is the only step that must not fail half way.
    let work = beside.join(format!(".clicloud-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let installed = (|| -> Result<()> {
        let archive = work.join(&release.asset);
        setup::fetch(
            proxy,
            &format!("{RELEASES}/download/v{}/{}", release.version, release.asset),
            &archive,
            900,
        )?;
        let sum = setup::sha256(&fs::read(&archive)?);
        if sum != release.sum {
            return Err(t!("Сумма не совпала: {} вместо {}", sum, release.sum).into());
        }
        unpack(&archive, &work)?;
        let fresh = (work.join(format!("clicloud-{}-{target}", release.version))).join(name());
        if !fresh.is_file() {
            return Err(t!("В архиве релиза нет файла {}", name()).into());
        }
        swap(&fresh, &current)
    })();
    let _ = fs::remove_dir_all(&work);
    installed
}

// The tar to unpack with. Windows carries bsdtar in its own directory from 10
// onwards, and that reads a zip; the GNU tar that a shell such as Git Bash puts
// first on the path does not, so the one in the system is named outright.
fn tar() -> PathBuf {
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        let system = PathBuf::from(root).join("System32").join("tar.exe");
        if system.is_file() {
            return system;
        }
    }
    PathBuf::from("tar")
}

// Unpacks `archive` into `into`: a zip under Windows, a tar.gz everywhere else.
fn unpack(archive: &Path, into: &Path) -> Result<()> {
    let output = Command::new(tar())
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .map_err(|error| t!("Не удалось запустить tar: {}", error))?;
    if !output.status.success() {
        return Err(t!(
            "tar завершился с {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(())
}

// Puts `fresh` where `current` is, both of them in the same directory.
fn swap(fresh: &Path, current: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        // A running exe cannot be written over, but it can be moved aside, and what
        // is left of it goes when clicloud next starts.
        let aside = current.with_extension("old");
        let _ = fs::remove_file(&aside);
        fs::rename(current, &aside)?;
        if let Err(error) = fs::rename(fresh, current) {
            let _ = fs::rename(&aside, current);
            return Err(error.into());
        }
        let _ = fs::remove_file(&aside);
    }
    #[cfg(not(windows))]
    {
        // The running program holds the old file open by its inode, not by its name,
        // and goes on working while the name points at the new one.
        fs::rename(fresh, current)?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(current, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Removes what the last update of a running Windows binary could not remove itself.
pub fn tidy() {
    if cfg!(windows)
        && let Ok(current) = std::env::current_exe()
    {
        let _ = fs::remove_file(current.with_extension("old"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &str = concat!(
        "4cfd66a3fae92aa0fb0be90df6ff6780f3147955585d4e7bee33fb6167141cd3  clicloud-0.1.0-aarch64-apple-darwin.tar.gz\n",
        "047fb423cde3f6c2eab79f6f2ad3c73430a15ca90665e3baf28fd4861fc64c8b  clicloud-0.1.0-x86_64-apple-darwin.tar.gz\n",
        "4b853ce787e60f66386bc6b453df810ce62847b6aa317b1cdcd582807d4391af  clicloud-0.1.0-x86_64-pc-windows-msvc.zip\n",
        "23418663db767962587a43186384140ccff03674e63f322f444176e38c9bba5d  clicloud-0.1.0-x86_64-unknown-linux-gnu.tar.gz\n",
    );

    #[test]
    fn the_archive_of_this_platform_is_picked_out_of_the_sums() {
        let found = listed(SUMS, "x86_64-pc-windows-msvc", "zip").unwrap();
        assert_eq!(found.version, "0.1.0");
        assert_eq!(found.asset, "clicloud-0.1.0-x86_64-pc-windows-msvc.zip");
        assert_eq!(
            found.sum,
            "4b853ce787e60f66386bc6b453df810ce62847b6aa317b1cdcd582807d4391af"
        );
        // The archive of another platform is not ours, whatever it ends in.
        assert_eq!(
            listed(SUMS, "x86_64-unknown-linux-gnu", "tar.gz")
                .unwrap()
                .asset,
            "clicloud-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert!(listed(SUMS, "x86_64-pc-windows-msvc", "tar.gz").is_none());
        assert!(listed(SUMS, "riscv64gc-unknown-linux-gnu", "tar.gz").is_none());
        assert!(listed("", "x86_64-pc-windows-msvc", "zip").is_none());
    }

    #[test]
    fn a_name_that_is_not_a_plain_version_is_refused() {
        // The version is put into a URL and into a path, so a release that named an
        // archive after something else is no release of ours.
        let sum = "4b853ce787e60f66386bc6b453df810ce62847b6aa317b1cdcd582807d4391af";
        for version in ["../../etc", "0.1.0/..", "0.1.0 ", "", "0.1.0;rm"] {
            let line = format!("{sum}  clicloud-{version}-x86_64-pc-windows-msvc.zip\n");
            assert!(
                listed(&line, "x86_64-pc-windows-msvc", "zip").is_none(),
                "{version}"
            );
        }
        // A sum that is not one is refused as well.
        let line = "nope  clicloud-0.1.0-x86_64-pc-windows-msvc.zip\n";
        assert!(listed(line, "x86_64-pc-windows-msvc", "zip").is_none());
    }

    #[test]
    fn the_tar_that_is_picked_reads_what_the_release_publishes() {
        let picked = tar();
        // Under Windows the zip of a release needs bsdtar, which lives in the system
        // directory; the GNU tar of a shell on the path cannot read a zip at all.
        if cfg!(windows) {
            // Named outright, or plain `tar` only where the system has no bsdtar.
            let system = picked.parent().map(|at| at.ends_with("System32"));
            assert!(
                picked == Path::new("tar")
                    || (picked.file_name() == Some("tar.exe".as_ref()) && system == Some(true)),
                "{picked:?}"
            );
        } else {
            assert_eq!(picked, Path::new("tar"));
        }
        // Whatever was picked must at least run.
        let ran = Command::new(&picked).arg("--version").output();
        assert!(ran.is_ok_and(|out| out.status.success()), "{picked:?}");
    }

    #[test]
    fn versions_are_compared_part_by_part() {
        assert!(newer("0.1.0", "0.1.1"));
        assert!(newer("0.1.9", "0.1.10"));
        assert!(newer("0.9.0", "0.10.0"));
        assert!(newer("0.1.0", "1.0.0"));
        assert!(newer("0.1", "0.1.1"));
        // The same version, and an older one, are not newer.
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.1"));
        assert!(!newer("0.1.10", "0.1.9"));
        assert!(!newer("1.0.0", "0.9.9"));
        // Nothing that cannot be read counts for anything.
        assert!(!newer("0.1.0", ""));
        assert!(!newer("0.1.0", "junk"));
    }

    #[test]
    fn a_build_of_our_own_knows_it_came_from_a_release() {
        // Tests are not built by the release workflow, so this one never may.
        assert!(!installable());
        assert!(
            install(
                None,
                &Release {
                    version: "9.9.9".into(),
                    asset: "clicloud-9.9.9-x86_64-pc-windows-msvc.zip".into(),
                    sum: "0".repeat(64),
                }
            )
            .is_err()
        );
        // Nothing of an update is left lying about beside a binary that never ran one.
        tidy();
        let aside = std::env::current_exe().unwrap().with_extension("old");
        assert!(!aside.exists(), "{}", aside.display());
    }
}
