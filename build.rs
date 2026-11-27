fn main() {
    // Embeds the Stellar icon so the exe, shortcuts and taskbar show it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/stellar.ico");
        res.set("ProductName", "Stellar");
        res.set("FileDescription", "Stellar cooling & lighting");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {e}");
        }
    }
}
