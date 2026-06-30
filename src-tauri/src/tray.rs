//! Tray icon + menu. The icon is **rendered from the active theme's palette**
//! (pushed up from the webview via `set_tray_palette`), not loaded from per-theme
//! image files — so adding a theme needs no assets. The icon color reflects the
//! engine rollup; the menu drives hook install/uninstall, opens settings, quits.

use crate::engine::Rollup;
use crate::hooks;
use crate::windows;
use serde::{Deserialize, Serialize};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconId};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_store::StoreExt;

const TRAY_ID: &str = "beacon-tray";
const STORE_FILE: &str = "beacon.json";
const PALETTE_KEY: &str = "tray.palette";
/// Render size of the tray dot (square RGBA). The OS scales it to the menu-bar
/// height; 32px stays crisp when scaled down on both macOS and Windows.
const TRAY_SIZE: u32 = 32;

/// Colors pushed from the active theme. RGB triples (0–255). `rollup` colors
/// drive the tray; `state` colors let the notifier render matching glyphs from
/// the same source. Shapes are fixed per state (see `Shape`), not stored here.
#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct TrayPalette {
    pub red: [u8; 3],
    pub orange: [u8; 3],
    pub green: [u8; 3],
    pub grey: [u8; 3],
    pub needs_you: [u8; 3],
    pub working: [u8; 3],
    pub ready: [u8; 3],
}

impl Default for TrayPalette {
    /// The `classic` theme — the backend's fallback before the webview pushes the
    /// persisted choice, so the tray is never blank or wrongly-colored at launch.
    fn default() -> Self {
        TrayPalette {
            red: [244, 89, 94],
            orange: [245, 167, 66],
            green: [70, 201, 139],
            grey: [124, 130, 141],
            needs_you: [244, 89, 94],
            working: [245, 167, 66],
            ready: [70, 201, 139],
        }
    }
}

impl TrayPalette {
    fn rollup_rgb(&self, rollup: Rollup) -> [u8; 3] {
        match rollup {
            Rollup::Red => self.red,
            Rollup::Orange => self.orange,
            Rollup::Green => self.green,
            Rollup::Grey => self.grey,
        }
    }
}

/// The four state silhouettes — shape carries the meaning, not hue (so the icon
/// reads in greyscale and at 16px). Geometry is 1:1 with the webview StateGlyph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// Needs you — solid rounded square.
    Square,
    /// Working — solid dot.
    Dot,
    /// Ready — check mark.
    Check,
    /// None / stale — hollow ring.
    Ring,
}

/// Tray rollup → silhouette.
pub fn shape_for_rollup(rollup: Rollup) -> Shape {
    match rollup {
        Rollup::Red => Shape::Square,
        Rollup::Orange => Shape::Dot,
        Rollup::Green => Shape::Check,
        Rollup::Grey => Shape::Ring,
    }
}

fn dist_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (vx, vy) = (bx - ax, by - ay);
    let (wx, wy) = (px - ax, py - ay);
    let len2 = vx * vx + vy * vy;
    let t = if len2 > 0.0 {
        ((wx * vx + wy * vy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (px - (ax + t * vx), py - (ay + t * vy));
    (dx * dx + dy * dy).sqrt()
}

/// Signed distance (px, negative inside) from pixel center to the shape, in a
/// 24-unit design space scaled to `size`. Coordinates are 1:1 with StateGlyph.
fn signed_distance(shape: Shape, px: f32, py: f32, scale: f32) -> f32 {
    let (cx, cy) = (12.0 * scale, 12.0 * scale);
    let (rx, ry) = (px - cx, py - cy);
    match shape {
        Shape::Dot => (rx * rx + ry * ry).sqrt() - 5.4 * scale,
        Shape::Ring => ((rx * rx + ry * ry).sqrt() - 7.4 * scale).abs() - 1.3 * scale,
        Shape::Square => {
            // Rounded box SDF (half-extent 7.4, corner 3.6).
            let b = 7.4 * scale;
            let r = 3.6 * scale;
            let qx = rx.abs() - b + r;
            let qy = ry.abs() - b + r;
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            qx.max(qy).min(0.0) + outside - r
        }
        Shape::Check => {
            let d = dist_segment(
                px,
                py,
                5.0 * scale,
                12.6 * scale,
                10.0 * scale,
                17.4 * scale,
            )
            .min(dist_segment(
                px,
                py,
                10.0 * scale,
                17.4 * scale,
                19.3 * scale,
                6.8 * scale,
            ));
            d - 1.6 * scale // half of the 3.2 stroke
        }
    }
}

/// Render a state glyph into a straight-alpha RGBA buffer. Edges anti-aliased
/// over ~1px via the signed distance, so shapes stay crisp at any scale. Pure
/// math — no image decoding.
pub fn render_glyph_rgba(shape: Shape, rgb: [u8; 3], size: u32) -> Vec<u8> {
    let scale = size as f32 / 24.0;
    let mut buf = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let sd = signed_distance(shape, x as f32 + 0.5, y as f32 + 0.5, scale);
            let cov = (0.5 - sd).clamp(0.0, 1.0);
            let i = ((y * size + x) * 4) as usize;
            buf[i] = rgb[0];
            buf[i + 1] = rgb[1];
            buf[i + 2] = rgb[2];
            buf[i + 3] = (cov * 255.0).round() as u8;
        }
    }
    buf
}

