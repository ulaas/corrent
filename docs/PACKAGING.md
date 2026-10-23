# Packaging and releases

Releases are built by `.github/workflows/release.yml` when the owner pushes a `v*` tag. The
workflow uploads all artifacts to a **draft** GitHub release; nothing is published automatically.

| Platform | Artifact | Built by |
|---|---|---|
| Windows | `Corrent-<v>-x86_64.msi` (per-user, no admin) | `scripts/package-windows.ps1 -Msi` (cargo-wix + WiX 3, config in `wix/main.wxs`) |
| Windows | `Corrent-<v>-windows-portable.zip` | `scripts/package-windows.ps1` |
| Windows | winget manifests | release workflow fills `packaging/windows/winget/*.yaml` |
| Linux | `Corrent-<v>-x86_64.AppImage` | `scripts/package-appimage.sh` (linuxdeploy) |
| Linux | `Corrent-<v>-linux-x86_64.tar.gz` | release workflow |
| Linux | Flatpak | `packaging/linux/flatpak/io.github.ulaas.Corrent.yml` (manual, see below) |
| macOS | `Corrent-<v>-macos.dmg` | `scripts/package-macos.sh` (cargo-bundle + hdiutil) |
| Server | `Corrent-Server-<v>-linux-x86_64.tar.gz`, `-darwin-arm64.tar.gz` | `scripts/package-server.sh` (correntd, corrent-mpvw, service files) |
| Server | `Corrent-Server-<v>-windows-x86_64.zip` | `scripts/package-windows.ps1` |
| Server | Docker image | `packaging/server/docker/Dockerfile` (not published by the workflow) |

Every package also carries **`corrent-mpvw`**, the libmpv worker the media library uses for
probing, thumbnails and transcoding (next to the main executable). libmpv itself is not bundled
(licensing and size): the app plays and probes when the system has it and says so otherwise.
On Windows, where no package manager provides it, the app offers to download it on request
(*Get mpv for playback*, also in Settings › Playback): the pinned build from
shinchiro/mpv-winbuild-cmake that `scripts/fetch-libmpv.ps1` uses, checked against its SHA-256
(the newest release with GitHub's digest when the pinned one has been pruned), unpacked with
Windows' own `tar.exe` into `corrent-lib\` next to `corrent.exe`, where the player and
`corrent-mpvw` look first (`crates/corrent-core/src/mpv_get.rs`). Corrent never redistributes it.
Keep the pin in `mpv_get.rs` in step with the script.
Server installation (systemd, launchd, Windows service, Docker) is in `docs/SERVER.md`.

## Windows

**MSI** (`wix/main.wxs`): installs to `%LOCALAPPDATA%\Programs\Corrent` without a UAC prompt.
The Start Menu shortcut carries the AppUserModelID `Corrent.Corrent`, so notifications show
Corrent's name and icon. The optional feature *Open magnet links and .torrent files with
Corrent* runs `corrent.exe --register` on install and `--unregister` on uninstall — the same code
as Settings › General, which remembers the previous handler (e.g. another client) and restores it.
User data in `%APPDATA%\Corrent` is kept on uninstall. Upgrades replace the previous version
(`MajorUpgrade`); the `UpgradeCode` must never change.

**Portable zip**: `corrent.exe` plus a `portable` marker file. While the marker exists, all data
lives in `corrent-data\` next to the exe. Nothing is written to the registry unless the user
turns on *Open with Corrent* in Settings (the notification identity is never registered).

**Symbols**: release builds keep line tables in `corrent.pdb`/`correntd.pdb`, which are never
shipped. Crash reports list frames as offsets into the exe (`corrent.exe+0x1a2b3c`) with its
link stamp; the workflow keeps the exes and PDBs as the `symbols-windows` artifact. Keep that
artifact for every published release (LTO builds aren't reproducible), and resolve a user's
report with `scripts/symbolize.ps1 -Report crash-….txt -Exe <artifact>\corrent.exe`.
`cargo run --release -p corrent-core --example symbols_check` checks that the release profile's
symbols resolve to file:line.

**winget**: after a release is published, submit `dist/winget/*.yaml` from the workflow's
`windows` artifact to `microsoft/winget-pkgs` (`wingetcreate submit` or a pull request).

## Linux

`packaging/linux/corrent.desktop` registers `x-scheme-handler/magnet` and
`application/x-bittorrent`; `io.github.ulaas.Corrent.metainfo.xml` is the AppStream data.
Inside an AppImage, Corrent registers the AppImage file itself (`$APPIMAGE`) as the handler.

**Flatpak**: generate offline cargo sources, then build:

```
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/linux/flatpak/cargo-sources.json
flatpak-builder --user --install build-dir packaging/linux/flatpak/io.github.ulaas.Corrent.yml
```

In the Flatpak the window's app id is the Flatpak ID and handler registration is left to the
exported desktop file. Downloads default to `~/Downloads/Corrent` (`--filesystem=xdg-download`);
other folders go through the file chooser portal.

## macOS

`cargo bundle --release` uses `[package.metadata.bundle]` in `Cargo.toml` (magnet URL scheme,
`.torrent` document type from `packaging/macos/Info.plist.ext`). `scripts/package-macos.sh` wraps
the app in a dmg. The app is universal (Apple silicon and Intel).
Signing and notarization need the owner's Developer ID: locally set `CODESIGN_IDENTITY` and
`NOTARY_PROFILE` (a `notarytool store-credentials` profile); in CI add the secrets
`MACOS_CERTIFICATE_P12` (base64), `MACOS_CERTIFICATE_PASSWORD`, `MACOS_CODESIGN_IDENTITY` and an
App Store Connect API key (`MACOS_NOTARY_KEY_P8`, `MACOS_NOTARY_KEY_ID`, `MACOS_NOTARY_ISSUER`).
Without them the dmg is unsigned and opens after right-click › Open.

## Checklist for a release

1. Bump `version` in `Cargo.toml` and the `<release>` in the metainfo; `cargo build` to update the lock file.
2. Run the gates (`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo deny check && cargo test`)
   and `scripts/release-hygiene.ps1` (no debug hooks or test code in the release binaries).
3. Commit, tag `vX.Y.Z`, push the tag (owner). The Release workflow's `ci-gate` job waits until CI has
   passed on the tagged commit before building, and fails if that CI run fails or is cancelled (re-run CI, then the
   release). A manual run can tick `skip_ci_gate` in an emergency.
4. Check the draft release's artifacts on each platform (`docs/PLATFORM-TESTING.md`), then publish.
5. Submit the winget manifests, and keep the `symbols-windows` artifact with the release.
