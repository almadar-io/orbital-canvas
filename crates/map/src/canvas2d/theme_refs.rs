//! A 2D canvas drawn in its host's theme (`canvasTheme`). A colour slot (`fill`, `stroke`, `color`, `bgColor`, `tint`)
//! naming a theme colour key (first) or a diagram tone takes the theme's colour; a `font` whose family is `display`,
//! `body` or `mono` takes the theme's family, drawn from its font file when the theme names one.
//! Without a theme the names stay as given: tones and keys are not CSS colours, and the family keywords fall to
//! the generic faces.

use orbital_canvas_base::faces::register_face;
use orbital_canvas_base::packet::TextFamily;
use serde_json::{Map, Value};

const COLOR_SLOTS: [&str; 5] = ["fill", "stroke", "color", "bgColor", "tint"];
pub const FAMILY_KEYWORDS: [&str; 3] = ["display", "body", "mono"];

/// Resolve every theme reference under `props` against `theme`.
pub fn apply(props: &mut Map<String, Value>, theme: &Value) {
    let families = register_families(theme);
    walk_object(props, theme, &families);
}

/// The CSS family each keyword draws in, registering the theme's font files.
fn register_families(theme: &Value) -> Vec<(&'static str, String)> {
    FAMILY_KEYWORDS
        .iter()
        .filter_map(|keyword| {
            let font = theme.get("fonts")?.get(*keyword)?;
            let generic = font
                .get("generic")
                .and_then(Value::as_str)
                .unwrap_or("sans-serif");
            let family = font.get("family").and_then(Value::as_str).unwrap_or("");
            let file = font.get("file").and_then(Value::as_str).unwrap_or("");
            let named = register_face(family, file, generic_family(generic)).is_some();
            Some((
                *keyword,
                if named {
                    format!("\"{family}\"")
                } else {
                    generic.to_string()
                },
            ))
        })
        .collect()
}

fn generic_family(generic: &str) -> TextFamily {
    match generic {
        "serif" => TextFamily::Serif,
        "monospace" => TextFamily::Mono,
        _ => TextFamily::Sans,
    }
}

fn walk(value: &mut Value, theme: &Value, families: &[(&'static str, String)]) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|v| walk(v, theme, families)),
        Value::Object(map) => walk_object(map, theme, families),
        _ => {}
    }
}

fn walk_object(map: &mut Map<String, Value>, theme: &Value, families: &[(&'static str, String)]) {
    for (key, v) in map.iter_mut() {
        match v {
            Value::String(s) if COLOR_SLOTS.contains(&key.as_str()) => {
                if let Some(c) = theme_color(theme, s) {
                    *s = c;
                }
            }
            Value::String(s) if key == "font" => {
                if let Some(f) = theme_font(s, families) {
                    *s = f;
                }
            }
            _ => walk(v, theme, families),
        }
    }
}

fn theme_color(theme: &Value, name: &str) -> Option<String> {
    ["palette", "tones"]
        .iter()
        .find_map(|table| theme.get(*table)?.get(name)?.as_str())
        .map(str::to_string)
}

/// `600 26px display` → `600 26px "Limelight"`: the first family named after the size, when it is a keyword.
fn theme_font(font: &str, families: &[(&'static str, String)]) -> Option<String> {
    let tokens: Vec<&str> = font.split_whitespace().collect();
    let size_at = tokens.iter().position(|t| t.ends_with("px"))?;
    let rest = tokens[size_at + 1..].join(" ");
    let mut list = rest.split(',').map(str::trim);
    let first = list.next()?;
    let (_, css) = families.iter().find(|(k, _)| *k == first)?;
    let tail: Vec<&str> = list.collect();
    let family = if tail.is_empty() {
        css.clone()
    } else {
        format!("{css}, {}", tail.join(", "))
    };
    Some(format!("{} {family}", tokens[..=size_at].join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn theme() -> Value {
        json!({
            "tones": { "primary": "rgb(255, 72, 176)", "ink": "rgb(26, 22, 48)" },
            "palette": { "background": "rgb(247, 243, 232)", "card": "rgb(255, 253, 245)" },
            "fonts": {
                "display": { "family": "Theme Refs Test Display", "file": "https://fonts.test/display.ttf", "generic": "serif" },
                "body": { "family": "Theme Refs Test Body", "file": "", "generic": "sans-serif" }
            }
        })
    }

    #[test]
    fn tone_and_palette_names_in_colour_slots_take_the_theme_colour_and_literals_stay() {
        let mut props = json!({
            "bgColor": "background",
            "drawables": [
                { "type": "draw-shape", "fill": "primary", "stroke": "#112233", "shadow": { "color": "ink" } },
                { "type": "draw-text", "color": "card", "text": "primary" },
                { "type": "draw-shape", "fill": "red" }
            ]
        });
        apply(props.as_object_mut().unwrap(), &theme());
        assert_eq!(props["bgColor"], "rgb(247, 243, 232)");
        assert_eq!(props["drawables"][0]["fill"], "rgb(255, 72, 176)");
        assert_eq!(props["drawables"][0]["stroke"], "#112233");
        assert_eq!(props["drawables"][0]["shadow"]["color"], "rgb(26, 22, 48)");
        assert_eq!(props["drawables"][1]["color"], "rgb(255, 253, 245)");
        assert_eq!(
            props["drawables"][1]["text"], "primary",
            "only colour slots resolve"
        );
        assert_eq!(props["drawables"][2]["fill"], "red");
    }

    #[test]
    fn a_family_keyword_takes_the_theme_face_or_its_generic_when_the_theme_names_no_file() {
        let mut props = json!({ "drawables": [
            { "type": "draw-text", "font": "700 48px display" },
            { "type": "draw-text", "font": "400 16px body, sans-serif" },
            { "type": "draw-text", "font": "400 13px mono" },
            { "type": "draw-text", "font": "600 15px sans-serif" }
        ]});
        apply(props.as_object_mut().unwrap(), &theme());
        assert_eq!(
            props["drawables"][0]["font"],
            "700 48px \"Theme Refs Test Display\""
        );
        assert_eq!(
            props["drawables"][1]["font"],
            "400 16px sans-serif, sans-serif"
        );
        assert_eq!(
            props["drawables"][2]["font"], "400 13px mono",
            "no theme mono: the keyword stays"
        );
        assert_eq!(props["drawables"][3]["font"], "600 15px sans-serif");
        assert!(matches!(
            orbital_canvas_base::faces::face_named("theme refs test display"),
            Some(TextFamily::Face(_))
        ));
    }
}
