//! Windows only: embed the app icon and version info (VERSIONINFO) into `iridium-decks.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `IRIDIUM_DECKS_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/iridium-decks.ico");
    println!("cargo:rerun-if-env-changed=IRIDIUM_DECKS_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/iridium-decks.ico")
        .set("ProductName", "Decks")
        .set("FileDescription", "Decks presentations")
        .set("CompanyName", "Iridium Technologies, Inc.")
        .set("LegalCopyright", "Copyright (c) the Decks authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "iridium-decks.exe")
        .set("InternalName", "iridium-decks");
    if let Err(e) = res.compile() {
        if std::env::var_os("IRIDIUM_DECKS_REQUIRE_WINRES").is_some() {
            eprintln!("embedding Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=iridium-decks.exe built without icon/version resources: {e}");
    }
}
