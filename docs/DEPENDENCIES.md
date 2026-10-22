# Dependency policy

- **MSRV:** Rust 1.96 (`rust-version` in `Cargo.toml`; raised from 1.95 on 2026-10-22 because wasmtime 49,
  used by the plugins, needs 1.96), enforced by the `msrv` CI job. Raise it only
  in a dedicated commit, when a dependency requires it.
- **Pinned exactly:** `egui`, `eframe`, `egui_extras` (UI API churns between minors) and `librqbit`
  (engine behavior and its on-disk `session/` format). Upgrade them by hand in a dedicated commit:
  1. read the changelogs; 2. back up a real profile (`Settings → Advanced → Back up data…`);
  3. run the full test suite and the manual checklist in `docs/PLATFORM-TESTING.md`;
  4. for librqbit, also confirm an existing profile restores without re-checking.
- **Vendored patches:** `vendor/librqbit-dht`, `vendor/librqbit` and `vendor/librqbit-bencode` must match the pinned librqbit
  version; re-apply or drop the patches on every librqbit upgrade (see each `CORRENT-PATCH.md` and
  the diffs in `docs/upstream/`).
- **Everything else:** semver ranges; Dependabot proposes weekly updates (UI and engine crates are
  excluded, see `.github/dependabot.yml`).
- **Licenses and advisories:** `cargo deny check` (policy in `deny.toml`) runs in CI. Allowed licenses
  are permissive (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, MPL-2.0, …) plus OFL-1.1 for fonts.
- **Gates:** `cargo fmt --check`, `cargo clippy --all-targets` with `-D warnings`, `cargo test` on
  Windows, Linux and macOS.
