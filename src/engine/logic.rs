//! Pure decision logic used by the actor (no I/O, no session): state mapping, queue planning,
//! seeding limits and the alt-speed schedule. Kept separate so it is fully unit-tested.

use std::time::Duration;

use super::types::{InfoHash, TorrentState, TorrentView};
use crate::config::{Queue, Schedule, Seeding};
use crate::store::{PauseReason, TorrentMeta};

/// Engine-level state of a torrent, independent of librqbit's types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineStatus {
    Initializing,
    Live,
    Paused,
    Error,
}

pub struct StateInput {
    pub status: EngineStatus,
    pub finished: bool,
    pub busy: bool,
    pub stalled: bool,
    pub pause_reason: Option<PauseReason>,
}

pub fn map_state(i: &StateInput) -> TorrentState {
    match i.status {
        _ if i.busy => TorrentState::Checking,
        EngineStatus::Error => TorrentState::Error,
        EngineStatus::Initializing => TorrentState::Checking,
        EngineStatus::Paused => match i.pause_reason {
            Some(PauseReason::Queue) => TorrentState::Queued,
            _ if i.finished => TorrentState::Completed,
            _ => TorrentState::Paused,
        },
        EngineStatus::Live if i.finished => TorrentState::Seeding,
        EngineStatus::Live if i.stalled => TorrentState::Stalled,
        EngineStatus::Live => TorrentState::Downloading,
    }
}

/// Whether the alternative-speed schedule is active at `minutes` past midnight on `weekday`
/// (0 = Monday). Windows that cross midnight (e.g. 22:00–06:00) are supported.
pub fn in_schedule(sch: &Schedule, minutes: u16, weekday: u8) -> bool {
    if !sch.enabled || sch.days & (1 << weekday) == 0 {
        return false;
    }
    if sch.from_minutes <= sch.to_minutes {
        minutes >= sch.from_minutes && minutes < sch.to_minutes
    } else {
        minutes >= sch.from_minutes || minutes < sch.to_minutes
    }
}

/// One torrent as seen by the queue planner.
pub struct QueueItem<'a> {
    pub view: &'a TorrentView,
    pub meta: &'a TorrentMeta,
    /// How long the torrent has been below the slow thresholds (zero if not slow now).
    pub slow_for: Duration,
    /// Time since the queue (or user) last resumed it; `None` if never.
    pub since_resume: Option<Duration>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct QueuePlan {
    pub pause: Vec<InfoHash>,
    pub resume: Vec<InfoHash>,
}

pub const SLOW_GRACE: Duration = Duration::from_secs(60);

pub fn is_slow_now(v: &TorrentView, q: &Queue) -> bool {
    is_running(v.state) && v.down_bps < q.slow_down_kib as u64 * 1024 && v.up_bps < q.slow_up_kib as u64 * 1024
}

fn is_running(s: TorrentState) -> bool {
    matches!(s, TorrentState::Downloading | TorrentState::Stalled | TorrentState::Seeding | TorrentState::Metadata)
}

/// Decide which torrents the queue should pause or resume. `items` may be in any order.
pub fn plan_queue(q: &Queue, mut items: Vec<QueueItem<'_>>) -> QueuePlan {
    let mut plan = QueuePlan::default();
    if !q.enabled {
        return plan;
    }
    items.retain(|i| {
        let v = i.view;
        !v.pending
            && v.busy.is_none()
            && v.has_metadata
            && !matches!(v.state, TorrentState::Error | TorrentState::Checking)
            && !i.meta.force_started
            // User- and limit-paused torrents are outside the queue's control.
            && !matches!(i.meta.pause_reason, Some(PauseReason::User) | Some(PauseReason::SeedLimit))
    });
    items.sort_by_key(|i| i.view.queue_pos);
    let (mut dl, mut up, mut total) = (0u32, 0u32, 0u32);
    for i in items {
        let v = i.view;
        let running = is_running(v.state);
        let finished = v.completed_at.is_some() && v.done >= v.size;
        let settled = i.since_resume.is_none_or(|d| d >= SLOW_GRACE);
        let slow = q.ignore_slow && is_slow_now(v, q) && i.slow_for >= SLOW_GRACE && settled;
        if running && slow {
            continue; // inactive torrents don't occupy a slot
        }
        let fits = total < q.max_active_total
            && if finished { up < q.max_active_uploads } else { dl < q.max_active_downloads };
        if fits {
            total += 1;
            if finished {
                up += 1
            } else {
                dl += 1
            }
            if !running && i.meta.pause_reason == Some(PauseReason::Queue) {
                plan.resume.push(v.hash);
            }
        } else if running {
            plan.pause.push(v.hash);
        }
    }
    plan
}

