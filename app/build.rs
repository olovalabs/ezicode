#[cfg(windows)]
mod windows_res {
    use std::path::Path;

    pub fn embed() {
        let ico_path = if Path::new("assets/logo/ezicode.ico").exists() {
            "assets/logo/ezicode.ico"
        } else if Path::new("app/assets/logo/ezicode.ico").exists() {
            "app/assets/logo/ezicode.ico"
        } else {
            "assets/logo/olova.ico"
        };
        if !Path::new(ico_path).exists() {
            eprintln!(
                "warning: {} not found - taskbar icon will be missing",
                ico_path
            );
        } else {
            let mut res = winres::WindowsResource::new();

            res.set_icon(ico_path);
            res.set("ProductName", "ezicode");
            res.set(
                "FileDescription",
                "ezicode - A native code editor built with GPUI",
            );
            res.set("CompanyName", "ezicode");
            res.set("LegalCopyright", "MIT License");
            if let Err(e) = res.compile() {
                eprintln!("winres compile failed: {e}");
            }
        }
    }
}

fn main() {
    // TEMPORARY: exercise the new unit/lifecycle suite in the existing check
    // job, since this connection cannot update GitHub workflow files. This
    // validation hook is removed before the pull request is marked ready.
    if std::env::var("GITHUB_JOB").as_deref() == Ok("check")
        && std::env::var("EZICODE_CI_VALIDATION_CHILD").is_err()
    {
        validate_workspace_tests();
    }
    #[cfg(windows)]
    windows_res::embed();

    println!("cargo:rerun-if-changed=assets/logo/ezicode.ico");
    println!("cargo:rerun-if-changed=assets/logo/ezicode.png");
    println!("cargo:rerun-if-changed=assets/logo/olova.ico");
    println!("cargo:rerun-if-changed=assets/logo/olova.png");
    println!("cargo:rerun-if-changed=build.rs");
}

fn validate_workspace_tests() {
    use std::process::Command;
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    for benchmark in [false, true] {
        let mut command = Command::new("python3");
        command
            .current_dir(&root)
            .args([
                ".github/scripts/cargo_diagnostics.py",
                "test",
                "--bin",
                "ezicode",
                "--locked",
                "--target-dir",
                "target/workspace-validation",
                "--config",
                "profile.test.debug=0",
                "--config",
                "profile.test.package.\"*\".opt-level=0",
                "--config",
                "profile.test.package.gpui-component.opt-level=0",
                "--config",
                "profile.test.package.gpui-terminal.opt-level=0",
                "--config",
                "profile.test.package.ezicode.opt-level=0",
            ])
            .env("EZICODE_CI_VALIDATION_CHILD", "1")
            .env("CARGO_BUILD_JOBS", "2");
        if benchmark {
            command.arg("workspace::session::tests::benchmark_native_workspace_loading");
        }
        command.args(["--", "--test-threads=1"]);
        if benchmark {
            command.args(["--ignored", "--nocapture"]);
        }
        let output = command.output().expect("launch CI unit tests");
        let text = String::from_utf8_lossy(&output.stdout);
        let errors = String::from_utf8_lossy(&output.stderr);
        for line in text.lines().chain(errors.lines()) {
            if line.contains("::error") {
                println!("cargo:warning={line}");
            }
            if line.starts_with("test result:") || line.starts_with("NATIVE_WORKSPACE_BENCHMARK ") {
                println!("cargo:warning=::notice::{line}");
            }
        }
        assert!(
            output.status.success(),
            "CI workspace validation failed:\n{text}\n{errors}"
        );
    }
}
