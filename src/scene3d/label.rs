//! A text label for a 3D scene: shaped and rasterized by the 2D painter's vector text (`crate::vector`), then stored
//! in the asset cache under a content key, so a [`super::Material`] `map` draws it on a quad like any image.

use crate::packet::{DrawCommand, Halo, TextAlign, Vec2};
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

/// Raster pixels per world unit: the 2D painter's pixels per cell.
const DENSITY: f32 = crate::projector::TILE_WIDTH;

/// A rasterized label: the asset key its image is stored under and the quad it fills, in world units. `ascent` and
/// `descent` are the face's (descent negative) and `text_width` the shaped run, both in world units, for alignment.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub key: String,
    pub width: f32,
    pub height: f32,
    pub text_width: f32,
    pub ascent: f32,
    pub descent: f32,
}

/// What the label draws: `em` is the font size in world units, `halo` the outline drawn outside each glyph (troika's
/// `outlineWidth`), and colors are straight sRGB RGBA.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelStyle {
    pub em: f32,
    pub bold: bool,
    pub family: crate::packet::TextFamily,
    pub color: [f32; 4],
    pub halo: f32,
    pub halo_color: [f32; 4],
}

fn registered() -> &'static Mutex<HashSet<String>> {
    static KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    KEYS.get_or_init(Default::default)
}

/// Rasterize `text` in `style` (once per distinct label) and return the quad that shows it. The raster keeps the 2D
/// painter's density unless that would pass the WebGL2 texture limit, where it scales down to fit.
pub fn label(text: &str, style: &LabelStyle) -> Result<Label, String> {
    if !(style.em.is_finite() && style.em > 0.0) {
        return Err(format!("font size {} is not above zero", style.em));
    }
    if !(style.halo.is_finite() && style.halo >= 0.0) {
        return Err(format!("outline width {} is negative", style.halo));
    }
    let (ascent, descent) =
        crate::vector::text_font_metrics(style.em).ok_or("the bundled text face is unavailable")?;
    let text_width = crate::vector::shape(text, style.em, style.bold, style.family).width;
    // the AA fringe and the halo stroke sit outside the glyph box
    let pad = style.halo + 1.0 / DENSITY;
    // the raster draws the Latin face's middle on the quad's center, so the quad grows evenly around it to hold the
    // tallest face the text is shaped with
    let (top, bottom) = crate::vector::text_run_extent(text, style.em, style.bold, style.family)
        .ok_or("the bundled text face is unavailable")?;
    let middle = (ascent + descent) / 2.0;
    let half = (top - middle).max(middle - bottom);
    let (width, height) = (text_width + 2.0 * pad, 2.0 * half + 2.0 * pad);
    let limit = wgpu::Limits::downlevel_webgl2_defaults().max_texture_dimension_2d as f32;
    let density = DENSITY.min(limit / width.max(height));
    let (px_w, px_h) = (
        (width * density).ceil().max(1.0) as u32,
        (height * density).ceil().max(1.0) as u32,
    );
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    for v in [style.em, style.halo, density]
        .iter()
        .chain(&style.color)
        .chain(&style.halo_color)
    {
        v.to_bits().hash(&mut hasher);
    }
    (style.bold, style.family).hash(&mut hasher);
    let key = format!("scene3d-label:{:016x}", hasher.finish());
    let fresh = registered()
        .lock()
        .map_err(|_| "the label registry is poisoned")?
        .insert(key.clone());
    if fresh {
        let command = DrawCommand::Text {
            text: text.to_owned(),
            pos: Vec2 {
                x: px_w as f32 / 2.0,
                y: px_h as f32 / 2.0,
            },
            size: style.em * density,
            color: style.color,
            align: TextAlign::Center,
            weight: if style.bold { 700 } else { 400 },
            halo: (style.halo > 0.0).then_some(Halo {
                color: style.halo_color,
                width: 2.0 * style.halo * density,
            }),
            family: style.family,
        };
        let encoded = crate::vector::rasterize(&[&command], px_w, px_h, 1.0, 1.0)
            .ok_or_else(|| format!("a {px_w} x {px_h} label surface cannot be allocated"))
            .and_then(|rgba| {
                let image = image::RgbaImage::from_raw(px_w, px_h, rgba)
                    .ok_or("the label raster has the wrong size")?;
                let mut png = Vec::new();
                image
                    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                    .map_err(|e| format!("label encode: {e}"))?;
                Ok(png)
            });
        match encoded {
            Ok(png) => crate::asset::register_asset(&key, Arc::from(png), Some("image/png")),
            Err(why) => {
                if let Ok(mut keys) = registered().lock() {
                    keys.remove(&key);
                }
                return Err(why);
            }
        }
    }
    Ok(Label {
        key,
        width: px_w as f32 / density,
        height: px_h as f32 / density,
        text_width,
        ascent,
        descent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(bold: bool) -> LabelStyle {
        LabelStyle {
            em: 1.0,
            bold,
            family: crate::packet::TextFamily::Sans,
            color: [1.0, 1.0, 1.0, 1.0],
            halo: 0.0,
            halo_color: [0.0, 0.0, 0.0, 1.0],
        }
    }

    fn inked_pixels(l: &Label) -> usize {
        let png = crate::asset::asset_bytes(&l.key).expect("the label image is registered");
        image::load_from_memory(&png)
            .expect("the label image decodes")
            .into_rgba8()
            .pixels()
            .filter(|p| p.0[3] > 0)
            .count()
    }

    // the Arabic face hangs a yeh's two dots lower than the Latin face's descent, which the label box was cut to
    #[test]
    fn an_arabic_label_keeps_the_dots_under_a_yeh() {
        for bold in [false, true] {
            let yeh = label("\u{064A}", &style(bold)).expect("yeh");
            let alef_maksura = label("\u{0649}", &style(bold)).expect("alef maksura");
            assert!(
                inked_pixels(&yeh) > inked_pixels(&alef_maksura),
                "bold {bold}: the yeh draws its dots under the shape it shares with the alef maksura"
            );
        }
    }

    #[test]
    fn control_a_latin_label_keeps_the_latin_box() {
        let s = style(false);
        let l = label("Ag", &s).expect("label");
        let (ascent, descent) = crate::vector::text_font_metrics(s.em).expect("the bundled face");
        let px = 1.0 / DENSITY;
        assert!(
            (l.height - (ascent - descent + 2.0 * px)).abs() <= px,
            "height {} for ascent {ascent} and descent {descent}",
            l.height
        );
    }
}
