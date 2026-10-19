//! `corrent-mpvw`: a worker process around libmpv for the server. One job per process, so a
//! crashing or hanging decoder never takes the server with it.
//!
//! `corrent-mpvw probe <file|url>` prints one line of JSON (`ProbeResult`) or `{"error": "…"}`.
//! `corrent-mpvw encode <job.json>` runs an encode job, printing `{"pos": seconds}` lines and
//! finally `{"done": true}` or `{"error": "…"}`. Exit code 0 only on success.

use std::time::Duration;

fn fail(msg: &str) -> ! {
    println!("{}", serde_json::json!({ "error": msg }));
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(p) = std::env::var("CORRENT_LIBMPV") {
        corrent_mpv::Library::configure(Some(p.into()));
    }
    match args.first().map(String::as_str) {
        Some("probe") => {
            let Some(input) = args.get(1) else { fail("usage: corrent-mpvw probe <file>") };
            match corrent_mpv::probe(input, Duration::from_secs(30)) {
                Ok(r) => println!("{}", serde_json::to_string(&r).unwrap_or_default()),
                Err(e) => fail(&e.to_string()),
            }
        }
        Some("encode") => {
            let Some(path) = args.get(1) else { fail("usage: corrent-mpvw encode <job.json>") };
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| fail(&e.to_string()));
            let job: corrent_mpv::encode::EncodeJob =
                serde_json::from_str(&text).unwrap_or_else(|e| fail(&e.to_string()));
            let r = corrent_mpv::encode::run(&job, Duration::from_secs(6 * 3600), &mut |pos| {
                println!("{}", serde_json::json!({ "pos": pos }));
                true
            });
            match r {
                Ok(()) => println!("{}", serde_json::json!({ "done": true })),
                Err(e) => fail(&e.to_string()),
            }
        }
        Some("version") => match corrent_mpv::Library::get() {
            Ok(l) => println!("{}", serde_json::json!({ "path": l.path(), "api": l.version_string() })),
            Err(e) => fail(&e.to_string()),
        },
        _ => {
            eprintln!("usage: corrent-mpvw probe <file> | encode <job.json> | version");
            std::process::exit(2);
        }
    }
}
