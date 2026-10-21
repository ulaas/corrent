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

// ---- P6: settings round trip ----------------------------------------------------------------

const EDGE_INTS: &[i64] = &[0, 1, -1, 255, 256, 65_535, 65_536, 4_294_967_295, 4_294_967_296, i64::MAX, i64::MIN];

/// One random edit somewhere in a TOML document: a leaf value replaced with one of its kind, or
/// an array element added or removed.
fn mutate_toml(v: &mut toml::Value, r: &mut corrent_fuzz::Rng) {
    match v {
        toml::Value::Table(t) => {
            let keys: Vec<String> = t.keys().cloned().collect();
            if let Some(k) = keys.get(r.below(keys.len()))
                && let Some(child) = t.get_mut(k)
            {
                mutate_toml(child, r);
            }
        }
        toml::Value::Array(a) => match r.below(3) {
            0 if !a.is_empty() => {
                a.remove(r.below(a.len()));
            }
            1 if !a.is_empty() => {
                let c = a[r.below(a.len())].clone();
                a.push(c);
            }
            _ => {
                let n = a.len();
                if let Some(c) = a.get_mut(r.below(n)) {
                    mutate_toml(c, r);
                }
            }
        },
        toml::Value::Integer(i) => {
            *i = if r.chance(50) { *r.pick(EDGE_INTS) } else { (r.next_u64() % 100_000) as i64 };
        }
        toml::Value::Float(f) => *f = [0.0, -1.0, 1e-9, 1e30, 0.5][r.below(5)] + (r.below(1000) as f64) / 10.0,
        toml::Value::String(s) => *s = r.text(24),
        toml::Value::Boolean(b) => *b = !*b,
        toml::Value::Datetime(_) => {}
    }
}

/// Settings with random edits that still deserialize (each field keeps its type's range).
fn arbitrary_settings(seed: u64) -> Option<crate::config::Settings> {
    let mut r = corrent_fuzz::Rng::new(seed);
    let mut v = toml::Value::try_from(crate::config::Settings::default()).unwrap();
    // Watch folders are empty by default: give the edits one to change.
    if let Some(w) = v.get_mut("watch").and_then(|w| w.as_table_mut()) {
        let mut folder = toml::Table::new();
        folder.insert("path".into(), "C:/w".into());
        folder.insert("save_path".into(), "D:/x".into());
        folder.insert("category".into(), "tv".into());
        w.insert("folders".into(), toml::Value::Array(vec![toml::Value::Table(folder)]));
    }
    for _ in 0..=r.below(12) {
        mutate_toml(&mut v, &mut r);
    }
    v.try_into().ok()
}

proptest! {
    #![proptest_config(cases(1_500))]

    /// P6: what deserializes saves and reads back identically, the current version migrates to
    /// itself, and the engine's options build from it without a panic.
    #[test]
    fn p6_settings_round_trip(seed in any::<u64>(), alt in any::<bool>()) {
        use crate::migrate::{SETTINGS_VERSION, migrate_settings};
        let Some(mut s) = arbitrary_settings(seed) else { return Ok(()) };
        s.schema_version = SETTINGS_VERSION;
        let text = toml::to_string_pretty(&s);
        prop_assert!(text.is_ok(), "settings that load can't be saved: {:?}", text.err());
        let back = crate::config::Settings::parse(&text.unwrap());
        prop_assert_eq!(back.as_ref().ok(), Some(&s));
        let mut m = s.clone();
        prop_assert!(!migrate_settings(&mut m, SETTINGS_VERSION));
        prop_assert_eq!(&m, &s);
        let paths = crate::config::Paths { data_dir: std::env::temp_dir().join("corrent-p6-never-written"), portable: true };
        let lists = crate::engine::ipfilter::Prepared::default();
        let _ = crate::engine::session::options(&paths, &s, None, alt, &lists, &tokio_util::sync::CancellationToken::new());
    }
}

// ---- P7: store round trip -------------------------------------------------------------------

