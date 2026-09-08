use std::path::Path;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        // Verify the ICO exists before compiling
        let ico_path = if Path::new("assets/logo/ezicode.ico").exists() {
            "assets/logo/ezicode.ico"
        } else {
            "assets/logo/olova.ico"
        };
        if !Path::new(ico_path).exists() {
            eprintln!("warning: {} not found - taskbar icon will be missing", ico_path);
        } else {
            let mut res = winres::WindowsResource::new();
            // Set the main application icon (taskbar + title bar + .exe)
            res.set_icon(ico_path);
            res.set("ProductName", "ezicode");
            res.set("FileDescription", "ezicode - A native code editor built with GPUI");
            res.set("CompanyName", "ezicode");
            res.set("LegalCopyright", "MIT License");
            if let Err(e) = res.compile() {
                eprintln!("winres compile failed: {e}");
            } else {
                println!("cargo:warning=Embedded Windows resources from {}", ico_path);
            }
        }
    }

    // Re-run if the icon changes
    println!("cargo:rerun-if-changed=assets/logo/ezicode.ico");
    println!("cargo:rerun-if-changed=assets/logo/ezicode.png");
    println!("cargo:rerun-if-changed=assets/logo/olova.ico");
    println!("cargo:rerun-if-changed=assets/logo/olova.png");
    println!("cargo:rerun-if-changed=build.rs");
}