/// Encode an RGBA buffer to PNG bytes (used for notification icons). None on the
/// (practically impossible) encoder error.
pub fn encode_png(rgba: &[u8], size: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(out)
}

/// Build the tray glyph for a rollup from the given palette.
fn icon_for(palette: &TrayPalette, rollup: Rollup) -> Image<'static> {
    let buf = render_glyph_rgba(
        shape_for_rollup(rollup),
        palette.rollup_rgb(rollup),
        TRAY_SIZE,
    );
    Image::new_owned(buf, TRAY_SIZE, TRAY_SIZE)
}

fn tooltip_for(rollup: Rollup) -> &'static str {
    match rollup {
        Rollup::Red => "Beacon — a session needs you",
        Rollup::Orange => "Beacon — working",
        Rollup::Green => "Beacon — ready",
        Rollup::Grey => "Beacon — no live sessions",
    }
}

/// Load the last-pushed palette from the store, or the classic default. Reading
/// it at build time means a non-default theme survives a restart with no flash.
pub fn load_palette(app: &AppHandle) -> TrayPalette {
    if let Ok(store) = app.store(STORE_FILE) {
        if let Some(v) = store.get(PALETTE_KEY) {
            if let Ok(p) = serde_json::from_value::<TrayPalette>(v) {
                return p;
            }
        }
    }
    TrayPalette::default()
}

/// Persist the active palette so the tray restyles instantly on next launch.
pub fn save_palette(app: &AppHandle, palette: &TrayPalette) {
    if let Ok(store) = app.store(STORE_FILE) {
        if let Ok(v) = serde_json::to_value(palette) {
            store.set(PALETTE_KEY, v);
            let _ = store.save();
        }
    }
}

/// Build the tray icon and menu. Starts grey (no sessions yet), using `palette`.
pub fn build(app: &AppHandle, palette: &TrayPalette) -> tauri::Result<()> {
    let widget = MenuItem::with_id(app, "widget", "Show / hide widget", true, None::<&str>)?;
    let install = MenuItem::with_id(
        app,
        "install",
        "Install Claude Code hooks",
        true,
        None::<&str>,
    )?;
    let uninstall = MenuItem::with_id(app, "uninstall", "Uninstall hooks", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Open Beacon…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Beacon", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &widget, &sep1, &install, &uninstall, &sep2, &settings, &sep3, &quit,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon_for(palette, Rollup::Grey))
        // Colored dot, not a monochrome template.
        .icon_as_template(false)
        .tooltip(tooltip_for(Rollup::Grey))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| handle_menu(app, event.id().as_ref()))
        .build(app)?;

    Ok(())
}

/// Update the tray icon + tooltip to reflect the current rollup, using `palette`.
pub fn set_rollup(app: &AppHandle, rollup: Rollup, palette: &TrayPalette) {
    if let Some(tray) = app.tray_by_id(&TrayIconId::new(TRAY_ID)) {
        let _ = tray.set_icon(Some(icon_for(palette, rollup)));
        let _ = tray.set_icon_as_template(false);
        let _ = tray.set_tooltip(Some(tooltip_for(rollup)));
    }
}

