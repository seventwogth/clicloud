//! The programs clicloud cannot work without, and fetching the one that can be fetched.
//!
//! yt-dlp is one file per platform, published with its SHA-256, so it is fetched and
//! checked here and kept beside the library. mpv is a system package with libraries and
//! codecs of its own: a file dropped next to ours would miss them, go without updates
//! and fail for reasons we could not explain, so for mpv the user is told the one
//! command of their platform instead.
use crate::{Result, config};
use std::path::{Path, PathBuf};
use std::process::Command;

const RELEASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";

/// The file of the latest release that runs here, as the release names it.
pub fn asset() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "yt-dlp_macos"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "yt-dlp_linux_aarch64"
    } else if cfg!(target_os = "linux") {
        "yt-dlp_linux"
    } else {
        // Needs Python, and is the only one that runs anywhere else.
        "yt-dlp"
    }
}

pub fn url() -> String {
    format!("{RELEASE}/{}", asset())
}

/// What a fetched yt-dlp is called here, whatever the release called it.
pub fn name() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    }
}

/// Where a fetched program is kept: beside the library, never in the system.
pub fn directory() -> Option<PathBuf> {
    config::directory("XDG_DATA_HOME", ".local/share").map(|path| path.join("clicloud/bin"))
}

/// The one command that installs mpv where this runs, for the user to run themselves.
pub fn mpv() -> String {
    if cfg!(windows) || cfg!(target_os = "macos") {
        // Homebrew is not a given, and Windows has no build of its own at all.
        return if cfg!(windows) {
            "https://mpv.io/installation/".into()
        } else {
            "brew install mpv".into()
        };
    }
    // The package manager that is actually here knows how to word it.
    for (manager, command) in [
        ("pacman", "sudo pacman -S mpv"),
        ("apt", "sudo apt install mpv"),
        ("dnf", "sudo dnf install mpv"),
        ("zypper", "sudo zypper install mpv"),
        ("apk", "sudo apk add mpv"),
        ("xbps-install", "sudo xbps-install mpv"),
    ] {
        if crate::present(manager) {
            return command.into();
        }
    }
    "https://mpv.io/installation/".into()
}

/// Fetches yt-dlp of the latest release into `directory` and says where it landed.
///
/// What arrives is checked against the sum the release publishes before it is ever
/// run, and nothing is left behind if the two do not match.
pub fn install(proxy: Option<&str>, directory: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let sums = directory.join("SHA2-256SUMS");
    let listed = fetch(proxy, &format!("{RELEASE}/SHA2-256SUMS"), &sums)
        .and_then(|()| Ok(std::fs::read_to_string(&sums)?));
    let _ = std::fs::remove_file(&sums);
    let expected = published(&listed?, asset())
        .ok_or_else(|| t!("В релизе yt-dlp нет суммы для {}", asset()))?;
    let partial = directory.join(format!("{}.part", name()));
    let fetched = (|| -> Result<PathBuf> {
        fetch(proxy, &url(), &partial)?;
        let sum = sha256(&std::fs::read(&partial)?);
        if sum != expected {
            return Err(t!("Сумма не совпала: {} вместо {}", sum, expected).into());
        }
        let target = directory.join(name());
        std::fs::rename(&partial, &target)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(target)
    })();
    if fetched.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    fetched
}

/// Downloads `url` into `file` with curl, which every platform we run on carries.
fn fetch(proxy: Option<&str>, url: &str, file: &Path) -> Result<()> {
    let mut command = Command::new("curl");
    command.args([
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--max-time",
        "900",
    ]);
    if let Some(proxy) = proxy {
        command.args(["--proxy", proxy]);
    }
    let output = command
        .arg("--output")
        .arg(file)
        .arg(url)
        .output()
        .map_err(|error| t!("Не удалось запустить curl: {}", error))?;
    if !output.status.success() {
        return Err(t!(
            "curl завершился с {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(())
}

/// The sum that a `SHA2-256SUMS` file gives for the file called `asset`.
fn published(listed: &str, asset: &str) -> Option<String> {
    listed.lines().find_map(|line| {
        let (sum, name) = line.split_once(char::is_whitespace)?;
        let sum = sum.to_ascii_lowercase();
        (name.trim() == asset && sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(sum)
    })
}

// The first thirty-two bits of the fractional parts of the cube roots of the first
// sixty-four primes, as SHA-256 is defined with them.
#[rustfmt::skip]
const ROUNDS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The SHA-256 of `bytes` in the lower-case hexadecimal that the sums file uses.
///
/// Written out here so that the check does not depend on a program of the system and
/// on reading its words; the tests hold it to the sums that the standard publishes.
pub fn sha256(bytes: &[u8]) -> String {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = Vec::with_capacity(bytes.len() + 72);
    message.extend_from_slice(bytes);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&(bytes.len() as u64 * 8).to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        let mut words = [0u32; 64];
        for (word, four) in words.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*four);
        }
        for at in 16..64 {
            let (first, last) = (words[at - 15], words[at - 2]);
            let mixed = first.rotate_right(7) ^ first.rotate_right(18) ^ (first >> 3);
            let carried = last.rotate_right(17) ^ last.rotate_right(19) ^ (last >> 10);
            words[at] = words[at - 16]
                .wrapping_add(mixed)
                .wrapping_add(words[at - 7])
                .wrapping_add(carried);
        }
        let mut round = state;
        for (at, word) in words.iter().enumerate() {
            let [a, b, c, d, e, f, g, h] = round;
            let spread = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let chosen = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(spread)
                .wrapping_add(chosen)
                .wrapping_add(ROUNDS[at])
                .wrapping_add(*word);
            let mixed = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let most = (a & b) ^ (a & c) ^ (b & c);
            let second = mixed.wrapping_add(most);
            round = [
                first.wrapping_add(second),
                a,
                b,
                c,
                d.wrapping_add(first),
                e,
                f,
                g,
            ];
        }
        for (held, word) in state.iter_mut().zip(round) {
            *held = held.wrapping_add(word);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_what_the_standard_publishes_sums_for() {
        for (text, sum) in [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ] {
            assert_eq!(sha256(text.as_bytes()), sum, "{text:?}");
        }
        // A message of exactly one block, and one of more than two.
        assert_eq!(
            sha256(&[b'a'; 55]),
            "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318"
        );
        assert_eq!(sha256(&[b'a'; 1_000_000])[..16], *"cdc76e5c9914fb92");
    }

    #[test]
    fn reads_the_sum_of_the_file_that_runs_here() {
        let listed = "1fa6733c37ea6fb51c99ad8fe785e7b7e5f3246c9b980230329d4fb72ed8d4d6  yt-dlp\n\
            66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a  yt-dlp.exe\n\
            not-a-sum  yt-dlp_linux\n";
        assert_eq!(
            published(listed, "yt-dlp").unwrap(),
            "1fa6733c37ea6fb51c99ad8fe785e7b7e5f3246c9b980230329d4fb72ed8d4d6"
        );
        assert_eq!(
            published(listed, "yt-dlp.exe").unwrap(),
            "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a"
        );
        // A line that holds no sum names nothing, and neither does a file not listed.
        assert!(published(listed, "yt-dlp_linux").is_none());
        assert!(published(listed, "yt-dlp_macos").is_none());
        assert!(published("", "yt-dlp").is_none());
        // What runs here is named by the release, and kept under a plain name.
        assert!(asset().starts_with("yt-dlp"));
        assert!(url().ends_with(asset()));
        assert!(name() == "yt-dlp" || name() == "yt-dlp.exe");
    }
}
