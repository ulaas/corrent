//! STABILITY.md workstream Z, property tests P1–P10: the queue, counters, the queue planner,
//! seeding limits, the schedule, settings and store round trips, config files, IP filter lists
//! and download rules. Case counts are sized for the gate; `CORRENT_FUZZ_ITERS` raises them all
//! (`scripts/fuzz-long.ps1`). A failing case is shrunk by proptest and written to
//! `proptest-regressions/` next to this file: commit it with the fix.

use std::time::Duration;

use proptest::prelude::*;

use crate::config::{Queue, Schedule, Seeding};
use crate::engine::logic::{QueueItem, SLOW_GRACE, in_schedule, plan_queue, seed_limit_reason};
use crate::engine::types::{InfoHash, TorrentState, TorrentView};
use crate::store::{PauseReason, QueueMove, Store, TorrentMeta};

fn cases(n: usize) -> ProptestConfig {
    ProptestConfig { cases: corrent_fuzz::iterations(n) as u32, ..ProptestConfig::default() }
}

fn hash(i: usize) -> InfoHash {
    InfoHash([i as u8; 20])
}

// ---- P1: queue positions --------------------------------------------------------------------

#[derive(Clone, Debug)]
enum QOp {
    Add,
    Place(Vec<usize>, Option<usize>),
    Move(usize, QueueMove),
    Remove(usize),
    Normalize,
}

fn qop() -> impl Strategy<Value = QOp> {
    let mv = prop_oneof![Just(QueueMove::Top), Just(QueueMove::Up), Just(QueueMove::Down), Just(QueueMove::Bottom)];
    prop_oneof![
        2 => Just(QOp::Add),
        3 => (prop::collection::vec(0..48usize, 0..5), prop::option::of(0..48usize)).prop_map(|(h, b)| QOp::Place(h, b)),
        3 => (0..48usize, mv).prop_map(|(h, m)| QOp::Move(h, m)),
        1 => (0..48usize).prop_map(QOp::Remove),
        1 => Just(QOp::Normalize),
    ]
}

/// The store's queue, front first.
fn queue_order(s: &Store) -> Vec<InfoHash> {
    let mut v: Vec<(u32, InfoHash)> = s.data.torrents.iter().map(|(h, m)| (m.queue_pos, *h)).collect();
    v.sort();
    v.into_iter().map(|(_, h)| h).collect()
}

proptest! {
    #![proptest_config(cases(2_000))]

    /// P1: positions stay exactly 0..n, and every operation does what a plain list model does.
    #[test]
    fn p1_queue_stays_a_dense_ordered_list(start in 0..12usize, ops in prop::collection::vec(qop(), 0..30)) {
        let path = std::env::temp_dir().join(format!("corrent-p1-{}-never-written.json", std::process::id()));
        let mut s = Store::load(path);
        let mut model: Vec<InfoHash> = Vec::new();
        // New torrents get the next unused hash (the store appends them at the end).
        let add = |s: &mut Store, model: &mut Vec<InfoHash>| {
            if let Some(h) = (0..48).map(hash).find(|h| !model.contains(h)) {
                s.meta_quiet(h);
                model.push(h);
            }
        };
        for _ in 0..start {
            add(&mut s, &mut model);
        }
        for op in ops {
            match op {
                QOp::Add => add(&mut s, &mut model),
                QOp::Place(hs, before) => {
                    let hs: Vec<InfoHash> = hs.into_iter().map(hash).collect();
                    let before = before.map(hash);
                    s.queue_place(&hs, before);
                    let moving: Vec<InfoHash> = model.iter().filter(|h| hs.contains(h)).copied().collect();
                    if !moving.is_empty() && !before.is_some_and(|b| moving.contains(&b)) {
                        model.retain(|h| !moving.contains(h));
                        let at = before.and_then(|b| model.iter().position(|h| *h == b)).unwrap_or(model.len());
                        model.splice(at..at, moving);
                    }
                }
                QOp::Move(i, mv) => {
                    s.queue_move(hash(i), mv);
                    if let Some(cur) = model.iter().position(|h| *h == hash(i)) {
                        let h = model.remove(cur);
                        let at = match mv {
                            QueueMove::Top => 0,
                            QueueMove::Bottom => model.len(),
                            QueueMove::Up => cur.saturating_sub(1),
                            QueueMove::Down => (cur + 1).min(model.len()),
                        };
                        model.insert(at, h);
                    }
                }
                QOp::Remove(i) => {
                    s.remove(&hash(i));
                    model.retain(|h| *h != hash(i));
                }
                QOp::Normalize => s.normalize_queue(),
            }
            let mut pos: Vec<u32> = s.data.torrents.values().map(|m| m.queue_pos).collect();
            pos.sort_unstable();
            prop_assert_eq!(pos, (0..model.len() as u32).collect::<Vec<_>>());
            prop_assert_eq!(queue_order(&s), model.clone());
        }
        s.discard();
    }
}

