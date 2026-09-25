//! Linux desktop integration: the taskbar/dock icon and the desktop entry.
//!
//! On Windows the taskbar icon is baked into the executable — `build.rs` links
//! `assets/logo/ezicode.ico` as a resource, so the shell always has something to
//! show. Linux has no equivalent of that. A panel or compositor resolves an
//! icon from two places instead:
//!
//! 1. the **desktop entry** in an XDG data directory, whose `Icon=` key names an
//!    icon in the **hicolor theme**, and
//! 2. for X11 panels that do no desktop-entry lookup at all (tint2, i3bar,
//!    plain xfce4-panel, ...), the **`_NET_WM_ICON`** property on the window.
//!
//! Carrying the PNG in our own embedded assets satisfies neither, which is why
//! the taskbar logo showed up on Windows and stayed blank on Linux. So we write
//! both halves for the current user on startup, before the first window exists:
//!
//! * [`preflight`] — handles `--install-desktop` and otherwise installs the
//!   entry plus every icon size a panel is likely to ask for.
//! * [`apply_window_icon`] — sets `_NET_WM_ICON` when the session turns out to
//!   be X11.
//!
//! Everything here is best effort. A read-only or unusual `$HOME`, a missing
//! icon-cache tool, a PNG we cannot decode or an X11 connection that will not
//! open must never stop the editor from starting.
//!
//! The install is user-scoped (`$XDG_DATA_HOME`) on purpose: writing to
//! `/usr/share` needs root and is the packaging job anyway. `ezicode.desktop`
//! in the repository root documents what a real system-wide install must ship.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Must match `WindowOptions::app_id` in `main.rs` and `StartupWMClass` in the
/// desktop entry below. KWin, GNOME's dash and Mutter match the window's
/// `app_id`/`WM_CLASS` against the desktop file name, so all three strings
/// agreeing is what makes the logo appear.
const APP_ID: &str = "ezicode";

const APP_NAME: &str = "ezicode";
const GENERIC_NAME: &str = "Code Editor";
const COMMENT: &str = "Fast, modern code editor powered by GPUI";

/// Sizes panels, launchers and icon-theme lookups actually ask for, descending.
/// The hicolor spec keys icon files by exact pixel size, so a theme that only
/// ships 1024x1024 fails a 32x32 request on toolkits that do a strict lookup.
const ICON_SIZES: &[u32] = &[512, 256, 128, 64, 48, 32, 24, 16];

/// A subset of [`ICON_SIZES`] for `_NET_WM_ICON`, which is a single property
/// holding every size back to back. 256 is the largest a taskbar button is ever
/// drawn at; above that we would just bloat the property.
const WINDOW_ICON_SIZES: &[u32] = &[256, 128, 64, 48, 32, 24, 16];

/// Runs before the window is created: handles the CLI and installs the desktop
/// entry and icon theme. Returns `true` when the process should exit without
/// starting the editor (the `--install-desktop` flag).
pub fn preflight() -> bool {
    let install_only = std::env::args()
        .skip(1)
        .any(|a| a == "--install-desktop" || a == "--install");
    let changed = install();

    if install_only {
        refresh_caches();
        if changed {
            println!("[linux] installed {APP_ID}.desktop and icon theme");
        } else {
            println!("[linux] {APP_ID} desktop entry and icons are already up to date");
        }
        return true;
    }
    false
}

/// Writes the desktop entry and the icon theme files, returning whether anything
/// was actually written.
pub fn install() -> bool {
    let Some(png) = icon_source() else {
        eprintln!("[linux] no embedded logo found; taskbar icon will be missing");
        return false;
    };

    let data_home = data_home();
    let stamp_path = stamp_path(&data_home);

    // Decoding and resampling a 1024x1024 logo on every launch just to
    // rediscover that nothing changed would cost more than the whole window
    // setup, so a cheap fingerprint short-circuits it. The existence check
    // still runs, so a user who deletes the icon gets it back.
    let stamp = format!("{APP_ID} {:016x}\n", fingerprint(&png));
    let already_current = read_to_string(&stamp_path).is_ok_and(|current| current == stamp);
    if already_current && icons_present(&data_home) {
        return false;
    }

    let desktop_dir = data_home.join("applications");
    let icons_dir = data_home.join("icons");

    let mut changed = write_desktop_entry(&desktop_dir);
    changed |= install_icon_theme(&icons_dir, &png);

    if changed {
        let _ = write_file(&stamp_path, stamp.as_bytes());
        // The cache tools are optional, slow-ish, and a failure only means the
        // next session recomputes the same caches.
        refresh_caches();
    }
    changed
}