fn handle_menu(app: &AppHandle, id: &str) {
    // Use the live, configured port (it can change in settings).
    let port = crate::current_port(app);
    match id {
        "widget" => windows::toggle(app),
        "install" => {
            let msg = match crate::install_beacon_hooks(app) {
                Ok(path) => format!("Hooks installed in {}", path.display()),
                Err(e) => format!("Install failed: {e}"),
            };
            toast(app, &msg);
            show_settings(app);
        }
        "uninstall" => {
            let msg = match hooks::uninstall(port) {
                Ok(path) => format!("Hooks removed from {}", path.display()),
                Err(e) => format!("Uninstall failed: {e}"),
            };
            toast(app, &msg);
            show_settings(app);
        }
        "settings" => show_settings(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

pub(crate) fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        // Dev-mode self-heal. In `tauri dev` the settings window points at the
        // Vite dev server and is kept hidden between opens. If that webview is
        // ever left holding a dead page — Vite was down when it first tried to
        // load, Vite restarted, or an HMR full reload fired while it was hidden
        // — it presents blank when next shown (and can take the always-on-top
        // widget's transparent webview down with it when both are stale: the
        // "settings hides the widget" report). `reload()` is not enough: it only
        // re-runs the *current* document, so a webview whose initial navigation
        // failed has nothing live to reload and stays blank. Re-navigating to
        // the configured dev URL forces a fresh fetch, recovering even a
        // never-loaded webview. Release builds serve static bundled assets that
        // can't go stale, so this is compiled out there — no flash in production.
        #[cfg(debug_assertions)]
        if let Some(dev_url) = app.config().build.dev_url.clone() {
            let _ = window.navigate(dev_url);
        } else {
            let _ = window.reload();
        }
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Surface a short status message to the settings window (if open).
fn toast(app: &AppHandle, message: &str) {
    let _ = app.emit("beacon://toast", message);
    eprintln!("[beacon] {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(buf: &[u8], size: u32, x: u32, y: u32) -> u8 {
        buf[((y * size + x) * 4 + 3) as usize]
    }

    #[test]
    fn filled_dot_is_opaque_center_clear_corner() {
        let size = 32;
        let buf = render_glyph_rgba(Shape::Dot, [10, 20, 30], size);
        // Center fully covered; the color is preserved.
        let c = ((size / 2 * size + size / 2) * 4) as usize;
        assert_eq!(alpha_at(&buf, size, size / 2, size / 2), 255);
        assert_eq!(&buf[c..c + 3], &[10, 20, 30]);
        // Corner is outside the disc → transparent.
        assert_eq!(alpha_at(&buf, size, 0, 0), 0);
    }

    #[test]
    fn ring_is_hollow_in_the_middle() {
        let size = 32;
        let buf = render_glyph_rgba(Shape::Ring, [200, 100, 50], size);
        // The very center of a ring is empty; a point on the ring band is filled.
        assert_eq!(alpha_at(&buf, size, size / 2, size / 2), 0);
        let band_x = (size as f32 / 2.0 + 7.4 * (size as f32 / 24.0)).round() as u32 - 1;
        assert!(alpha_at(&buf, size, band_x, size / 2) > 0);
    }

    #[test]
    fn square_fills_its_center_and_each_shape_differs() {
        let size = 32;
        let sq = render_glyph_rgba(Shape::Square, [1, 2, 3], size);
        assert_eq!(alpha_at(&sq, size, size / 2, size / 2), 255);
        // Square covers more area than the check stroke (shape-based difference).
        let total = |b: &[u8]| b.iter().skip(3).step_by(4).map(|&a| a as u64).sum::<u64>();
        let ck = render_glyph_rgba(Shape::Check, [1, 2, 3], size);
        assert!(total(&sq) > total(&ck));
    }

    #[test]
    fn png_encodes_nonempty() {
        let buf = render_glyph_rgba(Shape::Dot, [1, 2, 3], 16);
        let png = encode_png(&buf, 16).expect("encodes");
        // PNG magic number.
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }
}