fn mutate_json(v: &mut serde_json::Value, r: &mut corrent_fuzz::Rng) {
    use serde_json::Value;
    match v {
        Value::Object(o) => {
            let keys: Vec<String> = o.keys().cloned().collect();
            if let Some(k) = keys.get(r.below(keys.len()))
                && let Some(c) = o.get_mut(k)
            {
                mutate_json(c, r);
            }
        }
        Value::Array(a) => match r.below(3) {
            0 if !a.is_empty() => {
                a.remove(r.below(a.len()));
            }
            1 if !a.is_empty() => {
                let c = a[r.below(a.len())].clone();
                a.push(c);
            }
            _ => {
                let n = a.len();
                if let Some(c) = a.get_mut(r.below(n)) {
                    mutate_json(c, r);
                }
            }
        },
        Value::Number(n) => {
            // Mostly of the same kind and in the narrow types' range (u8 to u32), sometimes not.
            let float = n.is_f64();
            *v = if float || r.chance(5) {
                serde_json::json!((r.below(1000) as f64) / 8.0)
            } else if r.chance(10) {
                serde_json::json!(r.pick(EDGE_INTS))
            } else {
                serde_json::json!(*r.pick(&[0u64, 1, 2, 7, 100, 127, 255, 65_535, 1 << 20, u32::MAX as u64]))
            }
        }
        Value::String(s) => *s = r.text(24),
        Value::Bool(b) => *b = !*b,
        Value::Null => *v = serde_json::json!(r.below(5)),
    }
}

/// A store as Corrent writes it (torrents, categories, tags), with random edits.
fn arbitrary_store(seed: u64) -> Option<crate::store::StoreData> {
    let mut r = corrent_fuzz::Rng::new(seed);
    let fixture = include_bytes!("../tests/fixtures/profile-v2/store.json");
    let mut v: serde_json::Value = serde_json::from_slice(fixture).unwrap();
    for _ in 0..=r.below(12) {
        mutate_json(&mut v, &mut r);
    }
    serde_json::from_value(v).ok()
}

/// Serialize, deserialize, serialize: the same document (no `PartialEq` needed, and map order
/// doesn't matter).
fn json_round_trips<T: serde::Serialize + serde::de::DeserializeOwned>(x: &T) -> Result<(), String> {
    let a = serde_json::to_value(x).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(x).map_err(|e| e.to_string())?;
    let back: T = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let b = serde_json::to_value(&back).map_err(|e| e.to_string())?;
    if a == b { Ok(()) } else { Err(format!("{a}\nbecame\n{b}")) }
}

proptest! {
    #![proptest_config(cases(1_500))]

    /// P7: a store that loads saves and loads back identically; the current version migrates to
    /// itself.
    #[test]
    fn p7_store_round_trip(seed in any::<u64>()) {
        use crate::migrate::{STORE_VERSION, migrate_store};
        let Some(mut d) = arbitrary_store(seed) else { return Ok(()) };
        d.schema_version = STORE_VERSION;
        prop_assert_eq!(json_round_trips(&d), Ok(()));
        let before = serde_json::to_value(&d).unwrap();
        prop_assert!(!migrate_store(&mut d, STORE_VERSION));
        prop_assert_eq!(serde_json::to_value(&d).unwrap(), before);
    }
}

