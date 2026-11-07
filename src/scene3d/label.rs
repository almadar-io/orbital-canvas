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
    pub mono: bool,
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
    let text_width = crate::vector::shape(text, style.em, style.bold, style.mono).width;
    // the AA fringe and the halo stroke sit outside the glyph box
    let pad = style.halo + 1.0 / DENSITY;
    let (width, height) = (text_width + 2.0 * pad, ascent - descent + 2.0 * pad);
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
    (style.bold, style.mono).hash(&mut hasher);
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
            mono: style.mono,
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
mod zz_probe {
    #[test]
    fn zz_write_u_label() {
        let style = super::LabelStyle { em: 0.32, bold: true, mono: false, color: [0.0, 0.0, 0.0, 1.0], halo: 0.02, halo_color: [1.0, 1.0, 1.0, 1.0] };
        let l = super::label("u+v", &style).unwrap();
        let bytes = crate::asset::asset_bytes(&l.key).unwrap();
        std::fs::write("/private/tmp/claude-501/-Users-osamaalghanmi-kflow-ai-builder/50357319-b734-4e98-8048-80b5e76ff042/scratchpad/desk/label-u.png", bytes).unwrap();
    }
}
