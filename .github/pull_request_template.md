## What and why

<!-- What does this change, and why? Link the issue if there is one. -->

## Checklist

- [ ] `scripts/gate.ps1` / `scripts/gate.sh` prints `GATE OK` (fmt, clippy, tests, cargo deny)
- [ ] New user-visible strings use `t!` and have a Turkish entry in `assets/i18n/tr.toml`
- [ ] `web/`: `npm run check` passes and `web/dist` is rebuilt (only if the web client changed)
