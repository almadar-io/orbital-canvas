//! The drawing axes of a learning canvas resolved to concrete values (the Rust
//! twin of `@almadar/core` `CanvasTheme`, which exists for "surfaces that cannot
//! evaluate CSS ... the native painters"). Tones and weights come from the
//! almadar-heritage theme (light/dark); a host that has resolved its own theme
//! passes it as the scene's `canvasTheme` (the `CanvasTheme` JSON shape) and it
//! overlays the defaults field by field.

use crate::mappers::game_canvas_2d::parse_css_color;
use crate::packet::{LineCap, LineJoin};
use serde_json::Value;
use std::collections::HashMap;

pub type Rgba = [f32; 4];

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FillStyle {
    Solid,
    Tint,
    Outline,
    Hatch,
}

impl FillStyle {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "solid" => Some(Self::Solid),
            "tint" => Some(Self::Tint),
            "outline" => Some(Self::Outline),
            "hatch" => Some(Self::Hatch),
            _ => None,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Marker {
    Triangle,
    Open,
    Line,
    Dot,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Weight {
    Thin,
    Normal,
    Bold,
}

impl Weight {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "thin" => Some(Self::Thin),
            "normal" => Some(Self::Normal),
            "bold" => Some(Self::Bold),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagramTheme {
    pub ground: Rgba,
    /// `canvasTheme.surfaces.muted` (`--color-muted`): the neutral wash of diagram chrome.
    pub surface_muted: Rgba,
    /// `canvasTheme.surfaces.card` (`--color-card`): card surfaces inside a diagram.
    pub surface_card: Rgba,
    /// `canvasTheme.surfaces.onPrimary` (`--color-primary-foreground`): text on a primary fill.
    pub surface_on_primary: Rgba,
    pub tones: HashMap<String, Rgba>,
    pub strokes: [f32; 3],
    pub line_cap: LineCap,
    pub line_join: LineJoin,
    pub dash: Vec<f32>,
    pub dot: Vec<f32>,
    pub fill_style: FillStyle,
    pub fill_opacity: f32,
    pub marker: Marker,
    pub label_size: String,
    pub label_weight: u16,
    pub label_uppercase: bool,
    pub text: HashMap<String, f32>,
    pub corner: f32,
    /// `canvasTheme.motion.enabled`: false means every animation draws its rest frame.
    pub motion: bool,
}

const TONES: [&str; 22] = [
    "ink",
    "label",
    "guide",
    "axis",
    "grid",
    "highlight",
    "fill",
    "series-1",
    "series-2",
    "series-3",
    "series-4",
    "series-5",
    "series-6",
    "series-7",
    "series-8",
    "primary",
    "accent",
    "success",
    "warning",
    "error",
    "info",
    "muted",
];

const HERITAGE_LIGHT: [&str; 22] = [
    "#1f2d3d", "#334155", "#8a8f98", "#64748b", "#e4e0d8", "#a07a10", "#00767c", "#00767c",
    "#7d7d12", "#215eac", "#a4444c", "#536225", "#7a4a8a", "#875c27", "#6b7689", "#00767c",
    "#c9a227", "#15803d", "#b45309", "#b42318", "#00767c", "#475569",
];

const HERITAGE_DARK: [&str; 22] = [
    "#e6e1d4", "#c9ced8", "#7f8aa0", "#98a2b6", "#2a3650", "#c3ae41", "#00b1b8", "#0a98a0",
    "#a9841b", "#568acf", "#b86a60", "#7d9146", "#a379b1", "#b57b43", "#758091", "#00b1b8",
    "#d4b44a", "#4ade80", "#fbbf24", "#f87171", "#2cc7cd", "#9aa6b8",
];

fn hex(s: &str) -> Rgba {
    parse_css_color(s).unwrap_or([0.0, 0.0, 0.0, 1.0])
}

impl DiagramTheme {
    pub fn heritage(dark: bool) -> Self {
        let palette = if dark {
            &HERITAGE_DARK
        } else {
            &HERITAGE_LIGHT
        };
        let tones = TONES
            .iter()
            .zip(palette.iter())
            .map(|(name, c)| (name.to_string(), hex(c)))
            .collect();
        let text = [
            ("xs", 12.0),
            ("sm", 14.0),
            ("base", 16.0),
            ("lg", 18.0),
            ("xl", 20.0),
            ("2xl", 24.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        Self {
            ground: hex(if dark { "#152036" } else { "#ffffff" }),
            surface_muted: hex(if dark { "#1a2436" } else { "#f1eee8" }),
            surface_card: hex(if dark { "#152036" } else { "#ffffff" }),
            surface_on_primary: hex(if dark { "#0f172a" } else { "#ffffff" }),
            tones,
            strokes: [1.0, 1.5, 2.5],
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            dash: vec![6.0, 4.0],
            dot: vec![1.0, 4.0],
            fill_style: FillStyle::Tint,
            fill_opacity: 0.16,
            marker: Marker::Triangle,
            label_size: "sm".into(),
            label_weight: 600,
            label_uppercase: false,
            text,
            corner: 6.0,
            motion: true,
        }
    }

    /// The scene's theme: heritage for its `colorScheme`, then its `canvasTheme`
    /// (a resolved `CanvasTheme`) laid over it.
    pub fn from_scene(props: &Value) -> Self {
        let dark = props.get("colorScheme").and_then(Value::as_str) == Some("dark");
        let mut theme = Self::heritage(dark);
        if let Some(ct) = props.get("canvasTheme").filter(|v| v.is_object()) {
            theme.overlay(ct);
        }
        theme
    }

    fn overlay(&mut self, ct: &Value) {
        let color = |v: &Value| v.as_str().and_then(parse_css_color);
        if let Some(c) = ct.get("ground").and_then(color) {
            self.ground = c;
        }
        if let Some(c) = ct.pointer("/surfaces/muted").and_then(color) {
            self.surface_muted = c;
        }
        if let Some(c) = ct.pointer("/surfaces/card").and_then(color) {
            self.surface_card = c;
        }
        if let Some(c) = ct.pointer("/surfaces/onPrimary").and_then(color) {
            self.surface_on_primary = c;
        }
        if let Some(tones) = ct.get("tones").and_then(Value::as_object) {
            for (name, v) in tones {
                if let Some(c) = color(v) {
                    self.tones.insert(name.clone(), c);
                }
            }
        }
        if let Some(st) = ct.get("strokes") {
            for (i, key) in ["thin", "normal", "bold"].iter().enumerate() {
                if let Some(w) = st.get(*key).and_then(Value::as_f64) {
                    self.strokes[i] = w as f32;
                }
            }
        }
        let list = |v: Option<&Value>| -> Option<Vec<f32>> {
            v?.as_array()?
                .iter()
                .map(|n| n.as_f64().map(|f| f as f32))
                .collect()
        };
        if let Some(d) = list(ct.get("dash")) {
            self.dash = d;
        }
        if let Some(d) = list(ct.get("dot")) {
            self.dot = d;
        }
        match ct.get("lineCap").and_then(Value::as_str) {
            Some("butt") => self.line_cap = LineCap::Butt,
            Some("round") => self.line_cap = LineCap::Round,
            Some("square") => self.line_cap = LineCap::Square,
            _ => {}
        }
        match ct.get("lineJoin").and_then(Value::as_str) {
            Some("miter") => self.line_join = LineJoin::Miter,
            Some("round") => self.line_join = LineJoin::Round,
            Some("bevel") => self.line_join = LineJoin::Bevel,
            _ => {}
        }
        if let Some(f) = ct
            .get("fillStyle")
            .and_then(Value::as_str)
            .and_then(FillStyle::parse)
        {
            self.fill_style = f;
        }
        if let Some(o) = ct.get("fillOpacity").and_then(Value::as_f64) {
            self.fill_opacity = o as f32;
        }
        match ct.get("marker").and_then(Value::as_str) {
            Some("triangle") => self.marker = Marker::Triangle,
            Some("open") => self.marker = Marker::Open,
            Some("line") => self.marker = Marker::Line,
            Some("dot") => self.marker = Marker::Dot,
            _ => {}
        }
        if let Some(label) = ct.get("label") {
            if let Some(s) = label.get("size").and_then(Value::as_str) {
                self.label_size = s.to_string();
            }
            if let Some(w) = label
                .get("weight")
                .and_then(Value::as_str)
                .and_then(|w| w.parse().ok())
            {
                self.label_weight = w;
            }
            if let Some(u) = label.get("uppercase").and_then(Value::as_bool) {
                self.label_uppercase = u;
            }
        }
        if let Some(on) = ct
            .get("motion")
            .and_then(|m| m.get("enabled"))
            .and_then(Value::as_bool)
        {
            self.motion = on;
        }
        if let Some(text) = ct.get("text").and_then(Value::as_object) {
            for (name, v) in text {
                if let Some(px) = v.as_f64() {
                    self.text.insert(name.clone(), px as f32);
                }
            }
        }
        if let Some(c) = ct.get("corner").and_then(Value::as_f64) {
            self.corner = c as f32;
        }
    }

    /// A tone's color. Tones are the closed `DIAGRAM_TONES` set, all present.
    pub fn tone(&self, name: &str) -> Option<Rgba> {
        self.tones.get(name).copied()
    }

    pub fn stroke_width(&self, w: Weight) -> f32 {
        self.strokes[match w {
            Weight::Thin => 0,
            Weight::Normal => 1,
            Weight::Bold => 2,
        }]
    }

    /// A text size step in px; an unknown step is a caller bug, not a fallback.
    pub fn text_px(&self, step: &str) -> f32 {
        self.text
            .get(step)
            .copied()
            .unwrap_or_else(|| self.text["sm"])
    }

    /// `series-N` for a zero-based index, wrapping over the eight series tones.
    pub fn series_tone(index: usize) -> String {
        format!("series-{}", (index % 8) + 1)
    }

    /// Replace the categorical palette: `series-1..N` resolve to `colors`
    /// (each a tone name or literal), cycling when fewer than eight.
    pub fn with_series(mut self, colors: &[String]) -> Self {
        if colors.is_empty() {
            return self;
        }
        let resolved: Vec<Rgba> = colors
            .iter()
            .enumerate()
            .map(|(i, c)| self.mark_color(Some(c), &Self::series_tone(i)))
            .collect();
        for i in 0..8 {
            self.tones
                .insert(Self::series_tone(i), resolved[i % resolved.len()]);
        }
        self
    }

    /// `markColor`: a tone name resolves through the theme, a CSS literal parses,
    /// absent or unresolvable falls to the `fallback` tone (and an unresolvable
    /// literal is logged once per value).
    pub fn mark_color(&self, value: Option<&str>, fallback: &str) -> Rgba {
        let fallback_color = || self.tone(fallback).unwrap_or(self.ground);
        match value {
            None | Some("") => fallback_color(),
            Some(v) => {
                if let Some(c) = self.tone(v) {
                    c
                } else if let Some(c) = parse_css_color(v) {
                    c
                } else {
                    crate::mappers::learning::gap_once(
                        "color-literal",
                        &format!("unresolvable color `{v}` — fell back to the {fallback} tone"),
                    );
                    fallback_color()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dark_scheme_swaps_the_palette_and_ground_light_does_not() {
        let light = DiagramTheme::from_scene(&json!({}));
        let dark = DiagramTheme::from_scene(&json!({"colorScheme": "dark"}));
        assert_ne!(light.ground, dark.ground);
        assert_ne!(light.tone("ink"), dark.tone("ink"));
        assert_eq!(light.tone("series-1"), Some(hex("#00767c")));
        assert_eq!(dark.tone("series-1"), Some(hex("#0a98a0")));
    }

    #[test]
    fn motion_is_on_unless_the_canvas_theme_turns_it_off() {
        assert!(DiagramTheme::from_scene(&json!({})).motion);
        let off = json!({"canvasTheme": {"motion": {"enabled": false}}});
        assert!(!DiagramTheme::from_scene(&off).motion);
        let on = json!({"canvasTheme": {"motion": {"enabled": true}}});
        assert!(
            DiagramTheme::from_scene(&on).motion,
            "control: enabled keeps it on"
        );
        let silent = json!({"canvasTheme": {"ground": "#000000"}});
        assert!(
            DiagramTheme::from_scene(&silent).motion,
            "control: a theme without motion keeps it on"
        );
    }

    #[test]
    fn canvas_theme_surfaces_overlay_the_muted_and_card_surfaces() {
        let t = DiagramTheme::from_scene(&json!({
            "canvasTheme": {"surfaces": {"muted": "rgb(10, 20, 30)", "card": "#ff0000"}}
        }));
        assert_eq!(t.surface_muted, hex("#0a141e"));
        assert_eq!(t.surface_card, hex("#ff0000"));
        let plain = DiagramTheme::from_scene(&json!({}));
        assert_eq!(
            plain.surface_muted,
            DiagramTheme::heritage(false).surface_muted,
            "control: heritage default"
        );
    }

    #[test]
    fn a_resolved_canvas_theme_overlays_only_the_fields_it_carries() {
        let t = DiagramTheme::from_scene(&json!({
            "canvasTheme": {"ground": "#101010", "tones": {"ink": "#ff0000"}, "strokes": {"bold": 4}}
        }));
        assert_eq!(t.ground, hex("#101010"));
        assert_eq!(t.tone("ink"), Some(hex("#ff0000")));
        assert_eq!(
            t.tone("grid"),
            Some(hex("#e4e0d8")),
            "untouched tone keeps the default"
        );
        assert_eq!(t.stroke_width(Weight::Bold), 4.0);
        assert_eq!(t.stroke_width(Weight::Thin), 1.0);
    }

    #[test]
    fn mark_color_resolves_tone_then_literal_then_fallback() {
        let t = DiagramTheme::heritage(false);
        assert_eq!(t.mark_color(Some("axis"), "ink"), t.tone("axis").unwrap());
        assert_eq!(t.mark_color(Some("#336699"), "ink"), hex("#336699"));
        assert_eq!(t.mark_color(None, "grid"), t.tone("grid").unwrap());
        assert_eq!(
            t.mark_color(Some("not-a-color(1)"), "ink"),
            t.tone("ink").unwrap()
        );
    }

    #[test]
    fn series_override_cycles_over_the_eight_series_tones() {
        let t = DiagramTheme::heritage(false).with_series(&["#ff0000".into(), "#00ff00".into()]);
        assert_eq!(t.tone("series-1"), Some(hex("#ff0000")));
        assert_eq!(t.tone("series-2"), Some(hex("#00ff00")));
        assert_eq!(t.tone("series-3"), Some(hex("#ff0000")));
        assert_eq!(
            t.tone("ink"),
            Some(hex("#1f2d3d")),
            "non-series tones untouched"
        );
    }
}
