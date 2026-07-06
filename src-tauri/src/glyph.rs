//! State-glyph rasterizer: pure signed-distance-field math that renders the
//! four state silhouettes into RGBA buffers (and PNG for notification icons).
//! Shared by the tray (`tray.rs`) and the notifier (`notify.rs`) so both draw
//! from the same geometry — which is itself 1:1 with the webview's StateGlyph.
//! No image decoding anywhere; everything is computed.

use crate::engine::Rollup;

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
