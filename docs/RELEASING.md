# Preparing a release

1. Set the version in `Cargo.toml`, update `Cargo.lock`, and write
   `docs/releases/vVERSION.md`.
2. Run CI on the exact commit: Linux, macOS (Apple Silicon and Intel), Windows,
   and Rust 1.88. CI includes the multiprocess library tests; Unix jobs also run
   CLI integration and PTY smoke tests.
3. Run the separate Live smoke workflow. It uses SoundCloud, yt-dlp and mpv,
   downloads audio into a temporary directory and decodes it with a null output.
   Also check audible playback and TUI controls on the supported desktop systems.
   For a deployment-specific proxy run `tests/live_smoke.py --proxy URL` locally.
4. Run Release candidate on that commit. It checks CI again and builds four native
   archives, each containing the binary, documentation and source commit, with
   SHA-256 checksums. Artifacts are available from the workflow run.
5. Enable the workflow's `draft` input to also create a draft GitHub Release.
   Inspect its archives, notes and checksums before publishing it. The draft uses
   the version from Cargo and targets the workflow's exact commit. Existing
   releases are not overwritten; an attempt to create one again fails.

The release workflow does not publish a release automatically. A successful
offline CI run does not establish that SoundCloud is reachable from users' networks.
