"""PTY/IPC smoke test; no network, audio device or installed mpv needed.

Run after cargo build: python3 tests/tui_smoke.py
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def run(proxy=False, terminate=False):
    # Short path: the IPC socket below it must fit the Unix socket path limit on macOS.
    with tempfile.TemporaryDirectory(prefix="cc-", dir="/tmp") as directory:
        root = Path(directory)
        extractor = root / "yt-dlp"
        extractor.write_text('''#!/usr/bin/env python3
import json, os, sys, time
if "--output" in sys.argv:
    sys.stdout.buffer.write(b"mock-audio")
else:
    if os.path.exists(os.environ["SLOW_SEARCH"]):
        open(os.environ["SEARCH_PID"], "w").write(str(os.getpid()))
        time.sleep(30)
    print(json.dumps({"entries":[{"title":"Night radio","uploader":"Test artist","duration":235,"webpage_url":"https://soundcloud.com/test/night"}]}))
''')
        player = root / "mpv"
        player.write_text('''#!/usr/bin/env python3
import json, os, socket, sys, time
open(os.environ["MPV_PID"], "w").write(str(os.getpid()))
with open(os.environ["MPV_ARGS"], "a") as log:
    log.write(sys.argv[-1]+"\\n")
path = next(arg.split("=",1)[1] for arg in sys.argv if arg.startswith("--input-ipc-server="))
with socket.socket(socket.AF_UNIX) as server:
    server.bind(path)
    server.listen(1)
    conn, _ = server.accept()
    volume = 70
    with conn, conn.makefile("rb") as stream:
        conn.sendall(b'{"event":"file-loaded"}\\n')
        for line in stream:
            command = json.loads(line)["command"]
            with open(os.environ["IPC_LOG"], "a") as log:
                log.write(json.dumps(command)+"\\n")
            if command[0] == "observe_property":
                name = command[2]
                data = {"time-pos":12,"duration":235,"pause":False,"volume":volume}[name]
            elif command[:2] == ["cycle","pause"]:
                name, data = "pause", True
            elif command[:2] == ["add","volume"]:
                volume += command[2]
                name, data = "volume", volume
            else:
                continue
            conn.sendall((json.dumps({"event":"property-change","name":name,"data":data})+"\\n").encode())
if os.environ.get("MPV_LINGER"):
    time.sleep(30)
''')
        extractor.chmod(0o755)
        player.chmod(0o755)
        (root / "config.json").write_text("{}")
        temporary = root / "t"
        temporary.mkdir()
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
        before = termios.tcgetattr(slave)
        env = dict(os.environ, TERM="xterm-256color", CLICLOUD_YT_DLP=str(extractor),
                   CLICLOUD_MPV=str(player), CLICLOUD_CONFIG=str(root / "config.json"),
                   IPC_LOG=str(root / "ipc.jsonl"), MPV_PID=str(root / "mpv.pid"),
                   TMPDIR=str(temporary), SLOW_SEARCH=str(root / "slow"),
                   SEARCH_PID=str(root / "search.pid"), MPV_ARGS=str(root / "mpv.args"),
                   XDG_CACHE_HOME=str(root / "home-cache"))
        for name in ("CLICLOUD_CACHE_DIR", "CLICLOUD_NO_CACHE"):
            env.pop(name, None)
        cache = root / "cache"
        if terminate:
            env["MPV_LINGER"] = "1"
        env.pop("CLICLOUD_PROXY", None)
        process = subprocess.Popen(["target/debug/clicloud", "--tor" if proxy else "--no-proxy",
                                    "--cache-dir", str(cache),
                                    "ui", "--library", str(root / "library.json")],
                                   stdin=slave, stdout=slave, stderr=slave, env=env)
        output = bytearray()

        def read_until(check, seconds=5):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.1)[0]:
                    output.extend(os.read(master, 65536))
                if check():
                    return
            raise AssertionError(output.decode(errors="replace")[-4000:])

        def log():
            path = root / "ipc.jsonl"
            return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

        try:
            read_until(lambda: b"CLICLOUD" in output)
            # Esc kills a hanging search; the next one must then run normally.
            search_pid = root / "search.pid"
            (root / "slow").touch()
            os.write(master, b"/slow\r")
            read_until(lambda: search_pid.exists() and search_pid.read_text())
            os.write(master, b"\x1b")
            read_until(lambda: not alive(int(search_pid.read_text())))
            (root / "slow").unlink()
            os.write(master, b"/ambient\r")
            read_until(lambda: b"Night radio" in output)
            os.write(master, b"fa\r")
            read_until(lambda: len(log()) >= 4)
            if terminate:
                player_pid = int((root / "mpv.pid").read_text())
                process.send_signal(signal.SIGTERM)
                process.wait(timeout=5)
                assert process.returncode == 0, process.returncode
                assert termios.tcgetattr(slave) == before, "Terminal mode was not restored"
                assert not alive(player_pid), "mpv outlived the client"
                assert not list(temporary.iterdir()), "IPC directory was not removed"
                assert not list((cache / "partial").iterdir()), "Download was left behind"
                print("PASS: SIGTERM stops mpv, removes the IPC directory, restores the terminal")
                return
            os.write(master, b" ++\x1b[C")
            read_until(lambda: ["cycle", "pause"] in log() and ["seek", 10, "relative"] in log()
                       and log().count(["add", "volume", 5]) == 2)
            # The track was stored while it played; the next start needs no downloader.
            stored = cache / "audio" / "test.night"
            read_until(stored.exists)
            assert stored.read_bytes() == b"mock-audio"
            os.write(master, b"n")
            read_until(lambda: sum(c[0] == "observe_property" for c in log()) >= 8)
            assert (root / "mpv.args").read_text().split() == ["-", str(stored)]
            # The stored track is listed in the library and can be removed from the cache there.
            assert json.loads((cache / "tracks" / "test.night").read_text())["title"] == "Night radio"
            os.write(master, b"2x")
            read_until(lambda: not stored.exists())
            # A setting changed in the settings window lands in the settings file.
            os.write(master, b"ojjjjj\x1b[C")
            settings = root / "config.json"
            read_until(lambda: json.loads(settings.read_text() or "{}").get("search_limit") == 35)
            os.write(master, b"q")
            process.wait(timeout=5)
            assert process.returncode == 0
            assert termios.tcgetattr(slave) == before, "Terminal mode was not restored"
            saved = json.loads((root / "library.json").read_text())
            assert len(saved["favorites"]) == 1 and len(saved["recent"]) == 1
            assert not list(temporary.iterdir()), "IPC directory was not removed"
            assert not (root / "home-cache").exists(), "The cache directory flag was ignored"
            print("PASS: TUI search and its cancel, favorites, queue, IPC controls, cache, settings,",
                  "terminal cleanup; proxy=", proxy)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    run()
    run(proxy=True)
    run(terminate=True)