/// `$XDG_DATA_HOME`, falling back to the spec default of `~/.local/share`.
fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"))
}

/// The embedded logo, at full resolution. `assets/logo/ezicode.png` is a
/// 1024x1024 RGBA PNG; the Olova name is the upstream fallback the rest of the
/// build already uses.
fn icon_source() -> Option<Vec<u8>> {
    use crate::assets::AppAssets;
    AppAssets::get("logo/ezicode.png")
        .or_else(|| AppAssets::get("logo/olova.png"))
        .map(|f| f.data.into_owned())
}

fn desktop_entry_path(desktop_dir: &Path) -> PathBuf {
    desktop_dir.join(format!("{APP_ID}.desktop"))
}

fn icon_path(icons_dir: &Path, size: u32) -> PathBuf {
    // hicolor is keyed by exact size, so every size is a separate file.
    icons_dir
        .join("hicolor")
        .join(format!("{size}x{size}"))
        .join("apps")
        .join(format!("{APP_ID}.png"))
}

fn stamp_path(data_home: &Path) -> PathBuf {
    data_home.join(APP_ID).join("desktop-install.stamp")
}

fn desktop_entry_contents() -> String {
    // `%F` is the desktop-entry spec's argument escape: a literal `%` has to be
    // written `%%`, otherwise a path containing one is mangled. `TryExec` must
    // keep the real `%`, so it is derived from the unescaped path.
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| APP_ID.to_string());
    let exec = path.replace('%', "%%");

    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={APP_NAME}\n\
         GenericName={GENERIC_NAME}\n\
         Comment={COMMENT}\n\
         Exec={exec} %F\n\
         TryExec={path}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Development;IDE;TextEditor;\n\
         MimeType=text/plain;inode/directory;\n\
         # We never answer org.freedesktop.Startup, and claiming to would leave a\n\
         # ghost entry sitting in the taskbar for the whole launch.\n\
         StartupNotify=false\n\
         # Must equal the WM_CLASS / Wayland app_id GPUI sets from `app_id`, or\n\
         # desktop-file matching silently falls back to a generic icon.\n\
         StartupWMClass={APP_ID}\n"
    )
}

fn write_desktop_entry(desktop_dir: &Path) -> bool {
    let contents = desktop_entry_contents();
    write_if_changed(&desktop_entry_path(desktop_dir), contents.as_bytes())
}

fn install_icon_theme(icons_dir: &Path, png: &[u8]) -> bool {
    let Some(ladder) = resample_ladder(png, ICON_SIZES) else {
        eprintln!("[linux] could not decode the embedded logo; taskbar icon will be missing");
        return false;
    };

    let mut changed = false;
    for (size, image) in ladder {
        if let Some(bytes) = encode_png(&image) {
            changed |= write_if_changed(&icon_path(icons_dir, size), &bytes);
        }
    }
    changed
}

/// Cheap FNV-1a over the embedded bytes. 1024x1024 is under half a megabyte, so
/// this is microseconds, and it is far better than pulling in a hashing crate
/// just to answer "did the logo change since last launch?".
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// True only when the desktop entry and every icon file are still on disk.
fn icons_present(data_home: &Path) -> bool {
    desktop_entry_path(&data_home.join("applications")).is_file()
        && ICON_SIZES
            .iter()
            .all(|&size| icon_path(&data_home.join("icons"), size).is_file())
}

/// Decodes the source PNG and produces one image per entry of `sizes`, halving
/// at each step instead of resampling the full 1024x1024 source every time:
/// cheaper, and a sharper result than one huge Lanczos step. `sizes` must be in
/// descending order.
fn resample_ladder(png: &[u8], sizes: &[u32]) -> Option<Vec<(u32, image::RgbaImage)>> {
    let source = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();

    let mut ladder = Vec::with_capacity(sizes.len());
    let mut working = source;
    for &size in sizes {
        working = image::imageops::resize(&working, size, size, image::imageops::FilterType::Lanczos3);
        ladder.push((size, working.clone()));
    }
    Some(ladder)
}

fn encode_png(image: &image::RgbaImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// Writes `contents` to `path` only if it differs, so repeated launches do not
/// churn mtimes and invalidate every panel's icon cache.
fn write_if_changed(path: &Path, contents: &[u8]) -> bool {
    if read_to_string(path).is_ok_and(|existing| existing.as_bytes() == contents) {
        return false;
    }
    match write_file(path, contents) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("[linux] could not write {}: {e}", path.display());
            false
        }
    }
}

