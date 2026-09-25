//! On-disk extension store: discovery, installation and removal.
//!
//! Layout (all under the platform config dir, next to `settings.json`):
//!
//! ```text
//! <config>/extensions/
//!     installed/
//!         <extension-id>/
//!             extension.toml
//!             themes/*.json          (theme extensions)
//!             icon_themes/*.json     (icon-theme extensions)
//!             languages/<lang>/config.toml
//!             extension.wasm         (compiled logic, optional)
//! ```
//!
//! This mirrors how Zed stores installed extensions, so the exact same folder
//! from a Zed extension repository works when copied in.

use std::path::{Path, PathBuf};

use super::manifest::ExtensionManifest;

/// Root directory that holds all extension state.
pub fn extensions_root() -> PathBuf {
    crate::settings::config_dir().join("extensions")
}

/// Directory holding one sub-folder per installed extension.
pub fn installed_dir() -> PathBuf {
    extensions_root().join("installed")
}

/// A single installed extension discovered on disk.
#[derive(Debug, Clone)]
pub struct InstalledExtension {
    pub manifest: ExtensionManifest,
    pub dir: PathBuf,
}

/// Which Zed-compatible capabilities an extension advertises. Used for badges
/// in the UI and to decide which subsystems to feed the extension into.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub themes: bool,
    pub icon_themes: bool,
    pub languages: bool,
    pub grammars: bool,
    pub language_servers: bool,
    /// A compiled `extension.wasm` component is present.
    pub wasm: bool,
}

impl InstalledExtension {
    pub fn themes_dir(&self) -> PathBuf {
        self.dir.join("themes")
    }

    pub fn icon_themes_dir(&self) -> PathBuf {
        self.dir.join("icon_themes")
    }

    pub fn languages_dir(&self) -> PathBuf {
        self.dir.join("languages")
    }

    pub fn wasm_path(&self) -> PathBuf {
        self.dir.join("extension.wasm")
    }

    /// Collect every `themes/*.json` file's contents.
    pub fn theme_files(&self) -> Vec<PathBuf> {
        json_files_in(&self.themes_dir())
    }

    pub fn icon_theme_files(&self) -> Vec<PathBuf> {
        json_files_in(&self.icon_themes_dir())
    }

    /// Each `languages/<lang>/config.toml` path.
    pub fn language_config_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let dir = self.languages_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let cfg = entry.path().join("config.toml");
                if cfg.is_file() {
                    out.push(cfg);
                }
            }
        }
        out.sort();
        out
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            themes: !self.theme_files().is_empty(),
            icon_themes: !self.icon_theme_files().is_empty(),
            languages: !self.language_config_files().is_empty(),
            grammars: !self.manifest.grammars.is_empty(),
            language_servers: !self.manifest.language_servers.is_empty(),
            wasm: self.wasm_path().is_file(),
        }
    }
}

fn json_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Read and parse the `extension.toml` in `dir`, returning the installed entry.
fn load_from_dir(dir: &Path) -> Option<InstalledExtension> {
    let manifest_path = dir.join("extension.toml");
    let text = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest = match ExtensionManifest::parse(&text) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "extension: failed to parse {}: {e}",
                manifest_path.display()
            );
            return None;
        }
    };
    Some(InstalledExtension {
        manifest,
        dir: dir.to_path_buf(),
    })
}

/// Scan the installed directory and return every valid extension, sorted by
/// display name. This performs disk I/O; callers that render at high frequency
/// should use [`super::installed`] which memoizes the result.
pub fn scan() -> Vec<InstalledExtension> {
    let mut out = Vec::new();
    let dir = installed_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(ext) = load_from_dir(&path) {
                    out.push(ext);
                }
            }
        }
    }
    out.sort_by(|a, b| {
        a.manifest
            .name
            .to_lowercase()
            .cmp(&b.manifest.name.to_lowercase())
    });
    out
}

