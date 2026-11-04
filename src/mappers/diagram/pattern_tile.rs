use super::atlas_sprites;
use super::draw::{frame, Pen};
use super::{props_f32, props_str};
use crate::mappers::game_canvas_2d::CanvasSceneError;
use crate::mappers::learning::canvas::theme_for;
use crate::mappers::scene::MappedScene;
use orbital_core_base::language_data;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::OnceLock;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Geometry {
    unit: f32,
    variants: HashMap<String, Variant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Variant {
    height: f32,
    elements: Vec<Element>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Element {
    Path {
        d: String,
        #[serde(default)]
        opacity: Option<f32>,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
        #[serde(default)]
        filled: bool,
        #[serde(default)]
        opacity: Option<f32>,
    },
}

static GEOMETRY: OnceLock<Geometry> = OnceLock::new();

fn geometry() -> Result<&'static Geometry, CanvasSceneError> {
    if let Some(geometry) = GEOMETRY.get() {
        return Ok(geometry);
    }
    let installed = language_data::language_data().map_err(|error| {
        CanvasSceneError::NothingToDraw(format!("pattern-tile geometry: {error}"))
    })?;
    let registry: Value = serde_json::from_str(&installed.patterns)
        .map_err(|error| CanvasSceneError::NothingToDraw(format!("pattern registry: {error}")))?;
    let source = registry
        .pointer("/patterns/pattern-tile/geometry")
        .cloned()
        .ok_or_else(|| {
            CanvasSceneError::NothingToDraw(
                "pattern-tile geometry missing from installed contract".into(),
            )
        })?;
    let parsed: Geometry = serde_json::from_value(source).map_err(|error| {
        CanvasSceneError::NothingToDraw(format!("pattern-tile geometry invalid: {error}"))
    })?;
    Ok(GEOMETRY.get_or_init(|| parsed))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn svg_data_url(svg: &str) -> String {
    let mut url = String::from("data:image/svg+xml;utf8,");
    for byte in svg.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            url.push(byte as char);
        } else {
            write!(&mut url, "%{byte:02X}").expect("writing into a String cannot fail");
        }
    }
    url
}

fn tile_group(
    variant: &Variant,
    unit: f32,
    size: f32,
    stroke_width: f32,
    color: [f32; 4],
) -> String {
    let unit_stroke = stroke_width * unit / size;
    let hex = format!(
        "#{:02x}{:02x}{:02x}",
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    );
    let mut svg = format!(
        "<g transform=\"scale({})\" fill=\"none\" stroke=\"{hex}\" stroke-opacity=\"{}\" stroke-width=\"{unit_stroke}\">",
        size / unit,
        color[3].clamp(0.0, 1.0)
    );
    for element in &variant.elements {
        match element {
            Element::Path { d, opacity } => {
                let opacity = opacity
                    .map(|value| format!(" opacity=\"{value}\""))
                    .unwrap_or_default();
                write!(&mut svg, "<path d=\"{}\"{opacity}/>", xml_escape(d))
                    .expect("writing into a String cannot fail");
            }
            Element::Circle {
                cx,
                cy,
                r,
                filled,
                opacity,
            } => {
                let fill = if *filled { hex.as_str() } else { "none" };
                let opacity = opacity
                    .map(|value| format!(" opacity=\"{value}\""))
                    .unwrap_or_default();
                write!(&mut svg, "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"{fill}\" fill-opacity=\"{}\"{opacity}/>", color[3].clamp(0.0, 1.0))
                    .expect("writing into a String cannot fail");
            }
        }
    }
    svg.push_str("</g>");
    svg
}

