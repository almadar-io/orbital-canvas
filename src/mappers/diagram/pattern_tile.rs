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

fn svg_markup(
    variant: &Variant,
    unit: f32,
    size: f32,
    stroke_width: f32,
    color: [f32; 4],
) -> String {
    let height = size * variant.height / unit;
    let unit_stroke = stroke_width * unit / size;
    let hex = format!(
        "#{:02x}{:02x}{:02x}",
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    );
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{height}\" viewBox=\"0 0 {unit} {}\"><g fill=\"none\" stroke=\"{hex}\" stroke-opacity=\"{}\" stroke-width=\"{unit_stroke}\">",
        variant.height,
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
    svg.push_str("</g></svg>");
    svg
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
}
