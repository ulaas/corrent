fn main() {
    println!("cargo:rerun-if-changed=assets/icon/corrent.ico");
    // Embed the icon and version info so Explorer, the taskbar and file associations show it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/corrent.ico")
            .set("FileDescription", "Corrent BitTorrent client")
            .set("ProductName", "Corrent")
            .set("OriginalFilename", "corrent.exe");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
