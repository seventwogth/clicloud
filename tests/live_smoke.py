"""Opt-in Unix smoke test using real SoundCloud, yt-dlp and mpv (silent decoding)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import tempfile


def run(command, env, timeout=150):
    process = subprocess.Popen(command, env=env, text=True, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True)
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
        raise RuntimeError(f"Live command timed out after {timeout}s") from None
    if process.returncode:
        raise RuntimeError(f"Live command exited {process.returncode}:\n{stdout[-3000:]}\n{stderr[-3000:]}")
    return stdout, stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/clicloud")
    parser.add_argument("--yt-dlp", default="yt-dlp")
    parser.add_argument("--mpv", default="mpv")
    parser.add_argument("--proxy", help="Explicit HTTP/SOCKS proxy; otherwise use a direct connection")
    parser.add_argument("--query", default="ambient")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("This test requires Unix process groups and shell wrappers")
    extractor = shutil.which(args.yt_dlp)
    player = shutil.which(args.mpv)
    if not extractor or not player:
        parser.error("Install yt-dlp and mpv or provide their paths")
    with tempfile.TemporaryDirectory(prefix="clicloud-live-") as folder:
        root = Path(folder)
        config = root / "config.json"
        config.write_text('{"language":"en","media_keys":"off"}')
        log = root / "mpv.log"
        wrapper = root / "mpv"
        wrapper.write_text(
            f"#!/bin/sh\nexec {shlex.quote(player)} --ao=null --ao-null-untimed "
            f"--log-file={shlex.quote(str(log))} \"$@\"\n"
        )
        wrapper.chmod(0o755)
        env = {key: value for key, value in os.environ.items() if not key.startswith("CLICLOUD_")}
        env.update(XDG_CACHE_HOME=str(root / "xdg-cache"), XDG_DATA_HOME=str(root / "data"))
        base = [str(Path(args.binary).resolve()), "--config", str(config),
                "--yt-dlp", extractor, "--mpv", str(wrapper), "--cache-dir", str(root / "cache")]
        route = ["--proxy", args.proxy] if args.proxy else ["--no-proxy"]
        stdout, _ = run(base + route + ["search", args.query, "--limit", "3", "--json"], env)
        tracks = json.loads(stdout)
        if not tracks:
            raise RuntimeError("SoundCloud search returned no tracks")
        track = min(tracks, key=lambda track: track.get("duration") or 999999)
        url = track["url"]
        print(f"PASS: live search ({len(tracks)} tracks)", flush=True)
        run(base + route + ["play", url], env)
        if "AO: [null]" not in log.read_text(errors="replace"):
            raise RuntimeError("mpv did not initialize real audio decoding")
        files = list((root / "cache" / "audio").iterdir())
        if not files or not all(file.stat().st_size > 0 for file in files):
            raise RuntimeError("Playback did not leave a complete cached track")
        before = {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in files}
        print("PASS: live download, mpv decoding and cache commit", flush=True)

        # The downloader may answer --version; any actual extraction is a failure.
        offline = root / "offline-yt-dlp"
        marker = root / "unexpected-download"
        offline.write_text(
            '#!/bin/sh\nif [ "$1" = "--version" ]; then echo offline-check; exit 0; fi\n'
            f"touch {shlex.quote(str(marker))}\nexit 99\n"
        )
        offline.chmod(0o755)
        cached = base.copy()
        cached[cached.index("--yt-dlp") + 1] = str(offline)
        _, stderr = run(cached + ["--proxy", "http://127.0.0.1:1", "play", url], env)
        if "Track from the cache" not in stderr or marker.exists():
            raise RuntimeError("Cached playback attempted extraction")
        after = {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in files}
        if before != after or "AO: [null]" not in log.read_text(errors="replace"):
            raise RuntimeError("Cached audio was changed or could not be decoded")
        print("PASS: offline cached playback with unavailable proxy and downloader", flush=True)

        run(base + route + ["--no-cache", "play", url], env)
        if "AO: [null]" not in log.read_text(errors="replace"):
            raise RuntimeError("Uncached streaming did not decode audio")
        print("PASS: uncached streaming through " + ("proxy" if args.proxy else "mpv's yt-dlp hook"),
              flush=True)


if __name__ == "__main__":
    main()
