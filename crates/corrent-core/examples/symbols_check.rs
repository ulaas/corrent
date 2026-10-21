//! STABILITY.md O2 acceptance: the release profile's symbols resolve a crash report's offsets
//! to file:line. `cargo run --release -p corrent-core --example symbols_check`
fn main() {
    match corrent_core::symbols::self_check() {
        Ok(text) => println!("{text}\nSYMBOLS OK"),
        Err(text) => {
            println!("{text}\nSYMBOLS FAIL");
            std::process::exit(1);
        }
    }
}
