//! `atlas-image` and `atlas-panel` (`AtlasImage.tsx`): sprites cut out of a packed sheet by the
//! atlas JSON (`atlas::resolve_atlas_rect`, the port of `atlasSlice.subRectFor`). The image draws
//! exactly that sub-rect, fitted into its box; the panel draws it as a nine-slice frame or a
//! tiled pattern. Until the sheet and atlas have loaded the scene reserves its box and draws
//! nothing, as the web does.

use super::draw::{frame, Pen};
use super::{props_f32, props_str};
use crate::asset;
use crate::atlas;
use crate::mappers::game_canvas_2d::CanvasSceneError;
use crate::mappers::learning::canvas::theme_for;
use crate::mappers::learning::gap_once;
use crate::mappers::scene::MappedScene;
use crate::packet::{DrawCommand, Rect};
use serde_json::Value;

const PLACEHOLDER: f32 = 16.0;

struct Source {
    url: String,
    /// `None` until the atlas / sheet has loaded.
    rect: Option<Rect>,
}

fn source(props: &Value) -> Option<Source> {
    let asset_v = props.get("asset")?;
    let url = asset_v
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())?
        .to_string();
    asset::ensure_asset(&url);
    let atlas_url = asset_v
        .get("atlas")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty());
    let sprite = asset_v
        .get("sprite")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty());
    let rect = match (atlas_url, sprite) {
        (Some(a), Some(sp)) => {
            asset::ensure_asset(a);
            atlas::resolve_atlas_rect(a, sp)
        }
        _ => asset::image_info(&url).map(|(w, h)| Rect {
            x: 0.0,
            y: 0.0,
            w: w as f32,
            h: h as f32,
        }),
    };
    Some(Source { url, rect })
}

fn sprite(url: &str, src: Rect, dst: Rect) -> DrawCommand {
    DrawCommand::Sprite {
        texture: url.to_string(),
        src,
        dst,
        rotation: 0.0,
        opacity: 1.0,
        flip_x: false,
        flip_y: false,
    }
}

fn place(pen: &Pen, url: &str, src: Rect, d: Rect) -> DrawCommand {
    let (x, y) = pen.xf.p(d.x, d.y);
    sprite(
        url,
        src,
        Rect {
            x,
            y,
            w: d.w * pen.xf.s,
            h: d.h * pen.xf.s,
        },
    )
}

pub fn image(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let Some(src) = source(props) else {
        return Err(CanvasSceneError::NothingToDraw(
            "atlas-image without asset.url".into(),
        ));
    };
    let size = props.get("size").and_then(Value::as_f64).map(|s| s as f32);
    let given = |k: &str| {
        props
            .get(k)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .or(size)
    };
    let (gw, gh) = (given("width"), given("height"));
    let natural = src.rect.map(|r| (r.w, r.h));
    let (bw, bh) = match (gw, gh, natural) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((nw, nh))) => (w, w * nh / nw),
        (None, Some(h), Some((nw, nh))) => (h * nw / nh, h),
        (None, None, Some((nw, nh))) => (nw, nh),
        (Some(w), None, None) => (w, w),
        (None, Some(h), None) => (h, h),
        (None, None, None) => (PLACEHOLDER, PLACEHOLDER),
    };
    let theme = theme_for(props);
    let (xf, pw, ph) = frame(bw, bh, viewport);
    let mut pen = Pen::new(&theme, xf);
    pen.clear(theme.ground);
    if let Some(r) = src.rect {
        let (k_contain, k_cover) = ((bw / r.w).min(bh / r.h), (bw / r.w).max(bh / r.h));
        let (s_rect, d_rect) = match props_str(props, "fit").unwrap_or("contain") {
            "fill" => (
                r,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: bw,
                    h: bh,
                },
            ),
            "cover" => {
                let (vw, vh) = (bw / k_cover, bh / k_cover);
                (
                    Rect {
                        x: r.x + (r.w - vw) / 2.0,
                        y: r.y + (r.h - vh) / 2.0,
                        w: vw,
                        h: vh,
                    },
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        w: bw,
                        h: bh,
                    },
                )
            }
            _ => {
                let (dw, dh) = (r.w * k_contain, r.h * k_contain);
                (
                    r,
                    Rect {
                        x: (bw - dw) / 2.0,
                        y: (bh - dh) / 2.0,
                        w: dw,
                        h: dh,
                    },
                )
            }
        };
        let cmd = place(&pen, &src.url, s_rect, d_rect);
        pen.push(cmd);
    }
    Ok(pen.finish("atlas-image", pw, ph))
}