proptest! {
    #![proptest_config(cases(60))]

    /// P7, from disk: a damaged `store.json` is replaced by its `.bak` (with a warning; the
    /// torrents and categories are back).
    #[test]
    fn p7_damaged_store_comes_back_from_its_backup(seed in any::<u64>(), garbage in prop::collection::vec(any::<u8>(), 0..64)) {
        let Some(mut d) = arbitrary_store(seed) else { return Ok(()) };
        d.schema_version = crate::migrate::STORE_VERSION;
        if serde_json::from_slice::<crate::store::StoreData>(&garbage).is_ok() {
            return Ok(());
        }
        let dir = std::env::temp_dir().join(format!("corrent-p7-{}-{seed:x}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.json");
        std::fs::write(&path, &garbage).unwrap();
        std::fs::write(dir.join("store.json.bak"), serde_json::to_vec_pretty(&d).unwrap()).unwrap();
        let s = Store::load(path);
        let ok = s.warning.is_some()
            && s.data.torrents.keys().collect::<std::collections::BTreeSet<_>>() == d.torrents.keys().collect()
            && s.data.categories == d.categories;
        s.discard();
        let _ = std::fs::remove_dir_all(&dir);
        prop_assert!(ok);
    }
}

// ---- P8: config files as found on disk ------------------------------------------------------

proptest! {
    #![proptest_config(cases(1_500))]

    /// P8: damaged settings files never panic: what parses round-trips, salvage keeps the
    /// sections it can.
    #[test]
    fn p8_damaged_settings_files(seed in any::<u64>()) {
        let mut r = corrent_fuzz::Rng::new(seed);
        let fixtures = [
            include_str!("../tests/fixtures/profile-v0/settings.toml"),
            include_str!("../tests/fixtures/profile-v2/settings.toml"),
        ];
        let text = r.case(&fixtures, 256);
        let _ = crate::migrate::toml_version(&text);
        if let Ok(mut s) = crate::config::Settings::parse(&text) {
            crate::migrate::migrate_settings(&mut s, crate::migrate::toml_version(&text));
            let back = crate::config::Settings::parse(&toml::to_string_pretty(&s).unwrap()).unwrap();
            prop_assert_eq!(back, s);
        }
        let (s, _) = crate::config::Settings::salvage(&text);
        prop_assert!(toml::to_string_pretty(&s).is_ok());
    }

    /// P8: damaged store files: what parses migrates and round-trips.
    #[test]
    fn p8_damaged_store_files(seed in any::<u64>()) {
        let mut r = corrent_fuzz::Rng::new(seed);
        let fixtures = [
            include_str!("../tests/fixtures/profile-v0/store.json"),
            include_str!("../tests/fixtures/profile-v2/store.json"),
        ];
        let text = r.case(&fixtures, 256);
        let from = crate::migrate::json_version(text.as_bytes());
        if let Ok(mut d) = serde_json::from_str::<crate::store::StoreData>(&text) {
            crate::migrate::migrate_store(&mut d, from);
            prop_assert_eq!(json_round_trips(&d), Ok(()));
        }
    }
}

/// The P6/P7 generators mostly produce values that load (otherwise those tests check nothing).
#[test]
fn generators_mostly_make_loadable_values() {
    let settings = (0..400).filter(|s| arbitrary_settings(*s).is_some()).count();
    let stores = (0..400).filter(|s| arbitrary_store(*s).is_some()).count();
    assert!(settings > 100 && stores > 100, "{settings} settings, {stores} stores of 400");
}

// ---- P9: IP filter lists --------------------------------------------------------------------

fn ip_entry() -> impl Strategy<Value = String> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let ip = prop_oneof![any::<Ipv4Addr>().prop_map(IpAddr::V4), any::<Ipv6Addr>().prop_map(IpAddr::V6)];
    let noise = "[ -~]{0,12}";
    prop_oneof![
        ip.clone().prop_map(|a| a.to_string()),
        (ip.clone(), 0..140u32).prop_map(|(a, l)| format!("{a}/{l}")),
        (ip.clone(), ip.clone()).prop_map(|(a, b)| format!("{a}-{b}")),
        (noise, ip.clone(), ip.clone()).prop_map(|(n, a, b)| format!("{n}:{a}-{b}")),
        (" *#?", noise).prop_map(|(p, n)| format!("{p}{n}")),
        (ip, noise).prop_map(|(a, n)| format!("{a}{n}")),
    ]
}

