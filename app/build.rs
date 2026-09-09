use std::path::Path;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {

        let ico_path = if Path::new("assets/logo/ezicode.ico").exists() {
            "assets/logo/ezicode.ico"
        } else if Path::new("app/assets/logo/ezicode.ico").exists() {
            "app/assets/logo/ezicode.ico"
        } else {
            "assets/logo/olova.ico"
        };
        if !Path::new(ico_path).exists() {
            eprintln!("warning: {} not found - taskbar icon will be missing", ico_path);
        } else {
            let mut res = winres::WindowsResource::new();

            res.set_icon(ico_path);
            res.set("ProductName", "ezicode");
            res.set("FileDescription", "ezicode - A native code editor built with GPUI");
            res.set("CompanyName", "ezicode");
            res.set("LegalCopyright", "MIT License");
            if let Err(e) = res.compile() {
                eprintln!("winres compile failed: {e}");
            }
        }
    }

    println!("cargo:rerun-if-changed=assets/logo/ezicode.ico");
    println!("cargo:rerun-if-changed=assets/logo/ezicode.png");
    println!("cargo:rerun-if-changed=assets/logo/olova.ico");
    println!("cargo:rerun-if-changed=assets/logo/olova.png");
    println!("cargo:rerun-if-changed=build.rs");
}