/// Why a seeding torrent should stop, if any limit is reached.
pub fn seed_limit_reason(v: &TorrentView, m: &TorrentMeta, s: &Seeding) -> Option<String> {
    if v.state != TorrentState::Seeding || m.seed_limit_exempt || m.force_started {
        return None;
    }
    let ratio_limit = m.seed_ratio_override.or(s.ratio_enabled.then_some(s.ratio));
    let time_limit = m.seed_time_override_min.or(s.time_enabled.then_some(s.time_minutes));
    if let Some(r) = ratio_limit
        && r > 0.0
        && v.ratio >= r
    {
        return Some(format!("reached ratio {r:.2}"));
    }
    if let Some(t) = time_limit
        && t > 0
        && m.seeding_seconds >= t as u64 * 60
    {
        return Some(format!("seeded for {}", crate::util::fmt_duration(Duration::from_secs(t as u64 * 60))));
    }
    if s.inactive_enabled && s.inactive_minutes > 0 && m.inactive_seeding_secs >= s.inactive_minutes as u64 * 60 {
        return Some("was inactive".to_string());
    }
    None
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn view(n: u8, state: TorrentState) -> TorrentView {
        TorrentView {
            hash: InfoHash([n; 20]),
            name: format!("t{n}"),
            state,
            size: 100,
            has_metadata: true,
            queue_pos: n as u32,
            ..Default::default()
        }
    }

    fn input(status: EngineStatus) -> StateInput {
        StateInput { status, finished: false, busy: false, stalled: false, pause_reason: None }
    }

    #[test]
    fn state_mapping() {
        assert_eq!(map_state(&input(EngineStatus::Live)), TorrentState::Downloading);
        assert_eq!(map_state(&StateInput { finished: true, ..input(EngineStatus::Live) }), TorrentState::Seeding);
        assert_eq!(map_state(&StateInput { stalled: true, ..input(EngineStatus::Live) }), TorrentState::Stalled);
        assert_eq!(map_state(&input(EngineStatus::Initializing)), TorrentState::Checking);
        assert_eq!(map_state(&input(EngineStatus::Error)), TorrentState::Error);
        assert_eq!(map_state(&input(EngineStatus::Paused)), TorrentState::Paused);
        assert_eq!(map_state(&StateInput { finished: true, ..input(EngineStatus::Paused) }), TorrentState::Completed);
        assert_eq!(
            map_state(&StateInput {
                pause_reason: Some(PauseReason::Queue),
                finished: true,
                ..input(EngineStatus::Paused)
            }),
            TorrentState::Queued
        );
        assert_eq!(map_state(&StateInput { busy: true, ..input(EngineStatus::Error) }), TorrentState::Checking);
    }

    #[test]
    fn schedule_windows() {
        let mut s = Schedule { enabled: true, days: 0b0011111, from_minutes: 8 * 60, to_minutes: 18 * 60 };
        assert!(in_schedule(&s, 9 * 60, 0));
        assert!(!in_schedule(&s, 7 * 60, 0));
        assert!(!in_schedule(&s, 18 * 60, 0), "end is exclusive");
        assert!(!in_schedule(&s, 9 * 60, 5), "Saturday not selected");
        s.from_minutes = 22 * 60;
        s.to_minutes = 6 * 60;
        assert!(in_schedule(&s, 23 * 60, 1));
        assert!(in_schedule(&s, 60, 1));
        assert!(!in_schedule(&s, 12 * 60, 1));
        s.enabled = false;
        assert!(!in_schedule(&s, 23 * 60, 1));
    }

    fn q(dl: u32, up: u32, total: u32) -> Queue {
        Queue {
            enabled: true,
            max_active_downloads: dl,
            max_active_uploads: up,
            max_active_total: total,
            ..Default::default()
        }
    }

    fn items<'a>(views: &'a [TorrentView], metas: &'a [TorrentMeta]) -> Vec<QueueItem<'a>> {
        views
            .iter()
            .zip(metas)
            .map(|(view, meta)| QueueItem { view, meta, slow_for: Duration::ZERO, since_resume: None })
            .collect()
    }

    #[test]
    fn queue_pauses_extra_downloads_in_order() {
        let views: Vec<_> = (0..4).map(|i| view(i, TorrentState::Downloading)).collect();
        let metas = vec![TorrentMeta::default(); 4];
        let plan = plan_queue(&q(2, 5, 10), items(&views, &metas));
        assert_eq!(plan.pause, vec![InfoHash([2; 20]), InfoHash([3; 20])]);
        assert!(plan.resume.is_empty());
    }

    #[test]
    fn queue_resumes_queued_when_slot_frees() {
        let mut views: Vec<_> = (0..3).map(|i| view(i, TorrentState::Queued)).collect();
        views[0].state = TorrentState::Downloading;
        let metas = vec![
            TorrentMeta::default(),
            TorrentMeta { pause_reason: Some(PauseReason::Queue), ..Default::default() },
            TorrentMeta { pause_reason: Some(PauseReason::Queue), ..Default::default() },
        ];
        let plan = plan_queue(&q(2, 5, 10), items(&views, &metas));
        assert_eq!(plan.resume, vec![InfoHash([1; 20])]);
        assert!(plan.pause.is_empty());
    }

    #[test]
    fn queue_ignores_user_paused_forced_and_errored() {
        let mut views: Vec<_> = (0..4).map(|i| view(i, TorrentState::Downloading)).collect();
        views[3].state = TorrentState::Error;
        let metas = vec![
            TorrentMeta { force_started: true, ..Default::default() },
            TorrentMeta { pause_reason: Some(PauseReason::User), ..Default::default() },
            TorrentMeta::default(),
            TorrentMeta::default(),
        ];
        let plan = plan_queue(&q(1, 5, 10), items(&views, &metas));
        assert!(plan.pause.is_empty(), "only #2 is managed and it fits: {plan:?}");
    }

    #[test]
    fn queue_seeding_uses_upload_slots() {
        let mut views: Vec<_> = (0..3).map(|i| view(i, TorrentState::Seeding)).collect();
        for v in &mut views {
            v.completed_at = Some(1);
            v.done = 100;
        }
        let metas = vec![TorrentMeta::default(); 3];
        let plan = plan_queue(&q(5, 1, 10), items(&views, &metas));
        assert_eq!(plan.pause.len(), 2);
    }

    #[test]
    fn queue_total_limit_applies() {
        let views: Vec<_> = (0..3).map(|i| view(i, TorrentState::Downloading)).collect();
        let metas = vec![TorrentMeta::default(); 3];
        let plan = plan_queue(&q(10, 10, 1), items(&views, &metas));
        assert_eq!(plan.pause.len(), 2);
    }

    #[test]
    fn queue_slow_needs_grace_period() {
        let views: Vec<_> = (0..2).map(|i| view(i, TorrentState::Downloading)).collect();
        let metas = vec![TorrentMeta::default(); 2];
        let mut queue = q(1, 5, 10);
        queue.ignore_slow = true;
        // Slow but only briefly: still occupies the slot.
        let mut it = items(&views, &metas);
        it[0].slow_for = Duration::from_secs(5);
        assert_eq!(plan_queue(&queue, it).pause, vec![InfoHash([1; 20])]);
        // Slow for long enough: frees the slot.
        let mut it = items(&views, &metas);
        it[0].slow_for = Duration::from_secs(120);
        assert!(plan_queue(&queue, it).pause.is_empty());
        // Just resumed: not considered slow yet.
        let mut it = items(&views, &metas);
        it[0].slow_for = Duration::from_secs(120);
        it[0].since_resume = Some(Duration::from_secs(10));
        assert_eq!(plan_queue(&queue, it).pause.len(), 1);
    }

    #[test]
    fn queue_disabled_does_nothing() {
        let views: Vec<_> = (0..3).map(|i| view(i, TorrentState::Downloading)).collect();
        let metas = vec![TorrentMeta::default(); 3];
        let mut queue = q(1, 1, 1);
        queue.enabled = false;
        assert_eq!(plan_queue(&queue, items(&views, &metas)), QueuePlan::default());
    }

    #[test]
    fn seed_limits() {
        let mut v = view(1, TorrentState::Seeding);
        let mut m = TorrentMeta::default();
        let mut s = Seeding::default();
        assert_eq!(seed_limit_reason(&v, &m, &s), None);
        s.ratio_enabled = true;
        s.ratio = 1.0;
        v.ratio = 1.2;
        assert!(seed_limit_reason(&v, &m, &s).unwrap().contains("ratio"));
        m.seed_ratio_override = Some(5.0);
        assert_eq!(seed_limit_reason(&v, &m, &s), None, "per-torrent override wins");
        m.seed_ratio_override = None;
        m.seed_limit_exempt = true;
        assert_eq!(seed_limit_reason(&v, &m, &s), None, "user resumed after a limit");
        m.seed_limit_exempt = false;
        s.ratio_enabled = false;
        s.time_enabled = true;
        s.time_minutes = 10;
        m.seeding_seconds = 601;
        assert!(seed_limit_reason(&v, &m, &s).unwrap().contains("seeded"));
        s.time_enabled = false;
        s.inactive_enabled = true;
        s.inactive_minutes = 1;
        m.inactive_seeding_secs = 59;
        assert_eq!(seed_limit_reason(&v, &m, &s), None);
        m.inactive_seeding_secs = 60;
        assert_eq!(seed_limit_reason(&v, &m, &s).as_deref(), Some("was inactive"));
        v.state = TorrentState::Paused;
        assert_eq!(seed_limit_reason(&v, &m, &s), None, "only seeding torrents");
    }
}
