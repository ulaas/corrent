# Platform verification checklist

Windows is exercised continuously during development. Linux and macOS code is compiled
(see below) but has **not yet been run on real machines**. Use this checklist the first time
Corrent runs on each platform, and after changes to `src/integration/`, `src/app/tray.rs`,
`src/app/titlebar.rs` or `src/app/pickers.rs`. Record results at the bottom.

## What is verified automatically

| Check | How | Status |
|---|---|---|
| Whole crate type-checks for Linux, including native C deps | `scripts/cross/build-linux.ps1 -CheckOnly` | ✅ 2026-10-08 |
| Linux x86-64 release binary links | `scripts/cross/build-linux.ps1 -Release` | ✅ 2026-10-08 |
| Linux binary needs only glibc ≥ 2.28 (`libc`, `libm`, `libpthread`, `libdl`); X11/Wayland/D-Bus are loaded at runtime | `python scripts/cross/elfneeded.py <binary>` | ✅ 2026-10-08 |
| macOS-only modules (`macos_events`, `os_handler`) type-check for `aarch64-apple-darwin` | isolated scratch crate, see CLAUDE.md | ✅ 2026-10-08 |
| Full macOS build | needs a Mac (or CI `macos-latest`) | ⏳ |

## Manual checklist (Linux and macOS)

Run from a terminal so logs are visible (`CORRENT_LOG=debug corrent --portable`).

1. **Start-up**: window opens with the custom title bar (Linux) or native traffic lights over a
   transparent title bar (macOS); fonts and icons render; no errors in the log.
2. **Window controls** (Linux): drag the title bar, double-click to maximize, minimize/maximize/
   close buttons, resize from every edge and corner.
3. **Add flow**: `corrent --portable "magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel"`
   opens the add dialog with the file list; Add downloads the torrent.
4. **File pickers**: Add torrent file, Browse in the add dialog and Settings. On Linux these use the
   XDG desktop portal (needs `xdg-desktop-portal` + a backend such as `-gtk`, `-kde` or `-gnome`).
   The window must keep repainting while a picker is open.
5. **Single instance**: with Corrent running, run the command from step 3 again; the second process
   exits immediately and the running window comes to the front with the torrent.
6. **Handler registration**: Settings → General → "Make Corrent the default".
   - Linux: `xdg-mime query default x-scheme-handler/magnet` prints `corrent.desktop`;
     `xdg-open "magnet:?xt=urn:btih:…"` reaches Corrent; `~/.local/share/applications/corrent.desktop`
     exists and its `Exec=` line quotes the path correctly (try a path with a space).
   - macOS: build the bundle (`cargo bundle --release`), launch it once, then
     `open "magnet:?xt=urn:btih:…"` — both while Corrent is running and while it is closed
     (Apple Event path). Double-click a `.torrent` in Finder.
7. **Tray**: tray icon appears (Linux: needs a StatusNotifier host — KDE, GNOME with the
   AppIndicator extension, XFCE, …); left-click toggles the window; menu actions work;
   close-to-tray and minimize-to-tray work.
8. **Notifications**: a completed download shows a desktop notification.
9. **Drag & drop**: drop a `.torrent` file onto the window.
10. **Restart**: quit and reopen; torrents come back without re-checking; no "closed unexpectedly"
    banner after a normal quit.
11. **Servers on the network**: with a `correntd` running on the same network, a notice
    "“name” is on this network" appears within seconds. macOS asks for local-network access the
    first time (the bundle declares `NSLocalNetworkUsageDescription` and `NSBonjourServices`;
    without them macOS 14+ finds nothing). Linux: works next to Avahi (both share UDP 5353).
12. **Data paths**: data lives in `~/.local/share/corrent` (Linux) or
    `~/Library/Application Support/Corrent` (macOS); the data dir is mode 0700 and contains
    `corrent.sock` while running.
13. **Update and restart** (docs/PACKAGING.md "Updates in place"; needs a published release newer
    than the build under test, or a debug build with `CORRENT_DEV_UPDATE`). None of these has run
    on a real installation yet except the Windows portable swap.
    - Windows MSI: install an older MSI, click Update and restart. The app closes, the installer's
      progress window runs without questions, the new version starts; `%APPDATA%\Corrent` is
      untouched, `<profile>\update` is gone after that start, the handler registration is as it
      was. Cancel the installer's window once: a message, and the old version starts again.
    - AppImage: from a writable folder, update; the file is replaced, the new one starts, the
      `.old` file goes at the next start. From a read-only folder: "How to update" only.
    - Linux tarball: unpack into `~/corrent`, update; both programs replaced, the new one starts.
      Installed under `/usr` (the AUR package): only the package manager's command is shown.
    - Flatpak, Snap, Scoop, Homebrew: "How to update" with the right command; nothing downloaded.
    - macOS: the dmg lands in Downloads, opens, and the dialog says to drag it across; run from
      the mounted image or translocated: told to use the release page.

## Results

| Date | Platform / distro / desktop | Tester | Result | Notes |
|---|---|---|---|---|
| | | | | |
