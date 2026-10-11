//! Sprite-sheet atlas slicing — the Rust port of `@almadar/ui`'s
//! `lib/atlasSlice.ts`, grounded in the `@almadar/core` JSON contract
//! (`packages/almadar-core/src/types/asset.ts:184-265`).
//!
//! An `Asset { url: <sheet.png>, atlas: <atlas.json>, sprite: <name> }` cuts
//! one sub-rect out of a packed sheet. Two atlas shapes exist:
//!
//! - `TextureAtlas` — named sub-rects (`subTextures[sprite]`), the ShoeBox /
//!   TexturePacker JSON every Kenney sheet ships.
//! - `Tilesheet` — a uniform grid cut by `"col,row"` or flat index.
//!
//! `sub_rect_for` is a line-by-line port of `subRectFor` — keep them in sync.
//! Parsed atlases are cached by URL (parse once); the underlying JSON bytes
//! arrive through [`crate::asset`].

use crate::packet::Rect;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

const NS: &str = "almadar::game_renderer::atlas";

/// One named sub-rectangle inside a packed sheet. Mirrors core `SubTexture`
/// (the trim/pad `frame*` fields exist on trimmed atlases; the blit path
/// ignores them, exactly like `subRectFor`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubTexture {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub frame_x: Option<f32>,
    pub frame_y: Option<f32>,
    pub frame_width: Option<f32>,
    pub frame_height: Option<f32>,
}

/// A packed sheet + named sub-rectangles. Mirrors core `TextureAtlas`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextureAtlas {
    pub image_path: String,
    pub sub_textures: HashMap<String, SubTexture>,
}

/// A uniform-grid tile page cut by index. Mirrors core `Tilesheet`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tilesheet {
    pub image_path: String,
    pub tile_width: f32,
    pub tile_height: f32,
    pub columns: u32,
    pub rows: u32,
    pub margin: Option<f32>,
    pub spacing: Option<f32>,
    pub names: Option<Vec<String>>,
}

/// A parsed atlas JSON — discriminated by key presence, the same test
/// `atlasSlice.isTilesheet` applies (`tileWidth` is a number).
#[derive(Debug, Clone)]
pub enum ParsedAtlas {
    Texture(TextureAtlas),
    Tilesheet(Tilesheet),
}

static PARSED: OnceLock<RwLock<HashMap<String, Arc<ParsedAtlas>>>> = OnceLock::new();

fn parsed() -> &'static RwLock<HashMap<String, Arc<ParsedAtlas>>> {
    PARSED.get_or_init(Default::default)
}

/// Parse atlas JSON bytes, dispatching on shape (`subTextures` → named atlas,
/// `tileWidth` → uniform tilesheet).
pub fn parse_atlas(json: &[u8]) -> Result<ParsedAtlas, String> {
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("atlas json: {e}"))?;
    if value.get("subTextures").is_some() {
        let atlas: TextureAtlas =
            serde_json::from_value(value).map_err(|e| format!("TextureAtlas: {e}"))?;
        return Ok(ParsedAtlas::Texture(atlas));
    }
    if value.get("tileWidth").is_some() {
        let sheet: Tilesheet =
            serde_json::from_value(value).map_err(|e| format!("Tilesheet: {e}"))?;
        return Ok(ParsedAtlas::Tilesheet(sheet));
    }
    Err("atlas json is neither a TextureAtlas (subTextures) nor a Tilesheet (tileWidth)".into())
}

/// The parsed atlas for a URL, fetched+parsed once and cached. Schedules the
/// fetch when unknown; `None` until the JSON lands or on parse failure
/// (logged — the caller skips the sprite this frame, like React).
pub fn parsed_atlas(atlas_url: &str) -> Option<Arc<ParsedAtlas>> {
    {
        let map = parsed().read().expect("atlas cache poisoned");
        if let Some(a) = map.get(atlas_url) {
            return Some(a.clone());
        }
    }
    let bytes = crate::asset::asset_bytes(atlas_url)?;
    match parse_atlas(&bytes) {
        Ok(atlas) => {
            tracing::info!(target: NS, url = %atlas_url, "atlas parsed");
            let atlas = Arc::new(atlas);
            parsed()
                .write()
                .expect("atlas cache poisoned")
                .insert(atlas_url.to_string(), atlas.clone());
            Some(atlas)
        }
        Err(e) => {
            tracing::warn!(target: NS, url = %atlas_url, error = %e, "atlas parse failed");
            None
        }
    }
}

