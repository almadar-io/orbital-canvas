//! `draw-sprite` and background tiles: images placed by their sheet frame, and images that failed to load.

use super::*;

pub(super) static FAILED_IMAGES: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

/// An image that failed to load paints nothing and the rest of the scene paints; the failure is logged once per url.
pub(super) fn skip_failed_image(url: &str) {
    let mut set = FAILED_IMAGES
        .get_or_init(Default::default)
        .write()
        .expect("failed image log poisoned");
    if set.insert(url.to_string()) {
        tracing::warn!(target: NS, url = %url, "draw-sprite image failed to load; the sprite paints nothing");
    }
}

pub(super) fn paint_sprite_gathering(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    gathered: &mut Gathered,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let start = out.len();
    paint_sprite(node, projector, to_screen, zoom, epoch_now_ms, out)?;
    if let Some(id) = hit_id(node) {
        let placed = out[start..].iter().find_map(|command| match command {
            DrawCommand::Sprite { dst, rotation, .. } => Some((*dst, *rotation)),
            _ => None,
        });
        if let Some((rect, rotation)) = placed {
            gathered.regions.push(Region {
                id: id.to_string(),
                rect,
                rotation,
                wheel: takes_wheel(node),
                fixed: false,
                drag: takes_drag(node),
            });
        }
    }
    Ok(())
}

