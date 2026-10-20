//! STABILITY.md D8: end the harness (`src/bin/corrent-harness.rs`) at a write, start again and
//! check that nothing it saw saved is lost and nothing downloads again.
//!
//! The gate runs a few kill points and the D6b rebuild; the whole matrix (every reached point, up
//! to 3 occurrences each, in the workload and in a start that rebuilds a lost list) is ignored by
//! default:
//! `cargo test -p corrent-core --features testkit --test durability -- --ignored --nocapture`.
// Test tooling: failing loudly is the point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HARNESS: &str = env!("CARGO_BIN_EXE_corrent-harness");

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("corrent-durability-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Outcome {
    killed: bool,
    ok: bool,
    log: String,
}

/// Run the harness (output captured: its error logs are expected and must not reach the
/// gate's output), at most 3 minutes.
fn harness(mode: &str, dir: &Path, env: &[(&str, String)]) -> Outcome {
    let mut cmd = Command::new(HARNESS);
    cmd.arg(mode)
        .arg(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("CORRENT_DEV_KILL_AT")
        .env_remove("CORRENT_DEV_KILL_LOG");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    // Drain the pipes on threads so a chatty child never blocks.
    let drain = |r: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut r) = r {
                let mut b = Vec::new();
                let _ = r.read_to_end(&mut b);
                s = String::from_utf8_lossy(&b).into_owned();
            }
            s
        })
    };
    let out = drain(child.stdout.take().map(|r| Box::new(r) as Box<dyn std::io::Read + Send>));
    let err = drain(child.stderr.take().map(|r| Box::new(r) as Box<dyn std::io::Read + Send>));
    let deadline = Instant::now() + Duration::from_secs(180);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let log = format!("{}{}", out.join().unwrap_or_default(), err.join().unwrap_or_default());
    #[cfg(windows)]
    let killed = status.code() == Some(75);
    #[cfg(unix)]
    let killed = std::os::unix::process::ExitStatusExt::signal(&status) == Some(6);
    Outcome { killed, ok: status.success(), log }
}

#[derive(Debug, PartialEq)]
enum Result {
    Survived,
    /// The run finished without reaching the point (timing): not a failure.
    Unreached,
    Lost(String),
}

/// Lose the engine's list and its copies (`garbage` or `missing`).
fn damage(dir: &Path, how: &str) {
    let ok = Command::new(HARNESS).arg("damage").arg(dir).arg(how).output().unwrap().status.success();
    assert!(ok, "damage {how}");
}

/// Run the workload, lose the list, kill the start that rebuilds it at the `n`-th `op:phase`,
/// then verify.
fn kill_rebuild_and_verify(name: &str, point: &str, n: u32) -> Result {
    let dir = scratch(name);
    let run = harness("run", &dir, &[]);
    if !run.ok {
        return Result::Lost(format!("the run failed:\n{}", run.log));
    }
    damage(&dir, "garbage");
    let start = harness("verify", &dir, &[("CORRENT_DEV_KILL_AT", format!("{point}:{n}"))]);
    if !start.killed {
        let _ = std::fs::remove_dir_all(&dir);
        return if start.ok { Result::Unreached } else { Result::Lost(format!("the start failed:\n{}", start.log)) };
    }
    let check = harness("verify", &dir, &[]);
    if check.ok {
        let _ = std::fs::remove_dir_all(&dir);
        Result::Survived
    } else {
        Result::Lost(format!("rebuild killed at {point}:{n}; profile kept in {}\n{}", dir.display(), check.log))
    }
}

/// Kill the run at the `n`-th `op:phase`, then verify.
fn kill_and_verify(name: &str, point: &str, n: u32) -> Result {
    let dir = scratch(name);
    let run = harness("run", &dir, &[("CORRENT_DEV_KILL_AT", format!("{point}:{n}"))]);
    if !run.killed {
        let _ = std::fs::remove_dir_all(&dir);
        return if run.ok { Result::Unreached } else { Result::Lost(format!("the run failed:\n{}", run.log)) };
    }
    let check = harness("verify", &dir, &[]);
    if check.ok {
        let _ = std::fs::remove_dir_all(&dir);
        Result::Survived
    } else {
        // Kept for a look.
        Result::Lost(format!("killed at {point}:{n}; profile kept in {}\n{}", dir.display(), check.log))
    }
}

