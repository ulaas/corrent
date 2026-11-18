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
| Full macOS build, app bundle, dmg and server pkg (arm64) | `scripts/package-macos.sh` on a Mac | ✅ 2026-11-15 |

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
   - macOS: build the bundle (`scripts/package-macos.sh`), launch it once, then
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
    - macOS (Developer ID build in a folder the user owns): Update and restart replaces the app;
      the new one starts by itself, `codesign --verify --deep --strict` and `spctl --assess` pass on
      it, no `.Corrent.app.*` folder is left, `hdiutil info` shows no image. Run from the mounted
      image, translocated, ad hoc signed or from an admin's `/Applications` as a standard user:
      the dmg lands in Downloads, opens, and the dialog says why and to drag it across.

## Smart TVs (needs a real TV; any platform)

Playing to DLNA renderers and AirPlay receivers was checked on one Samsung TV (see Results). On
a real Samsung (Tizen), LG or Sony TV on the same network, with the TV on (steps 1–4 over DLNA,
then the same over AirPlay after pairing in Settings → Casting):

1. The player's cast list shows the TV by its name (if not: add its description address under
   Settings → Playback → Casting → devices, `http://<tv>:9197/dmr` on Samsung).
2. An MP4 or MKV with H.264 or H.265 and AAC/AC-3 sound starts within a few seconds (the TV may
   first ask to allow the device); the position follows, pause, seek, volume, mute and stop work,
   and at the end the item is marked played and the next one starts.
3. A file with DTS sound, and any file with a subtitle chosen, plays converted (Settings →
   Sessions shows the conversion); seeking restarts the stream at the new place within a few
   seconds; pausing for two minutes and resuming works.
4. With the firewall closed for the media port the player says the device couldn't reach this
   computer, not that it can't play the file.

Note the TV's model and what failed (the log has the renderer's error codes at info level).

## Results

| Date | Platform / distro / desktop | Tester | Result | Notes |
|---|---|---|---|---|
| 2026-11-19 | Samsung QE85Q70B (2022, Tizen) from Linux | Claude, with the owner at the TV | Partial | Smart TVs steps 1–2 over DLNA (found, plays an MP4, position and volume follow) after two fixes the TV needed (`SOAPACTION` in capitals, a connection per command). AirPlay: paired with the PIN; MP4 and HLS play; pause, resume, seek, start position, end of item; a refused file is converted. Not run: DLNA seek and conversions (step 3), the firewall case (step 4), long playback, subtitles on either path. |
| 2026-11-15 | macOS 27.2, Apple silicon | Claude (packaging review) | Partial | Only start-up and quit (step 1, 10) from the dmg's `Corrent.app --portable` opened with `open`: window up, no Terminal window, bundled libmpv loaded from `Contents/Frameworks`, Cmd-Q-style quit leaves no crash marker. The rest of the checklist not run. |
