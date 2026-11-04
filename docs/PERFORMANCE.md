# Performance

Scale target (roadmap A9): 5,000 torrents, UI frame < 8 ms, actor tick < 20 ms.

Measure with the ignored test (release build):

```
cargo test --release perf_5000 -- --ignored --nocapture
```

Results on the development machine (Windows 11, 2026-10-08, 5,000 synthetic torrents):

| What | Time |
|---|---|
| Snapshot clone (the actor's per-tick publish) | 1.5 ms |
| New snapshot + filter/sort of the visible rows | 1.4 ms |
| Full UI frame with a new snapshot (incl. kittest/AccessKit overhead) | 3.8 ms |
| Full UI frame, unchanged snapshot | 0.4 ms |

The table only lays out visible rows; rows are re-filtered/sorted only when the snapshot
generation, the search text, the sort or a filter changes. Sidebar counts are cached per
generation. The Activity sparkline data (40 samples per torrent) is rebuilt every other tick and
shared behind an `Arc`.

Engine-side costs (librqbit tasks per torrent) are not synthesized here; they scale with the
number of *running* torrents, which the queue limits.

## Engine database writes (S2 / D5)

librqbit rewrites `session/session.json` in full on every add, pause, resume and file-selection
change. Since D5 it also syncs the file before the rename. The cost, measured with an ignored
test:

```
cargo test -p corrent-core --lib perf_session_flush -- --ignored --nocapture
```

Results on the development machine (Windows 11, NVMe SSD, 2026-10-19, 250 torrents, two runs
each):

| What | Without the sync | With the sync |
|---|---|---|
| Add 250 torrents | 505 ms | 708 ms |
| Resume all 250 | 318 ms | 523 ms |
| Pause all 250 | 270 ms | 400 ms |

That's about 0.6–1 ms per write. The file is rewritten whole, so the cost per write grows with
the number of torrents. With thousands of torrents, or on a hard disk or network drive, writes
should be batched (one per ~500 ms, at once on delete and stop). L4 and F16 in
`docs/STABILITY.md` measure that first.

## Per-tick static data (engine quality round, 2026-11-05)

Every tick formatted each torrent's tracker URLs and every snapshot clone copied them. They only
change with a re-add, so they are built once (`StaticInfo`) and shared: `TorrentView::trackers` is
an `Arc<[String]>`. The ignored test measures it (release build, 2,000 paused torrents with three
trackers each):

```
cargo test -p corrent-core --release --lib perf_engine_tick -- --ignored --nocapture
```

| What | Time |
|---|---|
| Snapshot clone, trackers shared | 0.35 ms |
| The copies of the trackers a clone no longer makes | 0.30 ms |
| Slowest of the last 60 ticks | 6 ms (before and after: dominated by other work) |
