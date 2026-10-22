# Contributing to Corrent

Thanks for helping! Bug reports, translations and pull requests are all welcome.

## Building

Rust 1.96 or newer (see the README for platform notes):

```sh
cargo build
cargo run -- --portable        # keeps all data next to the executable, away from your real profile
```

Playback and the media library use libmpv, loaded at runtime. On Windows,
`powershell -File scripts/fetch-libmpv.ps1` puts it next to the executable; on Linux install
`libmpv2` (or your distribution's equivalent).

## Before you open a pull request

CI runs the same checks; `scripts/gate.ps1` (Windows) or `scripts/gate.sh` runs them all and
prints `GATE OK`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

Changes to the web client (`web/`) also need `npm ci && npm run check` in `web/`, and
`npm run build` to refresh `web/dist` (the server embeds it).

## Conventions

- **Every user-visible string goes through `t!`** and needs a Turkish entry in
  `assets/i18n/tr.toml` (a test fails otherwise). Use named placeholders.
- **The UI thread never blocks on I/O.** It sends commands to the engine and reads snapshots.
- New settings fields must be classified in `diagnostics::SETTINGS_FIELDS` (a test fails until
  they are), so support bundles never leak them.
- Test with freely licensed torrents (Sintel, Big Buck Bunny, Linux ISOs) and in portable mode.
- Keep commits focused, with a message that says what changed and why.

The architecture is described in `docs/` (start with `docs/HANDOVER.md`); the vendored librqbit
patches are explained in each `vendor/*/CORRENT-PATCH.md`.

## License

By contributing you agree that your work is licensed under the MIT or Apache-2.0 license, at the
user's choice, like the rest of Corrent.