/// Resolve the source rect for `sprite` in `atlas` — the line-by-line port of
/// `atlasSlice.subRectFor`:
/// - `TextureAtlas`: `sprite` is a SubTexture name (e.g. `"blockBrown.png"`).
/// - `Tilesheet`: `sprite` is `"col,row"` or a flat row-major index.
/// Returns `None` when the name/index does not resolve.
pub fn sub_rect_for(atlas: &ParsedAtlas, sprite: &str) -> Option<Rect> {
    match atlas {
        ParsedAtlas::Tilesheet(sheet) => {
            let (col, row) = if let Some((c, r)) = sprite.split_once(',') {
                (c.trim().parse::<i64>().ok()?, r.trim().parse::<i64>().ok()?)
            } else {
                let i = sprite.parse::<i64>().ok()?;
                (
                    i.rem_euclid(sheet.columns as i64),
                    i.div_euclid(sheet.columns as i64),
                )
            };
            if col < 0 || row < 0 || col >= sheet.columns as i64 || row >= sheet.rows as i64 {
                return None;
            }
            let margin = sheet.margin.unwrap_or(0.0);
            let spacing = sheet.spacing.unwrap_or(0.0);
            Some(Rect {
                x: margin + col as f32 * (sheet.tile_width + spacing),
                y: margin + row as f32 * (sheet.tile_height + spacing),
                w: sheet.tile_width,
                h: sheet.tile_height,
            })
        }
        ParsedAtlas::Texture(atlas) => {
            let st = atlas.sub_textures.get(sprite)?;
            Some(Rect {
                x: st.x,
                y: st.y,
                w: st.width,
                h: st.height,
            })
        }
    }
}

/// One-call convenience: resolve `Asset { atlas, sprite }` to a source rect
/// via the caches (schedules fetches; `None` until ready — React parity).
pub fn resolve_atlas_rect(atlas_url: &str, sprite: &str) -> Option<Rect> {
    let atlas = parsed_atlas(atlas_url)?;
    sub_rect_for(&atlas, sprite)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATLAS_JSON: &str = r#"{
        "imagePath": "spritesheet_tilesBlue.png",
        "subTextures": {
            "tileBlue_01.png": { "x": 64, "y": 0, "width": 64, "height": 64 },
            "tileBlue_02.png": { "x": 128, "y": 0, "width": 64, "height": 64 }
        }
    }"#;

    const TILESHEET_JSON: &str = r#"{
        "imagePath": "tiles.png",
        "tileWidth": 64,
        "tileHeight": 64,
        "columns": 4,
        "rows": 2,
        "margin": 2,
        "spacing": 1
    }"#;

    #[test]
    fn texture_atlas_named_subrect_resolves() {
        let atlas = parse_atlas(ATLAS_JSON.as_bytes()).expect("parse");
        let r = sub_rect_for(&atlas, "tileBlue_02.png").expect("named rect");
        assert_eq!((r.x, r.y, r.w, r.h), (128.0, 0.0, 64.0, 64.0));
        assert!(
            sub_rect_for(&atlas, "missing.png").is_none(),
            "unknown name → None"
        );
    }

    #[test]
    fn tilesheet_col_row_and_flat_index_agree() {
        let atlas = parse_atlas(TILESHEET_JSON.as_bytes()).expect("parse");
        // "2,1": sx = 2 + 2*(64+1) = 132, sy = 2 + 1*(64+1) = 67
        let by_coord = sub_rect_for(&atlas, "2,1").expect("col,row");
        // flat index 6 = col 2, row 1 in a 4-column grid
        let by_index = sub_rect_for(&atlas, "6").expect("flat index");
        assert_eq!(by_coord.x, by_index.x);
        assert_eq!(by_coord.y, by_index.y);
        assert_eq!(
            (by_coord.x, by_coord.y, by_coord.w, by_coord.h),
            (132.0, 67.0, 64.0, 64.0)
        );
        assert!(
            sub_rect_for(&atlas, "9").is_none(),
            "index past grid → None"
        );
        assert!(sub_rect_for(&atlas, "abc").is_none(), "non-numeric → None");
    }

    #[test]
    fn shape_detection_rejects_unknown_json() {
        assert!(parse_atlas(br#"{"foo": 1}"#).is_err());
    }
}
