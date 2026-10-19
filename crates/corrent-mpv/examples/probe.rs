//! `cargo run -p corrent-mpv --example probe -- <file or URL>`: prints what mpv sees as JSON.
fn main() {
    let Some(input) = std::env::args().nth(1) else {
        eprintln!("usage: probe <file or URL>");
        std::process::exit(2);
    };
    match corrent_mpv::probe(&input, std::time::Duration::from_secs(30)) {
        Ok(r) => println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