/// `DrawSprite.tsx paintSprite`.
pub(super) fn paint_sprite(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    epoch_now_ms: f64,
    out: &mut Vec<DrawCommand>,
) -> Result<(), CanvasSceneError> {
    let Some(position) = node.get("position").and_then(parse_scene_pos) else {
        gap_once("sprite-position", "draw-sprite without position");
        return Ok(());
    };
    let tint = sprite_tint(node)?;
    if let Some(texture) = node.get("texture").filter(|t| !t.is_null()) {
        if tint.is_some() {
            return Err(invalid(
                "tint",
                "a target sprite is a render target, which takes no tint",
            ));
        }
        let texture = crate::layer_props::target_texture(texture)?;
        crate::layer_props::require_sprite_size(node)?;
        let src = node.get("frame").and_then(parse_xywh).unwrap_or_default();
        out.push(DrawCommand::Sprite {
            texture,
            src,
            dst: sprite_dst(node, position, projector, to_screen, zoom, src),
            rotation: get_f32(node, "rotation", 0.0),
            opacity: get_f32(node, "opacity", 1.0),
            flip_x: node.get("flipX").and_then(Value::as_bool).unwrap_or(false),
            flip_y: false,
            tint: None,
        });
        return Ok(());
    }
    let asset = node.get("asset");
    let url = asset
        .and_then(|a| a.get("url"))
        .and_then(Value::as_str)
        .or_else(|| asset.and_then(Value::as_str));
    let Some(url) = url else {
        return Err(CanvasSceneError::UnresolvedSprite(
            "no resolvable asset.url".into(),
        ));
    };
    let _ = asset::image_info(url);
    if asset::image_failed(url) {
        skip_failed_image(url);
        return Ok(());
    }
    if node.get("shadow").is_some_and(|shadow| !shadow.is_null()) {
        gap_once(
            "sprite-shadow",
            "draw-sprite shadow needs a GPU silhouette blur pass",
        );
        return Ok(());
    }

    // Source rect resolution order (DrawSprite.tsx 49-59): explicit frame →
    // atlas sub-rect → whole image. Unresolved = resource in-flight → skip.
    let src = if let Some(frame) = node.get("frame").and_then(parse_xywh) {
        frame
    } else if let (Some(animation), Some(atlas_url)) = (
        node.get("animation").and_then(Value::as_str),
        asset.and_then(|a| a.get("atlas")).and_then(Value::as_str),
    ) {
        let Some(atlas) = atlas::parsed_atlas(atlas_url) else {
            if asset::fetch_failed(atlas_url) || asset::asset_bytes(atlas_url).is_some() {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "atlas `{atlas_url}` does not parse"
                )));
            }
            return Ok(());
        };
        if matches!(&*atlas, atlas::ParsedAtlas::SpriteSheet(_)) {
            let time_ms = node
                .get("clockMs")
                .and_then(Value::as_f64)
                .unwrap_or(epoch_now_ms);
            let frame = atlas::sprite_sheet_frame(
                &atlas,
                animation,
                node.get("loop").and_then(Value::as_bool),
                time_ms,
            );
            let Some(frame) = frame else {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "atlas `{atlas_url}` has no animation `{animation}`"
                )));
            };
            frame
        } else if let Some(sprite) = asset.and_then(|a| a.get("sprite")).and_then(Value::as_str) {
            let Some(frame) = atlas::sub_rect_for(&atlas, sprite) else {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "atlas `{atlas_url}` has no sprite `{sprite}`"
                )));
            };
            frame
        } else {
            let Some((w, h)) = asset::image_info(url) else {
                return Ok(());
            };
            Rect {
                x: 0.0,
                y: 0.0,
                w: w as f32,
                h: h as f32,
            }
        }
    } else if let (Some(atlas_url), Some(sprite)) = (
        asset.and_then(|a| a.get("atlas")).and_then(Value::as_str),
        asset.and_then(|a| a.get("sprite")).and_then(Value::as_str),
    ) {
        asset::ensure_asset(atlas_url);
        let Some(r) = atlas::resolve_atlas_rect(atlas_url, sprite) else {
            if asset::fetch_failed(atlas_url) || asset::asset_bytes(atlas_url).is_some() {
                return Err(CanvasSceneError::UnresolvedSprite(format!(
                    "atlas `{atlas_url}` has no sprite `{sprite}`"
                )));
            }
            return Ok(());
        };
        r
    } else {
        let Some((w, h)) = asset::image_info(url) else {
            if asset::image_failed(url) {
                skip_failed_image(url);
            }
            return Ok(());
        };
        let whole = Rect {
            x: 0.0,
            y: 0.0,
            w: w as f32,
            h: h as f32,
        };
        match node.get("fit").and_then(Value::as_str) {
            Some("cover") => cover_crop(
                whole,
                get_f32(node, "width", 0.0),
                get_f32(node, "height", 0.0),
            ),
            _ => whole,
        }
    };

    // `spriteRect` (lib/drawable/contract.ts): an omitted width/height is one
    // cell on tile grids and the source's native px on world-pixel `free` boards.
    out.push(DrawCommand::Sprite {
        texture: url.to_string(),
        src,
        dst: sprite_dst(node, position, projector, to_screen, zoom, src),
        rotation: get_f32(node, "rotation", 0.0),
        opacity: get_f32(node, "opacity", 1.0),
        flip_x: node.get("flipX").and_then(Value::as_bool).unwrap_or(false),
        flip_y: false,
        tint,
    });
    Ok(())
}

/// A sprite's `tint`: a css colour (a palette name has already taken the theme's colour), or none.
fn sprite_tint(node: &Value) -> Result<Option<[f32; 4]>, CanvasSceneError> {
    match node.get("tint") {
        None | Some(Value::Null) => Ok(None),
        Some(tint) => tint
            .as_str()
            .and_then(parse_css_color)
            .map(Some)
            .ok_or_else(|| invalid("tint", "a css colour or a theme palette name")),
    }
}

/// The centred part of `whole` with the `width : height` aspect, so the image fills its rect without stretching.
fn cover_crop(whole: Rect, width: f32, height: f32) -> Rect {
    if width <= 0.0 || height <= 0.0 || whole.w <= 0.0 || whole.h <= 0.0 {
        return whole;
    }
    let aspect = width / height;
    if whole.w / whole.h > aspect {
        let w = whole.h * aspect;
        Rect {
            x: whole.x + (whole.w - w) / 2.0,
            w,
            ..whole
        }
    } else {
        let h = whole.w / aspect;
        Rect {
            y: whole.y + (whole.h - h) / 2.0,
            h,
            ..whole
        }
    }
}