proptest! {
    #![proptest_config(cases(5_000))]

    /// P9: entries never panic; a range has one family and starts before it ends; one address is
    /// itself; a CIDR block contains its address and is aligned to its mask; a reversed IPv4
    /// range is refused.
    #[test]
    fn p9_ip_filter_entries(line in ip_entry()) {
        use std::net::IpAddr;
        use crate::engine::ipfilter::parse_entry;
        let r = parse_entry(&line);
        if let Ok(Some((a, b))) = r {
            prop_assert_eq!(a.is_ipv4(), b.is_ipv4());
            prop_assert!(a <= b, "{} > {}", a, b);
        }
        if let Ok(ip) = line.trim().parse::<IpAddr>() {
            prop_assert_eq!(r.clone(), Ok(Some((ip, ip))));
        }
        if let Some((ip, len)) = line.split_once('/')
            && let (Ok(ip), Ok(len)) = (ip.parse::<IpAddr>(), len.parse::<u32>())
        {
            let bits = if ip.is_ipv4() { 32 } else { 128 };
            match r {
                Ok(Some((a, b))) => {
                    prop_assert!(len <= bits && a <= ip && ip <= b);
                    let size = |x: IpAddr| match x {
                        IpAddr::V4(v) => u128::from(v.to_bits()),
                        IpAddr::V6(v) => v.to_bits(),
                    };
                    // [a, b] is exactly the 2^(bits - len) addresses of the block.
                    let span = size(b) - size(a);
                    prop_assert_eq!(span.checked_add(1).map_or(128, |n| n.trailing_zeros()), bits - len);
                }
                _ => prop_assert!(len > bits),
            }
        }
        if let Some((a, b)) = line.split_once('-')
            && let (Ok(IpAddr::V4(a)), Ok(IpAddr::V4(b))) = (a.parse::<IpAddr>(), b.parse::<IpAddr>())
            && a > b
        {
            prop_assert!(r.is_err());
        }
    }

    /// P9: any bytes through the gzip path never panic, and the cap holds.
    #[test]
    fn p9_gzip_lists(bytes in prop::collection::vec(any::<u8>(), 0..512), gz in any::<bool>()) {
        let input = if gz { [vec![0x1f, 0x8b], bytes].concat() } else { bytes };
        if let Ok(out) = crate::engine::ipfilter::gunzip_capped(input, 1 << 16) {
            prop_assert!(out.len() <= 1 << 16 || !gz);
        }
    }
}

/// P9: a gzip bomb stops at the cap; a real gzip list comes back whole.
#[test]
fn p9_gzip_bomb_is_capped() {
    use std::io::Write;
    let gzip = |b: &[u8]| {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    };
    let bomb = gzip(&vec![0u8; 10 << 20]);
    assert!(bomb.len() < 64 << 10);
    assert_eq!(crate::engine::ipfilter::gunzip_capped(bomb, 1 << 20).unwrap().len(), 1 << 20);
    let list = b"corrent:1.2.3.4-1.2.3.9\n# a comment\n10.0.0.0/8\n".repeat(50);
    assert_eq!(crate::engine::ipfilter::gunzip_capped(gzip(&list), 1 << 20).unwrap(), list);
}

// ---- P10: download rules --------------------------------------------------------------------

fn episode_filter_text() -> impl Strategy<Value = String> {
    let num = prop_oneof![0..100u32, any::<u32>()].prop_map(|n| n.to_string());
    let range = prop_oneof![
        num.clone(),
        (num.clone(), num.clone()).prop_map(|(a, b)| format!("{a}-{b}")),
        num.clone().prop_map(|a| format!("{a}-")),
        "[ -~]{0,6}",
    ];
    (prop::bool::ANY, num, prop::collection::vec(range, 0..5), "[ ;]{0,2}")
        .prop_map(|(s, season, ranges, sep)| format!("{}{season}x{}{sep}", if s { "S" } else { "" }, ranges.join(";")))
}