#[test]
fn a_few_kills_during_saves_lose_nothing() {
    // Deterministic points: the first store temp file, the 5th engine database temp file (an add),
    // the second settings rename (with a fresh .bak), the re-add note, a restore half done.
    for (i, (point, n)) in [
        ("store:after_tmp", 1),
        ("engine-session:after_tmp", 5),
        ("settings:after_rename", 2),
        ("readd-note:after_tmp", 1),
        ("restore:after_keep", 2),
    ]
    .into_iter()
    .enumerate()
    {
        let r = kill_and_verify(&format!("smoke{i}"), point, n);
        assert_eq!(r, Result::Survived, "{point}:{n}");
    }
}

/// D6b: a lost list is rebuilt with nothing lost, also when the rebuild itself is cut off.
#[test]
fn a_rebuilt_session_database_loses_nothing() {
    let dir = scratch("rebuild");
    let run = harness("run", &dir, &[]);
    assert!(run.ok, "the run failed:\n{}", run.log);
    for how in ["garbage", "missing"] {
        damage(&dir, how);
        let check = harness("verify", &dir, &[]);
        assert!(check.ok, "{how}:\n{}", check.log);
        assert!(check.log.contains("rebuilt from the torrent files"), "{how}: no rebuild:\n{}", check.log);
    }
    for (how, point) in [("garbage", "session-recover:after_tmp:1"), ("missing", "session-recover:after_rename:1")] {
        damage(&dir, how);
        let killed = harness("verify", &dir, &[("CORRENT_DEV_KILL_AT", point.into())]);
        assert!(killed.killed, "{point} not reached:\n{}", killed.log);
        let check = harness("verify", &dir, &[]);
        assert!(check.ok, "after a kill at {point}:\n{}", check.log);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Which points one pass reaches, and how often: `run` alone, or a start that rebuilds.
fn count_points(rebuild: bool) -> BTreeMap<String, u32> {
    let dir = scratch(if rebuild { "count-rebuild" } else { "count" });
    let log = dir.join("points.log");
    let p = dir.join("p");
    let env = [("CORRENT_DEV_KILL_LOG", log.display().to_string())];
    let counted = if rebuild {
        let run = harness("run", &p, &[]);
        assert!(run.ok, "the run failed:\n{}", run.log);
        damage(&p, "garbage");
        harness("verify", &p, &env)
    } else {
        harness("run", &p, &env)
    };
    assert!(counted.ok, "the counting pass failed:\n{}", counted.log);
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for l in std::fs::read_to_string(&log).unwrap().lines() {
        *counts.entry(l.to_string()).or_default() += 1;
    }
    let _ = std::fs::remove_dir_all(&dir);
    counts
}

#[test]
#[ignore = "the full kill matrix takes several minutes"]
fn kills_at_every_write_point_lose_nothing() {
    let per_pair: u32 = std::env::var("CORRENT_KILL_MAX_PER_PAIR").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    // First, middle and last occurrence (later ones may not recur: saves are debounced).
    let mut jobs: Vec<(bool, String, u32)> = Vec::new();
    let mut points = 0;
    for rebuild in [false, true] {
        let counts = count_points(rebuild);
        points += counts.len();
        for (point, &max) in &counts {
            let mut ns = vec![1, max.div_ceil(2), max];
            ns.dedup();
            ns.truncate(per_pair as usize);
            jobs.extend(ns.into_iter().map(|n| (rebuild, point.clone(), n)));
        }
    }
    eprintln!("{points} points, {} kills", jobs.len());
    let jobs = std::sync::Mutex::new(jobs.into_iter().enumerate().collect::<Vec<_>>());
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                loop {
                    let Some((i, (rebuild, point, n))) = jobs.lock().unwrap().pop() else { break };
                    let r = if rebuild {
                        kill_rebuild_and_verify(&format!("r{i}"), &point, n)
                    } else {
                        kill_and_verify(&format!("m{i}"), &point, n)
                    };
                    let point = if rebuild { format!("rebuild/{point}") } else { point };
                    eprintln!(
                        "{point}:{n} -> {}",
                        match &r {
                            Result::Survived => "survived",
                            Result::Unreached => "unreached",
                            Result::Lost(_) => "LOST",
                        }
                    );
                    results.lock().unwrap().push((point, n, r));
                }
            });
        }
    });
    let results = results.into_inner().unwrap();
    let lost: Vec<String> = results
        .iter()
        .filter_map(|(p, n, r)| if let Result::Lost(e) = r { Some(format!("{p}:{n}: {e}")) } else { None })
        .collect();
    let unreached = results.iter().filter(|r| r.2 == Result::Unreached).count();
    eprintln!(
        "{} kills: {} survived, {unreached} unreached, {} lost",
        results.len(),
        results.len() - unreached - lost.len(),
        lost.len()
    );
    assert!(lost.is_empty(), "{}", lost.join("\n\n"));
}