pub(super) fn sprite_dst(
    node: &Value,
    position: ScenePos,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    zoom: f32,
    source: Rect,
) -> Rect {
    let tw = projector.tile_width;
    let free = projector.layout == Projection::Free;
    let w = node
        .get("width")
        .and_then(Value::as_f64)
        .map(|value| value as f32 * tw)
        .unwrap_or(if free { source.w } else { tw });
    let h = node
        .get("height")
        .and_then(Value::as_f64)
        .map(|value| value as f32 * tw)
        .unwrap_or(if free { source.h } else { tw });
    let anchor = node
        .get("anchor")
        .and_then(Value::as_str)
        .and_then(Anchor::parse)
        .unwrap_or(Anchor::TopLeft);
    let (px, py) = projector.anchor_point(position, anchor);
    let x = if anchor == Anchor::TopLeft {
        px
    } else {
        px - w / 2.0
    };
    let y = match anchor {
        Anchor::Ground => py - h,
        Anchor::Center => py - h / 2.0,
        Anchor::TopLeft => py,
    };
    let (x, y) = to_screen(x, y);
    Rect {
        x,
        y,
        w: w * zoom,
        h: h * zoom,
    }
}

/// Background image: atlas slices cover the viewport; whole images tile at
/// native pixel size with the camera phase, as in the existing web painter.
pub(crate) fn background_sprites(
    bg: &Value,
    width: f32,
    height: f32,
    cam: (f32, f32),
) -> Vec<DrawCommand> {
    let url = match bg {
        Value::String(s) => s.as_str(),
        Value::Object(_) => match bg.get("url").and_then(Value::as_str) {
            Some(url) => url,
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let atlas_slice = if let (Some(atlas_url), Some(sprite)) = (
        bg.get("atlas").and_then(Value::as_str),
        bg.get("sprite").and_then(Value::as_str),
    ) {
        asset::ensure_asset(atlas_url);
        atlas::resolve_atlas_rect(atlas_url, sprite)
    } else {
        None
    };
    if let Some(src) = atlas_slice {
        if src.w <= 0.0 || src.h <= 0.0 {
            return Vec::new();
        }
        let scale = (width / src.w).max(height / src.h);
        let w = src.w * scale;
        let h = src.h * scale;
        return vec![DrawCommand::Sprite {
            texture: url.to_string(),
            src,
            dst: Rect {
                x: (width - w) / 2.0,
                y: (height - h) / 2.0,
                w,
                h,
            },
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
            tint: None,
        }];
    }
    if bg.get("atlas").is_some() && bg.get("sprite").is_some() {
        return Vec::new();
    }
    let Some((image_w, image_h)) = asset::image_info(url) else {
        return Vec::new();
    };
    let (image_w, image_h) = (image_w as f32, image_h as f32);
    if image_w <= 0.0 || image_h <= 0.0 {
        return Vec::new();
    }
    let src = Rect {
        x: 0.0,
        y: 0.0,
        w: image_w,
        h: image_h,
    };
    let mut sprites = Vec::new();
    let start_x = -cam.0.rem_euclid(image_w);
    let start_y = -cam.1.rem_euclid(image_h);
    let mut y = start_y - image_h;
    while y < height {
        let mut x = start_x - image_w;
        while x < width {
            sprites.push(DrawCommand::Sprite {
                texture: url.to_string(),
                src,
                dst: Rect {
                    x,
                    y,
                    w: image_w,
                    h: image_h,
                },
                rotation: 0.0,
                opacity: 1.0,
                flip_x: false,
                flip_y: false,
                tint: None,
            });
            x += image_w;
        }
        y += image_h;
    }
    sprites
}
