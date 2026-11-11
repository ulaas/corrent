# Packaging and releases

Releases are built by `.github/workflows/release.yml` when the owner pushes a `v*` tag. The
workflow uploads all artifacts to a **draft** GitHub release; nothing is published automatically.

| Platform | Artifact | Built by |
|---|---|---|
| Windows | `Corrent-<v>-x86_64.msi` (per-user, no admin) | `scripts/package-windows.ps1 -Msi` (cargo-wix + WiX 3, config in `wix/main.wxs`) |
| Windows | `Corrent-<v>-windows-portable.zip` | `scripts/package-windows.ps1` |
| Windows | winget manifests | release workflow fills `packaging/windows/winget/*.yaml` |
| Windows | Scoop manifest | `packaging/windows/scoop/corrent.json` (portable zip) |
| macOS | Homebrew cask | `packaging/macos/homebrew/corrent.rb` (dmg) |
| Linux | AUR `corrent-bin` | `packaging/linux/aur/PKGBUILD` (tarball) |
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

## Updates in place

Once a day (Settings › Behavior › Updates, on by default) the app asks
`https://api.github.com/repos/ulaas/corrent/releases/latest` for the latest release: one
anonymous HTTPS request through the user's proxy if one is set, with `Corrent/<version>` as the
User-Agent and nothing that identifies the user or the profile. Drafts and pre-releases are
ignored. A newer release is announced (a toast, About, the Updates card); **nothing is downloaded
until the user clicks**. The click downloads one file, checks it, installs it and restarts
(`crates/corrent-core/src/self_update.rs`, `src/integration/updater.rs`, `src/app/update_ui.rs`).

**What is checked.** Only addresses under `https://github.com/ulaas/corrent/releases/download/`
are fetched, over HTTPS (redirects included), at most 600 MiB, and the size must be the one the
release states. The file is installed only if its SHA-256 equals the entry in the release's
`SHA256SUMS.txt` *and* the `digest` GitHub's API reports for the asset. Every source the release
has must agree and there must be at least one; a list that is there but doesn't name the file, or
can't be fetched, stops the update. Releases are not signed: this guards against a damaged or
swapped download, not against someone who can publish a release.

**Which file, and how it goes in** (by exact name; the kind is detected, never guessed):

