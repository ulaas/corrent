fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/correntd.ico");
    // Embed the icon and version info so Explorer and the Services console show it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/correntd.ico")
            .set("FileDescription", "Corrent media server")
            .set("ProductName", "Corrent")
            .set("OriginalFilename", "correntd.exe");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