/// Recursively copy `src` into `dst` (files + subdirectories), skipping any
/// version-control metadata so a cloned repo lands cleanly.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str() == Some(".git") {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Install (or reinstall) an extension from a local directory that contains an
/// `extension.toml`. Returns the id it was installed under.
pub fn install_from_path(src: &Path) -> Result<String, String> {
    let manifest_path = src.join("extension.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("no extension.toml in {}: {e}", src.display()))?;
    let manifest =
        ExtensionManifest::parse(&text).map_err(|e| format!("invalid extension.toml: {e}"))?;
    let id = manifest.effective_id();
    if id.is_empty() {
        return Err("extension.toml is missing an `id`".to_string());
    }
    let dest = installed_dir().join(&id);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("could not replace existing: {e}"))?;
    }
    copy_dir_recursive(src, &dest).map_err(|e| format!("copy failed: {e}"))?;
    Ok(id)
}

/// Clone a Zed extension from a Git URL and install it. Shells out to the
/// system `git` (already a hard dependency of the app's source-control
/// features), so no extra native git library is pulled in.
///
/// Accepts a full URL (`https://github.com/owner/repo`) or the `owner/repo`
/// shorthand, which is expanded to a GitHub HTTPS URL.
pub fn install_from_git(url_or_slug: &str) -> Result<String, String> {
    let url = normalize_git_url(url_or_slug);
    let tmp = extensions_root()
        .join(".tmp")
        .join(format!("clone-{}", now_millis()));
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }

    let output = std::process::Command::new("git")
        .args(["clone", "--depth", "1", url.as_str()])
        .arg(&tmp)
        .output()
        .map_err(|e| format!("failed to run git (is it installed?): {e}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let result = install_from_path(&tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Remove an installed extension by id. Returns whether anything was removed.
pub fn uninstall(id: &str) -> Result<bool, String> {
    // Guard against path traversal in the id.
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!("invalid extension id: {id:?}"));
    }
    let dir = installed_dir().join(id);
    if !dir.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("remove failed: {e}"))?;
    Ok(true)
}

fn normalize_git_url(input: &str) -> String {
    let s = input.trim();
    if s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("git@")
        || s.starts_with("ssh://")
        || s.ends_with(".git")
    {
        return s.to_string();
    }
    // `owner/repo` shorthand -> GitHub.
    if s.split('/').count() == 2 && !s.contains(' ') {
        return format!("https://github.com/{s}");
    }
    s.to_string()
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_git_urls() {
        assert_eq!(
            normalize_git_url("owner/repo"),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            normalize_git_url("https://github.com/a/b"),
            "https://github.com/a/b"
        );
        assert_eq!(
            normalize_git_url("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
    }

    #[test]
    fn rejects_bad_uninstall_ids() {
        assert!(uninstall("../etc").is_err());
        assert!(uninstall("a/b").is_err());
        assert!(uninstall("").is_err());
    }

    #[test]
    fn install_from_path_into_explicit_dir_roundtrips() {
        // Exercise copy + capability detection on an explicit directory,
        // avoiding any mutation of process-wide env / the real config dir.
        let base = std::env::temp_dir().join(format!("ezicode-ext-test-{}", now_millis()));
        let src = base.join("src-ext");
        let dst = base.join("installed").join("demo");
        std::fs::create_dir_all(src.join("themes")).unwrap();
        std::fs::write(
            src.join("extension.toml"),
            "id = \"demo\"\nname = \"Demo\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(src.join("themes").join("demo.json"), "{\"themes\":[]}").unwrap();

        copy_dir_recursive(&src, &dst).expect("copy");
        let loaded = load_from_dir(&dst).expect("load");
        assert_eq!(loaded.manifest.id, "demo");
        assert_eq!(loaded.manifest.version, "1.0.0");
        assert!(loaded.capabilities().themes);
        assert!(!loaded.capabilities().languages);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn copy_skips_git_metadata() {
        let base = std::env::temp_dir().join(format!("ezicode-ext-git-{}", now_millis()));
        let src = base.join("src");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join(".git").join("HEAD"), "ref: x").unwrap();
        std::fs::write(src.join("extension.toml"), "id=\"a\"\nname=\"A\"\n").unwrap();
        let dst = base.join("dst");
        copy_dir_recursive(&src, &dst).unwrap();
        assert!(dst.join("extension.toml").exists());
        assert!(!dst.join(".git").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
