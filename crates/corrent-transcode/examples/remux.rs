//! Writes a Matroska file's direct-stream HLS to a folder (for checking players by hand):
//! `cargo run -p corrent-transcode --example remux -- <in.mkv> <out dir> [audio track | --index]`
// A developer tool, not shipped.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, out, rest @ ..] = args.as_slice() else {
        eprintln!("usage: remux <in.mkv> <out dir> [audio track]");
        std::process::exit(2);
    };
    if rest.first().map(String::as_str) == Some("--index") {
        let h = corrent_transcode::remux::mkv::open(std::path::Path::new(input)).unwrap();
        for c in &h.clusters {
            println!(
                "{:>10} {:>9} {:>8.3} {}",
                c.offset,
                c.size,
                c.time_ns as f64 / 1e9,
                if c.key { "key" } else { "" }
            );
        }
        return;
    }
    let audio = rest.first().map_or(Some(1), |a| a.parse().ok().filter(|n: &i64| *n > 0));
    let r = corrent_transcode::remux::prepare(std::path::Path::new(input), audio).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let out = std::path::Path::new(out);
    std::fs::create_dir_all(out).unwrap();
    std::fs::write(out.join("index.m3u8"), r.playlist("")).unwrap();
    std::fs::write(out.join("init.mp4"), r.init()).unwrap();
    for i in 0..r.segments.len() {
        std::fs::write(out.join(format!("{i}.m4s")), r.segment(i).unwrap()).unwrap();
    }
    println!("{} segments, codecs {}", r.segments.len(), r.codecs());
}
