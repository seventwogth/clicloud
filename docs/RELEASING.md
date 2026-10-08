# Preparing a release

1. Set the version in `Cargo.toml`, update `Cargo.lock`, and write
   `docs/releases/vVERSION.md`. Set the same version in the download commands of
   both READMEs. The READMEs and the notes are packed into the archives, so
   finish them before the archives are built.
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
   the version from Cargo and targets the workflow's exact commit. A published
   release is not overwritten: creating one for its tag again fails. A draft has
   no tag yet, so a second run adds another draft of the same name beside the
   first; delete a stale draft on the releases page before running again.
6. After publishing, set `pkgver` and the checksums in `packaging/aur/*/PKGBUILD`
   and write each `.SRCINFO` again with `makepkg --printsrcinfo > .SRCINFO`.

The release workflow does not publish a release automatically. A successful
offline CI run does not establish that SoundCloud is reachable from users' networks.

Run the preparation workflows from the CLI:

```sh
gh workflow run live.yml --ref main
gh workflow run release.yml --ref main -f draft=true
```

Verify downloaded archives on Linux with `sha256sum --check SHA256SUMS`,
on macOS with `shasum -a 256 --check SHA256SUMS`, or compare Windows
`Get-FileHash -Algorithm SHA256 .\clicloud-VERSION-TARGET.zip` with the manifest.
