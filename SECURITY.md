# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue: use
**[Report a vulnerability](https://github.com/ulaas/corrent/security/advisories/new)** on the
repository's Security tab. Include what you found, how to reproduce it and which version or
commit you used. You should hear back within a week.

Things that are especially interesting:

- the remote-control and media-server APIs (authentication, pairing, the LAN guard, TLS),
- parsing of data from the network or disk (torrent files, magnet links, bencode, feeds),
- the local streaming server and its tokens,
- anything that writes outside the folders a user chose.

## Supported versions

Corrent is pre-1.0: fixes go into the latest release only.

## Diagnostics and crash reports

Crash reports and diagnostics bundles stay on your machine. If you attach one to a report, the
bundle from **⋮ → Save diagnostics…** (or `corrent --diagnostics <zip>`) leaves out passwords,
keys, tokens, feed addresses and your torrent list, and strips home folders, links and info
hashes from the logs.