// ---- P2: transfer counters ------------------------------------------------------------------

fn raw_counters() -> impl Strategy<Value = Vec<(u64, u64)>> {
    // Runs of growth, resets to small values (a re-add), and values near the top of u64.
    let step = prop_oneof![
        6 => (0..1u64 << 30, 0..1u64 << 30).prop_map(|(a, b)| (a, b, false)),
        1 => (0..1000u64, 0..1000u64).prop_map(|(a, b)| (a, b, true)),
        1 => (u64::MAX - 1000..=u64::MAX, u64::MAX - 1000..=u64::MAX).prop_map(|(a, b)| (a, b, true)),
    ];
    prop::collection::vec(step, 0..40).prop_map(|steps| {
        let (mut up, mut down) = (0u64, 0u64);
        steps
            .into_iter()
            .map(|(a, b, set)| {
                if set {
                    (up, down) = (a, b);
                } else {
                    (up, down) = (up.saturating_add(a), down.saturating_add(b));
                }
                (up, down)
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(cases(5_000))]

    /// P2: the totals never go down and are the sum of what each call reported.
    #[test]
    fn p2_counters_never_decrease(raws in raw_counters()) {
        let mut m = TorrentMeta::default();
        let (mut sum_up, mut sum_down) = (0u64, 0u64);
        for (up, down) in raws {
            let before = (m.uploaded_total, m.downloaded_total);
            let (du, dd) = m.absorb_counters(up, down);
            prop_assert!(m.uploaded_total >= before.0 && m.downloaded_total >= before.1);
            sum_up = sum_up.saturating_add(du);
            sum_down = sum_down.saturating_add(dd);
            prop_assert_eq!((m.uploaded_total, m.downloaded_total), (sum_up, sum_down));
        }
    }
}

// ---- P3: the queue planner ------------------------------------------------------------------

const STATES: [TorrentState; 9] = [
    TorrentState::Metadata,
    TorrentState::Checking,
    TorrentState::Downloading,
    TorrentState::Stalled,
    TorrentState::Seeding,
    TorrentState::Completed,
    TorrentState::Paused,
    TorrentState::Queued,
    TorrentState::Error,
];

#[derive(Clone, Debug)]
struct PItem {
    view: TorrentView,
    meta: TorrentMeta,
    slow_for: Duration,
    since_resume: Option<Duration>,
}

fn pitem() -> impl Strategy<Value = (usize, bool, bool, bool, bool, u8, bool, u8, u8, u8)> {
    // state, finished, pending, busy, has_metadata, pause reason, force-started, slow, resumed, speed
    (
        0..9usize,
        any::<bool>(),
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.9),
        0..4u8,
        prop::bool::weighted(0.1),
        0..2u8,
        0..3u8,
        0..3u8,
    )
}

fn queue() -> impl Strategy<Value = Queue> {
    (1..=6u32, 1..=6u32, 1..=10u32, any::<bool>()).prop_map(|(d, u, t, ignore_slow)| Queue {
        enabled: true,
        max_active_downloads: d,
        max_active_uploads: u,
        max_active_total: t,
        ignore_slow,
        slow_down_kib: 2,
        slow_up_kib: 2,
    })
}

fn items() -> impl Strategy<Value = Vec<PItem>> {
    prop::collection::vec(pitem(), 0..25)
        .prop_flat_map(|raw| {
            let n = raw.len();
            // Distinct queue positions (ties would make the order depend on the input order).
            (Just(raw), Just((0..n as u32).collect::<Vec<u32>>()).prop_shuffle())
        })
        .prop_map(|(raw, pos)| {
            raw.into_iter()
                .zip(pos)
                .enumerate()
                .map(|(i, ((st, fin, pending, busy, meta_ok, reason, force, slow, resumed, speed), pos))| {
                    let view = TorrentView {
                        hash: hash(i),
                        state: STATES[st],
                        size: 100,
                        done: if fin { 100 } else { 40 },
                        completed_at: fin.then_some(1),
                        pending,
                        busy: busy.then(|| "moving".to_string()),
                        has_metadata: meta_ok,
                        queue_pos: pos,
                        down_bps: [0, 1024, 1 << 20][speed as usize],
                        up_bps: [0, 1024, 1 << 20][speed as usize],
                        ..TorrentView::default()
                    };
                    let meta = TorrentMeta {
                        pause_reason: [
                            None,
                            Some(PauseReason::User),
                            Some(PauseReason::Queue),
                            Some(PauseReason::SeedLimit),
                        ][reason as usize],
                        force_started: force,
                        ..TorrentMeta::default()
                    };
                    let slow_for = if slow == 1 { SLOW_GRACE + Duration::from_secs(1) } else { Duration::ZERO };
                    let since_resume =
                        [None, Some(Duration::ZERO), Some(SLOW_GRACE + Duration::from_secs(1))][resumed as usize];
                    PItem { view, meta, slow_for, since_resume }
                })
                .collect()
        })
}

fn plan(q: &Queue, items: &[PItem]) -> crate::engine::logic::QueuePlan {
    plan_queue(
        q,
        items
            .iter()
            .map(|i| QueueItem { view: &i.view, meta: &i.meta, slow_for: i.slow_for, since_resume: i.since_resume })
            .collect(),
    )
}

fn running(s: TorrentState) -> bool {
    matches!(s, TorrentState::Downloading | TorrentState::Stalled | TorrentState::Seeding | TorrentState::Metadata)
}

/// What `plan_queue` controls (the rest it leaves alone).
fn controlled(i: &PItem) -> bool {
    let v = &i.view;
    !v.pending
        && v.busy.is_none()
        && v.has_metadata
        && !matches!(v.state, TorrentState::Error | TorrentState::Checking)
        && !i.meta.force_started
        && !matches!(i.meta.pause_reason, Some(PauseReason::User) | Some(PauseReason::SeedLimit))
}

proptest! {
    #![proptest_config(cases(3_000))]

    /// P3: the plan doesn't depend on the input order, pauses only running torrents it controls,
    /// resumes only queue-paused ones, never touches force-started ones, and without the
    /// slow-torrent exemption keeps every limit.
    #[test]
    fn p3_queue_plan_keeps_the_limits(q in queue(), items in items(), seed in any::<u64>()) {
        let p = plan(&q, &items);
        let mut shuffled = items.clone();
        let mut r = corrent_fuzz::Rng::new(seed);
        for i in (1..shuffled.len()).rev() {
            shuffled.swap(i, r.below(i + 1));
        }
        prop_assert_eq!(&plan(&q, &shuffled), &p, "the input order matters");
        let by_hash = |h: &InfoHash| items.iter().find(|i| i.view.hash == *h).unwrap();
        for h in &p.pause {
            let i = by_hash(h);
            prop_assert!(controlled(i) && running(i.view.state), "paused {:?}", i.view.state);
            prop_assert!(!p.resume.contains(h));
        }
        for h in &p.resume {
            let i = by_hash(h);
            prop_assert!(controlled(i) && !running(i.view.state) && i.meta.pause_reason == Some(PauseReason::Queue));
        }
        if !q.ignore_slow {
            // After the plan: running = (running ∖ paused) ∪ resumed, among the controlled.
            let active: Vec<&PItem> = items
                .iter()
                .filter(|i| controlled(i))
                .filter(|i| (running(i.view.state) && !p.pause.contains(&i.view.hash)) || p.resume.contains(&i.view.hash))
                .collect();
            let up = active.iter().filter(|i| i.view.completed_at.is_some() && i.view.done >= i.view.size).count() as u32;
            let dl = active.len() as u32 - up;
            prop_assert!(active.len() as u32 <= q.max_active_total, "{} active > {}", active.len(), q.max_active_total);
            prop_assert!(dl <= q.max_active_downloads && up <= q.max_active_uploads, "{dl} down, {up} up");
        }
    }
}

// ---- P4: seeding limits ---------------------------------------------------------------------

fn seeding() -> impl Strategy<Value = Seeding> {
    (any::<bool>(), 0.0f32..10.0, any::<bool>(), 0..5000u32, any::<bool>(), 0..5000u32).prop_map(
        |(re, r, te, t, ie, i)| Seeding {
            ratio_enabled: re,
            ratio: r,
            time_enabled: te,
            time_minutes: t,
            inactive_enabled: ie,
            inactive_minutes: i,
            ..Seeding::default()
        },
    )
}

fn seed_meta() -> impl Strategy<Value = TorrentMeta> {
    (
        prop::option::of(0.0f32..10.0),
        prop::option::of(0..5000u32),
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.1),
        0..400_000u64,
        0..400_000u64,
    )
        .prop_map(|(ro, to, exempt, force, secs, inactive)| TorrentMeta {
            seed_ratio_override: ro,
            seed_time_override_min: to,
            seed_limit_exempt: exempt,
            force_started: force,
            seeding_seconds: secs,
            inactive_seeding_secs: inactive,
            ..TorrentMeta::default()
        })
}

proptest! {
    #![proptest_config(cases(5_000))]

    /// P4: only a seeding torrent that isn't exempt or force-started stops; more ratio, more time
    /// or more inactivity never undo a stop; a per-torrent ratio overrides the global one.
    #[test]
    fn p4_seed_limits_are_monotonic(
        s in seeding(),
        m in seed_meta(),
        st in 0..9usize,
        ratio in 0.0f32..20.0,
        more in (0.0f32..5.0, 0..100_000u64, 0..100_000u64),
        other_global in (any::<bool>(), 0.0f32..10.0),
    ) {
        let v = TorrentView { state: STATES[st], ratio, ..TorrentView::default() };
        let r = seed_limit_reason(&v, &m, &s);
        if v.state != TorrentState::Seeding || m.seed_limit_exempt || m.force_started {
            prop_assert!(r.is_none());
        }
        if r.is_some() {
            let v2 = TorrentView { ratio: ratio + more.0, ..v.clone() };
            let m2 = TorrentMeta {
                seeding_seconds: m.seeding_seconds + more.1,
                inactive_seeding_secs: m.inactive_seeding_secs + more.2,
                ..m.clone()
            };
            prop_assert!(seed_limit_reason(&v2, &m2, &s).is_some(), "more seeding undid the stop");
        }
        if m.seed_ratio_override.is_some() {
            // The global ratio setting doesn't matter then (time and inactivity limits still do).
            let s2 = Seeding { ratio_enabled: other_global.0, ratio: other_global.1, ..s.clone() };
            let no_ratio = |s: &Seeding| Seeding { ratio_enabled: false, ..s.clone() };
            let a = seed_limit_reason(&v, &m, &s).is_some();
            let b = seed_limit_reason(&v, &m, &s2).is_some();
            prop_assert_eq!(a, b);
            prop_assert_eq!(seed_limit_reason(&v, &m, &no_ratio(&s)).is_some(), a);
        }
    }
}

// ---- P5: the alternative-speed schedule ------------------------------------------------------

/// The model: the minutes walked from `from` up to (not including) `to`, around midnight.
fn scheduled_minutes(from: u16, to: u16) -> Vec<bool> {
    let mut on = vec![false; 1440];
    let mut m = from % 1440;
    while m != to % 1440 {
        on[m as usize] = true;
        m = (m + 1) % 1440;
    }
    on
}

fn check_schedule(sch: &Schedule) {
    let on = scheduled_minutes(sch.from_minutes, sch.to_minutes);
    for day in 0..7u8 {
        let day_on = sch.enabled && sch.days & (1 << day) != 0;
        for minute in 0..1440u16 {
            assert_eq!(
                in_schedule(sch, minute, day),
                day_on && on[minute as usize],
                "{sch:?} day {day} minute {minute}"
            );
        }
    }
}

/// P5, exhaustively: every minute of every weekday, for windows within a day, across midnight,
/// empty, and turned off.
#[test]
fn p5_schedule_every_minute_of_the_week() {
    for (from, to) in [(9 * 60, 17 * 60), (22 * 60, 6 * 60), (0, 1439), (1439, 0), (600, 600), (0, 0)] {
        for days in [0b111_1111, 0b001_1111, 0b110_0000, 0] {
            for enabled in [true, false] {
                check_schedule(&Schedule { enabled, days, from_minutes: from, to_minutes: to });
            }
        }
    }
}

proptest! {
    #![proptest_config(cases(300))]

    #[test]
    fn p5_schedule_matches_the_model(from in 0..1440u16, to in 0..1440u16, days in any::<u8>(), enabled in any::<bool>()) {
        check_schedule(&Schedule { enabled, days, from_minutes: from, to_minutes: to });
    }

    /// Out-of-range input (a weekday past Sunday) is "not scheduled", not a panic.
    #[test]
    fn p5_any_weekday_is_safe(minute in any::<u16>(), weekday in any::<u8>(), days in any::<u8>()) {
        let sch = Schedule { enabled: true, days, from_minutes: 0, to_minutes: 1439 };
        let on = in_schedule(&sch, minute, weekday);
        prop_assert!(weekday < 7 || !on);
    }
}