fn svg_markup(
    variant: &Variant,
    unit: f32,
    size: f32,
    stroke_width: f32,
    color: [f32; 4],
) -> String {
    let height = size * variant.height / unit;
    let unit_stroke = stroke_width * unit / size;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{height}\" viewBox=\"0 0 {unit} {}\">{}</svg>",
        variant.height,
        tile_group(variant, unit, unit, unit_stroke, color)
    )
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let geometry = geometry()?;
    let name = props_str(props, "variant").unwrap_or("star8");
    let variant = geometry.variants.get(name).ok_or_else(|| {
        CanvasSceneError::NothingToDraw(format!(
            "pattern-tile variant `{name}` missing from installed geometry"
        ))
    })?;
    let size = props_f32(props, "size", 60.0);
    if !size.is_finite() || size <= 0.0 || geometry.unit <= 0.0 {
        return Err(CanvasSceneError::NothingToDraw(
            "pattern-tile size or geometry unit is nonpositive".into(),
        ));
    }
    let stroke_width = props_f32(props, "strokeWidth", 0.5);
    let theme = theme_for(props);
    let (xf, _, _) = frame(size, size * variant.height / geometry.unit, viewport);
    let pen = Pen::new(&theme, xf);
    let color = pen.paint(props_str(props, "color"), "primary");
    let svg = svg_markup(variant, geometry.unit, size, stroke_width, color);
    let width = size;
    let height = size * variant.height / geometry.unit;
    let image = json!({
        "type": "atlas-image",
        "asset": {"url": svg_data_url(&svg)},
        "width": width,
        "height": height,
        "fit": "fill",
        "canvasTheme": props.get("canvasTheme"),
    });
    atlas_sprites::image(&image, viewport)
}

fn geometric_svg(
    variant: &Variant,
    unit: f32,
    size: f32,
    stroke_width: f32,
    color: [f32; 4],
    width: f32,
    height: f32,
    opacity: f32,
    mode: &str,
) -> String {
    let tile_height = size * variant.height / unit;
    let tile = tile_group(variant, unit, size, stroke_width, color);
    let mask = match mode {
        "left" => "<linearGradient id=\"fade\" x1=\"0%\" y1=\"0%\" x2=\"60%\" y2=\"0%\"><stop offset=\"0%\" stop-color=\"white\" stop-opacity=\"1\"/><stop offset=\"100%\" stop-color=\"white\" stop-opacity=\"0\"/></linearGradient>",
        "right" => "<linearGradient id=\"fade\" x1=\"100%\" y1=\"0%\" x2=\"40%\" y2=\"0%\"><stop offset=\"0%\" stop-color=\"white\" stop-opacity=\"1\"/><stop offset=\"100%\" stop-color=\"white\" stop-opacity=\"0\"/></linearGradient>",
        "dual" => "<linearGradient id=\"fade\" x1=\"0%\" y1=\"0%\" x2=\"100%\" y2=\"0%\"><stop offset=\"0%\" stop-color=\"white\" stop-opacity=\"1\"/><stop offset=\"35%\" stop-color=\"white\" stop-opacity=\"0\"/><stop offset=\"65%\" stop-color=\"white\" stop-opacity=\"0\"/><stop offset=\"100%\" stop-color=\"white\" stop-opacity=\"1\"/></linearGradient>",
        "around" => "<radialGradient id=\"fade\" cx=\"50%\" cy=\"50%\" r=\"50%\"><stop offset=\"0%\" stop-color=\"white\" stop-opacity=\"0\"/><stop offset=\"55%\" stop-color=\"white\" stop-opacity=\"0\"/><stop offset=\"85%\" stop-color=\"white\" stop-opacity=\"1\"/><stop offset=\"100%\" stop-color=\"white\" stop-opacity=\"1\"/></radialGradient>",
        _ => "",
    };
    let mask_def = if mask.is_empty() {
        String::new()
    } else {
        format!("{mask}<mask id=\"mask\"><rect width=\"100%\" height=\"100%\" fill=\"url(#fade)\"/></mask>")
    };
    let mask_attr = if mask.is_empty() {
        ""
    } else {
        " mask=\"url(#mask)\""
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\"><defs><pattern id=\"tile\" x=\"0\" y=\"0\" width=\"{size}\" height=\"{tile_height}\" patternUnits=\"userSpaceOnUse\">{tile}</pattern>{mask_def}</defs><rect width=\"100%\" height=\"100%\" fill=\"url(#tile)\" opacity=\"{opacity}\"{mask_attr}/></svg>"
    )
}