proptest! {
    #![proptest_config(cases(3_000))]

    /// P10: episode filters never panic; what parses allows exactly its ranges (each ends at or
    /// after its start), for its season only.
    #[test]
    fn p10_episode_filters(text in episode_filter_text(), season in 0..100u32, episode in 0..200u32) {
        use crate::rss::rules::EpisodeFilter;
        let Ok(f) = EpisodeFilter::parse(&text) else { return Ok(()) };
        let t = text.trim().to_ascii_lowercase();
        let (s, rest) = t.split_once('x').unwrap();
        let fs: u32 = s.trim().trim_start_matches('s').parse().unwrap();
        let ranges: Vec<(u32, Option<u32>)> = rest
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| match p.split_once('-') {
                Some((a, b)) => (a.trim().parse().unwrap(), b.trim().parse().ok()),
                None => (p.parse().unwrap(), p.parse().ok()),
            })
            .collect();
        prop_assert!(!ranges.is_empty());
        for (a, b) in &ranges {
            prop_assert!(b.is_none_or(|b| b >= *a));
        }
        let want = season == fs && ranges.iter().any(|(a, b)| episode >= *a && b.is_none_or(|b| episode <= b));
        prop_assert_eq!(f.allows(season, episode), want);
        prop_assert!(!f.allows(fs.wrapping_add(1), episode) || fs == u32::MAX);
    }

    /// P10: titles never panic, and a found episode is written in the title.
    #[test]
    fn p10_episode_of_titles(title in "[ -~]{0,40}|.{0,40}", s in 0..100u32, e in 0..1000u32, sep in "[ ._-]?") {
        use crate::rss::rules::episode_of;
        if let Some((a, b)) = episode_of(&title) {
            prop_assert!(title.contains(&a.to_string()) && title.contains(&b.to_string()));
        }
        let t = format!("Show.Name.S{s:02}{sep}E{e:02}.1080p");
        prop_assert_eq!(episode_of(&t), Some((s, e)));
    }
}

/// Patterns that blow up or don't compile.
const NASTY: &[&str] = &[
    "(a*)*b",
    "(a|a)*",
    ".{1000}{1000}",
    "(",
    ")",
    "[",
    "a{99999999}",
    "\\",
    "(?i)ı",
    "\\p{Greek}+",
    "x{2,1}",
    "*",
    "?",
    "|",
    "a||b",
    "(?P<n>a)(?P<n>b)",
    "\\b\\B",
    "[[:alpha:]]",
    "\\u{10FFFF}",
];

proptest! {
    #![proptest_config(cases(1_000))]

    /// P10: any rule compiles to a matcher or an error (no panic, and blow-ups stop at the size
    /// limit), and matching any title doesn't panic.
    #[test]
    fn p10_rules_with_any_patterns(
        contain in prop_oneof![prop::sample::select(NASTY).prop_map(String::from), "[ -~]{0,24}", ".{0,24}"],
        exclude in prop_oneof![prop::sample::select(NASTY).prop_map(String::from), "[ -~]{0,24}"],
        regex in any::<bool>(),
        episodes in prop_oneof![Just(String::new()), episode_filter_text()],
        titles in prop::collection::vec(".{0,60}", 1..4),
    ) {
        let rule = crate::rss::RssRule {
            must_contain: contain,
            must_not_contain: exclude,
            use_regex: regex,
            episode_filter: episodes,
            ..Default::default()
        };
        if let Ok(m) = crate::rss::rules::Matcher::new(&rule) {
            for t in &titles {
                let _ = m.matches(t);
            }
        }
    }
}

/// P10: a pattern that would compile to a huge program is an error, quickly.
#[test]
fn p10_huge_patterns_are_refused_quickly() {
    for pattern in [".{1000}{1000}", "(\\w{100}){100}", "[a-z]{50000}"] {
        let t = std::time::Instant::now();
        let rule = crate::rss::RssRule { must_contain: pattern.into(), use_regex: true, ..Default::default() };
        assert!(crate::rss::rules::Matcher::new(&rule).is_err(), "{pattern}");
        assert!(t.elapsed() < Duration::from_secs(2), "{pattern}: {:?}", t.elapsed());
    }
}