/// Reads a file as UTF-8, so a binary blob (the icon PNGs) simply fails to match
/// and gets rewritten rather than needing a second code path.
fn read_to_string(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

fn write_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write to a sibling temp file and rename, so a panel reading the icon
    // never observes a half-written file.
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Recomputes the GTK icon cache and the desktop MIME database so the entry we
/// just wrote is visible to panels that only consult the caches. Both tools are
/// optional and neither is required for the icon to show.
fn refresh_caches() {
    let data_home = data_home();

    let invocations = [
        (
            "gtk-update-icon-cache",
            vec![
                "-f".to_string(),
                "-t".to_string(),
                data_home.join("icons").display().to_string(),
                "--ignore-theme-index".to_string(),
            ],
        ),
        (
            "update-desktop-database",
            vec![data_home.join("applications").display().to_string()],
        ),
    ];

    for (tool, args) in invocations {
        // Spawned and immediately dropped: the editor's startup must not block
        // on another process.
        let _ = Command::new(tool)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// Sets `_NET_WM_ICON` on the window, covering the X11 panels that ignore the
/// desktop entry. A no-op on Wayland: compositors must not be fed X11 concepts,
/// and the only raw handle we can read is an xcb window id anyway.
pub fn apply_window_icon(window: &gpui::Window) {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return;
    }
    if let Err(e) = set_net_wm_icon(window) {
        eprintln!("[linux] could not set the taskbar icon: {e:#}");
    }
}

fn set_net_wm_icon(window: &gpui::Window) -> anyhow::Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto;
    use x11rb::wrapper::ConnectionExt;

    // Fully qualified: `gpui::Window` has an inherent `window_handle()` that
    // returns GPUI's own `AnyWindowHandle`, which would shadow the trait method.
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|e| anyhow::anyhow!("no native window handle: {e:?}"))?;
    let RawWindowHandle::Xcb(xcb_handle) = handle.as_raw() else {
        // An Xlib or foreign window handle: not something we can poke a property
        // on, and not worth guessing about.
        return Ok(());
    };
    let window_id = xcb_handle.window.get();

    let png = icon_source().ok_or_else(|| anyhow::anyhow!("no embedded logo"))?;
    let ladder = resample_ladder(&png, WINDOW_ICON_SIZES)
        .ok_or_else(|| anyhow::anyhow!("could not decode the embedded logo"))?;
    let pixels = net_wm_icon_pixels(&ladder);
    if pixels.is_empty() {
        return Err(anyhow::anyhow!("resampling the logo produced no pixels"));
    }

    // GPUI's `display_handle` is unimplemented on X11, so we open our own
    // connection to the same `$DISPLAY`. A second connection to the same server
    // is ordinary X11 practice, and it keeps the vendored GPUI platform code
    // untouched.
    let (conn, screen_num) = x11rb::connect(None)?;
    let _screen = &conn.setup().roots[screen_num];

    let atom = xproto::ConnectionExt::intern_atom(&conn, false, b"_NET_WM_ICON")?
        .reply()?
        .atom;

    // `_NET_WM_ICON` is a CARDINAL property holding, for each size, a
    // width/height header followed by width*height ARGB pixels.
    conn.change_property32(
        xproto::PropMode::REPLACE,
        window_id,
        atom,
        xproto::AtomEnum::CARDINAL,
        &pixels,
    )?;
    conn.flush()?;

    Ok(())
}

/// Every size in [`WINDOW_ICON_SIZES`] laid out as `_NET_WM_ICON` expects:
/// `width, height, ARGB...` per size, concatenated.
///
/// `_NET_WM_ICON` uses straight (non-premultiplied) alpha, which is exactly
/// what `to_rgba8` gives us. The values go out as native-endian `u32`s, not
/// byteswapped ones: xcb (and therefore x11rb) puts format-32 property data on
/// the wire in the client's own byte order, and every reader casts the bytes
/// straight to a `uint32_t` and unpacks them as `a << 24 | r << 16 | g << 8 | b`.
/// Byte-swapping here silently exchanges red and blue.
fn net_wm_icon_pixels(ladder: &[(u32, image::RgbaImage)]) -> Vec<u32> {
    let mut out = Vec::new();
    for (size, image) in ladder {
        out.push(*size);
        out.push(*size);
        for px in image.pixels() {
            let [r, g, b, a] = px.0;
            out.push(u32::from(a) << 24 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b));
        }
    }
    out
}