pub fn map_geometric(
    props: &Value,
    viewport: Option<(f32, f32)>,
) -> Result<MappedScene, CanvasSceneError> {
    let geometry = geometry()?;
    let name = props_str(props, "variant").unwrap_or("star8");
    let variant = geometry.variants.get(name).ok_or_else(|| {
        CanvasSceneError::NothingToDraw(format!(
            "pattern-tile variant `{name}` missing from installed geometry"
        ))
    })?;
    let size = 60.0 * props_f32(props, "scale", 1.0);
    if !size.is_finite() || size <= 0.0 || geometry.unit <= 0.0 {
        return Err(CanvasSceneError::NothingToDraw(
            "geometric-pattern scale or geometry unit is nonpositive".into(),
        ));
    }
    let (width, height) = viewport.unwrap_or((size, size * variant.height / geometry.unit));
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(CanvasSceneError::NothingToDraw(
            "geometric-pattern viewport is nonpositive".into(),
        ));
    }
    let theme = theme_for(props);
    let (xf, _, _) = frame(width, height, viewport);
    let pen = Pen::new(&theme, xf);
    let color = pen.paint(props_str(props, "color"), "primary");
    let stroke_width = props_f32(props, "strokeWidth", 0.5);
    let opacity = props_f32(props, "opacity", 0.06).clamp(0.0, 1.0);
    let svg = geometric_svg(
        variant,
        geometry.unit,
        size,
        stroke_width,
        color,
        width,
        height,
        opacity,
        props_str(props, "mode").unwrap_or("background"),
    );
    let image = json!({
        "type": "atlas-image",
        "asset": {"url": svg_data_url(&svg)},
        "width": width,
        "height": height,
        "fit": "fill",
        "canvasTheme": props.get("canvasTheme"),
    });
    atlas_sprites::image(&image, viewport)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_to_svg_keeps_path_circle_order_and_stroke_size() {
        let variant = Variant {
            height: 150.0,
            elements: vec![
                Element::Path {
                    d: "M 0,0 L 100,0".into(),
                    opacity: Some(0.4),
                },
                Element::Circle {
                    cx: 50.0,
                    cy: 75.0,
                    r: 10.0,
                    filled: true,
                    opacity: None,
                },
            ],
        };
        let svg = svg_markup(&variant, 100.0, 50.0, 2.0, [1.0, 0.0, 0.0, 0.75]);
        assert!(svg.contains("height=\"75\" viewBox=\"0 0 100 150\""));
        assert!(svg.contains("stroke-width=\"4\""));
        assert!(svg.find("<path").unwrap() < svg.find("<circle").unwrap());
        assert!(svg.contains("fill=\"#ff0000\" fill-opacity=\"0.75\""));
        let url = svg_data_url(&svg);
        assert_eq!(crate::asset::image_info(&url), Some((50, 75)));
    }

    #[test]
    fn geometric_modes_tile_and_mask_the_viewport() {
        let variant = Variant {
            height: 100.0,
            elements: vec![Element::Circle {
                cx: 50.0,
                cy: 50.0,
                r: 50.0,
                filled: true,
                opacity: None,
            }],
        };
        let raster = |mode: &str| {
            let svg = geometric_svg(
                &variant,
                100.0,
                10.0,
                0.5,
                [1.0, 0.0, 0.0, 1.0],
                100.0,
                100.0,
                1.0,
                mode,
            );
            crate::svg::rasterize(
                &format!("test://geometric-{mode}"),
                svg.as_bytes(),
                100,
                100,
            )
            .expect("all modes produce an SVG raster")
        };
        let alpha =
            |raster: &crate::svg::Raster, x: usize, y: usize| raster.rgba[(y * 100 + x) * 4 + 3];
        for mode in ["background", "frame"] {
            let image = raster(mode);
            assert!(alpha(&image, 5, 45) > 200, "{mode} starts with a tile");
            assert!(alpha(&image, 95, 45) > 200, "{mode} repeats the tile");
        }
        let left = raster("left");
        assert!(alpha(&left, 5, 45) > alpha(&left, 95, 45));
        let right = raster("right");
        assert!(alpha(&right, 95, 45) > alpha(&right, 5, 45));
        let dual = raster("dual");
        assert!(alpha(&dual, 5, 45) > alpha(&dual, 45, 45));
        assert!(alpha(&dual, 95, 45) > alpha(&dual, 45, 45));
        let around = raster("around");
        assert!(alpha(&around, 5, 45) > alpha(&around, 45, 45));
    }
}