fn style_px(props: &Value, key: &str) -> Option<f32> {
    let v = props.get("style")?.get(key)?;
    if let Some(n) = v.as_f64() {
        return Some(n as f32);
    }
    let text = v.as_str()?;
    match text
        .strip_suffix("px")
        .and_then(|n| n.trim().parse::<f32>().ok())
    {
        Some(n) => Some(n),
        None => {
            gap_once("atlas-panel-style", &format!("style.{key} `{text}` is not a pixel size — the panel keeps its border-only size"));
            None
        }
    }
}

pub fn panel(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let Some(src) = source(props) else {
        return Err(CanvasSceneError::NothingToDraw(
            "atlas-panel without asset.url".into(),
        ));
    };
    let repeat = props_str(props, "mode") == Some("repeat");
    let border_slice = props_f32(props, "borderSlice", 16.0);
    let border_w = props_f32(props, "borderWidth", 16.0);
    let fallback = if repeat { 0.0 } else { border_w * 2.0 };
    let (bw, bh) = (
        style_px(props, "width").unwrap_or(fallback),
        style_px(props, "height").unwrap_or(fallback),
    );
    if bw <= 0.0 || bh <= 0.0 {
        return Err(CanvasSceneError::NothingToDraw(
            "atlas-panel tiling a box with no size".into(),
        ));
    }
    if props
        .get("children")
        .is_some_and(|c| c.as_array().is_none_or(|a| !a.is_empty()))
    {
        gap_once(
            "atlas-panel-children",
            "atlas-panel children are not composed into a Rust scene",
        );
    }
    let theme = theme_for(props);
    let (xf, pw, ph) = frame(bw, bh, viewport);
    let mut pen = Pen::new(&theme, xf);
    pen.clear(theme.ground);
    let Some(r) = src.rect else {
        return Ok(pen.finish("atlas-panel", pw, ph));
    };
    let cmds: Vec<DrawCommand> = if repeat {
        let mut out = Vec::new();
        let mut y = 0.0;
        while y < bh {
            let h = r.h.min(bh - y);
            let mut x = 0.0;
            while x < bw {
                let w = r.w.min(bw - x);
                out.push(place(
                    &pen,
                    &src.url,
                    Rect {
                        x: r.x,
                        y: r.y,
                        w,
                        h,
                    },
                    Rect { x, y, w, h },
                ));
                x += r.w;
            }
            y += r.h;
        }
        out
    } else {
        let bs = border_slice.min(r.w / 2.0).min(r.h / 2.0);
        let t = border_w * 1.0f32.min(bw / (2.0 * border_w)).min(bh / (2.0 * border_w));
        let (cw, ch) = (r.w - 2.0 * bs, r.h - 2.0 * bs);
        let part = |sx: f32, sy: f32, sw: f32, sh: f32, dx: f32, dy: f32, dw: f32, dh: f32| {
            place(
                &pen,
                &src.url,
                Rect {
                    x: r.x + sx,
                    y: r.y + sy,
                    w: sw,
                    h: sh,
                },
                Rect {
                    x: dx,
                    y: dy,
                    w: dw,
                    h: dh,
                },
            )
        };
        let (mw, mh) = ((bw - 2.0 * t).max(0.0), (bh - 2.0 * t).max(0.0));
        vec![
            part(bs, bs, cw, ch, t, t, mw, mh),
            part(bs, 0.0, cw, bs, t, 0.0, mw, t),
            part(bs, r.h - bs, cw, bs, t, bh - t, mw, t),
            part(0.0, bs, bs, ch, 0.0, t, t, mh),
            part(r.w - bs, bs, bs, ch, bw - t, t, t, mh),
            part(0.0, 0.0, bs, bs, 0.0, 0.0, t, t),
            part(r.w - bs, 0.0, bs, bs, bw - t, 0.0, t, t),
            part(0.0, r.h - bs, bs, bs, 0.0, bh - t, t, t),
            part(r.w - bs, r.h - bs, bs, bs, bw - t, bh - t, t, t),
        ]
    };
    for c in cmds {
        pen.push(c);
    }
    Ok(pen.finish("atlas-panel", pw, ph))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A 96x48 sheet (48x48 `panel.png` with 6px corner swatches, 48x48 `gem.png`) and its
    /// TextureAtlas JSON, both as data URLs so the fixture is self-contained.
    fn sheet() -> Value {
        serde_json::from_str(include_str!("fixtures/atlas-sheet.json")).unwrap()
    }

    fn asset_of(sprite: &str) -> Value {
        let s = sheet();
        json!({"url": s["sheet"], "atlas": s["atlas"], "sprite": sprite, "name": sprite})
    }

    fn sprites(s: &MappedScene) -> Vec<(Rect, Rect)> {
        s.packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Sprite { src, dst, .. } => Some((*src, *dst)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_atlas_image_draws_exactly_the_named_sub_rect_at_its_size() {
        let gem = image(
            &json!({"type": "atlas-image", "asset": asset_of("gem.png"), "size": 96}),
            None,
        )
        .unwrap();
        let sp = sprites(&gem);
        assert_eq!(sp.len(), 1);
        assert_eq!(
            (sp[0].0.x, sp[0].0.y, sp[0].0.w, sp[0].0.h),
            (48.0, 0.0, 48.0, 48.0),
            "the gem's frame, never the whole sheet"
        );
        assert_eq!((sp[0].1.w, sp[0].1.h), (96.0, 96.0));
        assert_eq!((gem.packet.width, gem.packet.height), (96.0, 96.0));
        let panel = image(
            &json!({"type": "atlas-image", "asset": asset_of("panel.png"), "size": 96}),
            None,
        )
        .unwrap();
        assert_eq!(
            sprites(&panel)[0].0.x,
            0.0,
            "control: another sprite name cuts another rect"
        );
    }

    #[test]
    fn natural_size_and_one_given_dimension_keep_the_aspect_ratio() {
        let natural = image(
            &json!({"type": "atlas-image", "asset": asset_of("gem.png")}),
            None,
        )
        .unwrap();
        assert_eq!((natural.packet.width, natural.packet.height), (48.0, 48.0));
        let wide = image(
            &json!({"type": "atlas-image", "asset": asset_of("gem.png"), "width": 120}),
            None,
        )
        .unwrap();
        assert_eq!((wide.packet.width, wide.packet.height), (120.0, 120.0));
    }

    #[test]
    fn fit_contain_letterboxes_cover_crops_and_fill_stretches() {
        let box_of = |fit: &str| {
            image(&json!({"type": "atlas-image", "asset": asset_of("gem.png"), "width": 100, "height": 50, "fit": fit}), None).unwrap()
        };
        let contain = sprites(&box_of("contain"))[0];
        assert_eq!(
            (contain.1.w, contain.1.h, contain.1.x),
            (50.0, 50.0, 25.0),
            "a square in a wide box is centred at the height"
        );
        let cover = sprites(&box_of("cover"))[0];
        assert_eq!((cover.1.w, cover.1.h), (100.0, 50.0));
        assert_eq!(
            (cover.0.w, cover.0.h),
            (48.0, 24.0),
            "cover crops the source to the box aspect"
        );
        let fill = sprites(&box_of("fill"))[0];
        assert_eq!(
            (fill.0.w, fill.0.h, fill.1.w, fill.1.h),
            (48.0, 48.0, 100.0, 50.0)
        );
    }

    #[test]
    fn without_a_url_there_is_nothing_to_draw_and_an_unknown_sprite_reserves_the_box() {
        assert!(matches!(
            image(&json!({"type": "atlas-image", "asset": {}}), None),
            Err(CanvasSceneError::NothingToDraw(_))
        ));
        let missing = image(
            &json!({"type": "atlas-image", "asset": asset_of("nope.png"), "size": 40}),
            None,
        )
        .unwrap();
        assert!(sprites(&missing).is_empty());
        assert_eq!(
            (missing.packet.width, missing.packet.height),
            (40.0, 40.0),
            "the box is reserved while the sprite does not resolve"
        );
    }

    #[test]
    fn a_nine_slice_panel_cuts_corners_edges_and_a_filled_centre() {
        let p = panel(&json!({"type": "atlas-panel", "asset": asset_of("panel.png"), "borderSlice": 6, "borderWidth": 12, "style": {"width": 120, "height": 80}}), None).unwrap();
        let sp = sprites(&p);
        assert_eq!(sp.len(), 9);
        let corner = sp
            .iter()
            .find(|(s, d)| s.x == 0.0 && s.y == 0.0 && d.x == 0.0 && d.y == 0.0)
            .unwrap();
        assert_eq!(
            (corner.0.w, corner.1.w),
            (6.0, 12.0),
            "a 6px source corner draws 12px wide"
        );
        let br = sp.iter().find(|(s, _)| s.x == 42.0 && s.y == 42.0).unwrap();
        assert_eq!(
            (br.1.x, br.1.y),
            (108.0, 68.0),
            "the bottom-right corner sits at the box corner"
        );
        let centre = sp
            .iter()
            .find(|(s, _)| s.x == 6.0 && s.y == 6.0 && s.w == 36.0)
            .unwrap();
        assert_eq!(
            (centre.1.x, centre.1.y, centre.1.w, centre.1.h),
            (12.0, 12.0, 96.0, 56.0),
            "the `fill` centre stretches inside the border"
        );
    }

    #[test]
    fn a_panel_smaller_than_its_borders_scales_them_down_and_defaults_to_border_only() {
        let tiny = panel(&json!({"type": "atlas-panel", "asset": asset_of("panel.png"), "borderSlice": 6, "borderWidth": 12, "style": {"width": 12, "height": 12}}), None).unwrap();
        let sp = sprites(&tiny);
        let corner = sp
            .iter()
            .find(|(s, _)| s.x == 0.0 && s.y == 0.0 && s.w == 6.0)
            .unwrap();
        assert_eq!(
            corner.1.w, 6.0,
            "12px box / (2 x 12px borders) halves the border"
        );
        let bare = panel(
            &json!({"type": "atlas-panel", "asset": asset_of("panel.png"), "borderWidth": 10}),
            None,
        )
        .unwrap();
        assert_eq!(
            (bare.packet.width, bare.packet.height),
            (20.0, 20.0),
            "no content and no size: just the border"
        );
    }

    #[test]
    fn repeat_mode_tiles_at_natural_size_and_crops_the_edge_tiles() {
        let p = panel(&json!({"type": "atlas-panel", "mode": "repeat", "asset": asset_of("gem.png"), "style": {"width": 100, "height": 48}}), None).unwrap();
        let sp = sprites(&p);
        assert_eq!(sp.len(), 3, "48 + 48 + a 4px sliver");
        assert_eq!((sp[2].0.w, sp[2].1.w, sp[2].1.x), (4.0, 4.0, 96.0));
        assert!(
            matches!(
                panel(
                    &json!({"type": "atlas-panel", "mode": "repeat", "asset": asset_of("gem.png")}),
                    None
                ),
                Err(CanvasSceneError::NothingToDraw(_))
            ),
            "a tiled box with no size is empty on the web too"
        );
    }
}