| This copy | Detected by | Release file | How |
|---|---|---|---|
| Windows portable | the `portable` marker and `corrent-mpvw.exe` beside `corrent.exe`, folder writable | `Corrent-<v>-windows-portable.zip` | `Corrent/corrent.exe`, `corrent-mpvw.exe` and `LICENSE.txt` are unpacked into `.corrent-update-<n>\` beside them. The running files are renamed to `*.old` (a running program can be renamed, not overwritten), the new ones take their names, the new `corrent.exe` is started. If any step fails, or the new program doesn't start, every file is put back. `corrent-data`, `corrent-lib` and the marker are never touched. The next start deletes `*.old`. |
| Windows MSI | `HKCU\Software\Corrent\Installer` `InstallDir` is the program's folder | `Corrent-<v>-x86_64.msi` | Downloaded into `<profile>\update\`. The app copies itself there as `corrent-update.exe`, starts it with `--run-installer` and quits. The copy waits until the app is gone, checks the MSI's SHA-256 once more, runs `msiexec /i <msi> /passive /norestart` (progress window, no questions, no UAC: the MSI is per-user) and starts the installed `corrent.exe`. Exit codes 0, 3010 and 1641 count as installed; anything else shows a message and starts the version that is there (the installer rolls a failed upgrade back). |
| AppImage | `$APPIMAGE` names a file in a writable folder | `Corrent-<v>-x86_64.AppImage` | Downloaded beside it, made executable, the old file is linked to `*.old`, the new one is renamed over it (one step), then started. |
| Linux tarball | `corrent` and `corrent-mpvw` in a writable folder outside `/usr`, `/opt`… | `Corrent-<v>-linux-x86_64.tar.gz` | The two programs are unpacked and swapped like the portable zip (old files linked to `*.old`, new ones renamed over). |
| macOS app | runs from `*.app/Contents/MacOS` | `Corrent-<v>-macos.dmg` | **Not replaced in place.** The disk image is downloaded to Downloads, checked and opened; the dialog says to quit and drag Corrent into Applications. Replacing a bundle safely needs things nobody has run yet on a real Mac (ad-hoc or Developer ID signatures surviving the copy, App Translocation, `/Applications` owned by another user). |
| Scoop, Homebrew, `/usr`·`/opt`·`/nix` (AUR and other packages), Flatpak, Snap | the path (`\scoop\apps\`), a `corrent` cask in the Caskroom, `$FLATPAK_ID`, `$SNAP` | none | Never touched. The dialog shows the command (`scoop update corrent`, `brew upgrade --cask corrent`, `yay -S corrent-bin`, `flatpak update io.github.ulaas.Corrent`, `snap refresh corrent`) and the release page. |
| Read-only folder, a build from source, a copy moved out of its package, not x86-64 | — | none | Told so, with the release page. |

**Coming back.** A process holds the data-folder lock (`corrent.lock`) for its whole life. The
new version is started with `--after-update` *before* the old one quits and waits (up to 90 s)
for that lock; then the old engine has saved and stopped, its single-instance pipe is closed and
its files are free, and the usual start follows. Without the wait the new process would forward
its launch to the old instance and exit. `--portable` and `--tv` are passed on.

**The MSI's own actions.** Corrent passes nothing to the installer but `/passive /norestart`: the
upgrade keeps the features the installation had (`MajorUpgrade` migrates them). `--unregister`
is skipped on upgrades (`NOT UPGRADINGPRODUCTCODE`); `--register` runs again only where the
*Open magnet links and .torrent files with Corrent* feature was installed, exactly as when the
user runs a newer MSI by hand. The updater itself never registers or unregisters anything. A
`corrent-mpvw.exe` still running (a scan in flight) can leave the installer with a file in use:
Windows then replaces it at the next restart (3010).

**Contracts a release must keep** (older versions rely on them):
- the asset names in the table above, and `SHA256SUMS.txt` in `sha256sum` format listing them;
- the portable zip's top folder `Corrent/`, the tarball's flat `corrent` and `corrent-mpvw`;
- `--after-update` (wait for the previous version, then start normally) and the MSI's
  `UpgradeCode`, per-user scope and `InstallDir` registry value.

Not covered: `correntd` and the web client (docs/WEB-PARITY.md), pre-releases, delta updates,
going back to an older version after an update that worked.

Testing without GitHub (debug builds only): `CORRENT_DEV_UPDATE=http://127.0.0.1:<port>` makes
the check read `<base>/latest.json` and accept downloads under `<base>/` over plain HTTP;
`CORRENT_DEV_UPDATE_NOW=1` starts the update as soon as it is found.

## Checklist for a release

1. Bump `version` in `Cargo.toml` and the `<release>` in the metainfo; `cargo build` to update the lock file.
2. Run the gates (`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo deny check && cargo test`)
   and `scripts/release-hygiene.ps1` (no debug hooks or test code in the release binaries).
3. Commit, tag `vX.Y.Z`, push the tag (owner). The Release workflow's `ci-gate` job waits until CI has
   passed on the tagged commit before building, and fails if that CI run fails or is cancelled (re-run CI, then the
   release). A manual run can tick `skip_ci_gate` in an emergency.
4. Check the draft release's artifacts on each platform (`docs/PLATFORM-TESTING.md`), then publish.
5. Publishing the release runs `.github/workflows/package-manifests.yml`: it fills the winget, Scoop,
   Homebrew and AUR templates with the release's version and SHA-256 sums and attaches them to the
   release (`package-manifests.zip` and the single files; re-run it by hand with a tag if needed).
   Submit them by hand (winget-pkgs, a Scoop bucket, a Homebrew tap, the AUR — run
   `makepkg --printsrcinfo > .SRCINFO` there), and keep the `symbols-windows` artifact with the release.
