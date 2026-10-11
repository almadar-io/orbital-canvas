//! Typed display-list packet consumed by the shared game renderer.

use serde::{Deserialize, Serialize};

/// Top-level packet describing one frame of game rendering.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameRenderPacket {
    /// Pattern name that produced this packet, e.g. "isometric-canvas".
    pub pattern: String,
    /// Surface width in logical pixels.
    pub width: f32,
    /// Surface height in logical pixels.
    pub height: f32,
    /// Optional background image URL/path.
    pub background_image: Option<String>,
    /// Base URL for resolving relative asset paths.
    pub asset_base_url: Option<String>,
    /// Optional 2D camera transform.
    pub camera: Option<Camera>,
    /// Ordered list of draw commands.
    pub draw_commands: Vec<DrawCommand>,
    /// Event name to emit each simulation tick.
    pub tick_event: Option<String>,
    /// Event name to emit each render frame.
    pub draw_event: Option<String>,
    /// Target frames per second.
    pub fps: Option<u32>,
    /// Time-driven motion over ranges of `draw_commands`; `draw_commands` holds each range's
    /// rest frame. [`GameRenderPacket::resolved`] plays them at a host clock time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animations: Vec<Animation>,
}

/// CSS timing function between two keyframes.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Easing {
    #[default]
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
}

/// One keyframe: the properties it names are interpolated, the others stay as the rest frame draws
/// them. `scale` and `rotate` (degrees, absolute) act about the animation's origin.
#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize)]
pub struct Keyframe {
    /// Position in the cycle, 0 to 1.
    pub offset: f32,
    #[serde(default)]
    pub scale: Option<f32>,
    #[serde(default)]
    pub rotate: Option<f32>,
    /// Multiplier on the alpha of every mark in the range.
    #[serde(default)]
    pub opacity: Option<f32>,
    /// Phase of the dash pattern of every stroke in the range.
    #[serde(default)]
    pub dash_offset: Option<f32>,
}

/// A CSS keyframe animation over `draw_commands[first..end]`, in the host's clock time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Animation {
    pub first: usize,
    pub end: usize,
    pub duration_ms: f32,
    /// Before it elapses the range shows its rest frame (CSS `animation-delay`, no fill).
    #[serde(default)]
    pub delay_ms: f32,
    /// Cycles to play; absent is infinite. After the last the range shows its rest frame.
    #[serde(default)]
    pub iterations: Option<u32>,
    #[serde(default)]
    pub easing: Easing,
    /// Transform origin in packet units.
    pub origin: Vec2,
    /// The rotation (degrees) the rest frame already carries, which a keyframe `rotate` replaces.
    #[serde(default)]
    pub base_rotation: f32,
    pub keyframes: Vec<Keyframe>,
}

/// 2D camera state.
#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
    pub rotation: f32,
}

/// Axis-aligned rectangle.
#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 2D vector.
#[derive(Debug, Copy, Clone, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// A single tile in a `TileGrid`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tile {
    pub row: u32,
    pub col: u32,
    pub kind: String,
}

/// Horizontal text alignment against the anchor x.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

fn default_text_weight() -> u16 {
    400
}

/// Outline drawn under text.
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct Halo {
    pub color: [f32; 4],
    pub width: f32,
}

/// Stroke end style.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineCap {
    Butt,
    #[default]
    Round,
    Square,
}

/// Stroke corner style.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineJoin {
    Miter,
    #[default]
    Round,
    Bevel,
}

/// Stroke style of a [`DrawCommand::Path`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stroke {
    pub color: [f32; 4],
    pub width: f32,
    /// Alternating on/off lengths; empty is a solid line.
    #[serde(default)]
    pub dash: Vec<f32>,
    /// Phase of the dash pattern (SVG `stroke-dashoffset`).
    #[serde(default)]
    pub dash_offset: f32,
    #[serde(default)]
    pub cap: LineCap,
    #[serde(default)]
    pub join: LineJoin,
}

/// One path operation. Angles are radians in screen convention (0 = +x,
/// increasing clockwise because y points down); an `Arc` sweeps from `start`
/// to `end` in the direction of the difference, a full ellipse when it spans
/// 2 pi or more, and is joined to the current point by a line like canvas
/// `ellipse()`.
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum PathSegment {
    MoveTo {
        x: f32,
        y: f32,
    },
    LineTo {
        x: f32,
        y: f32,
    },
    Arc {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        start: f32,
        end: f32,
    },
    /// A cubic Bezier from the current point; SVG `Q`/`T`/`A` arrive elevated to cubics.
    CubicTo {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x: f32,
        y: f32,
    },
    Close,
}

/// One drawing primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DrawCommand {
    /// Fill the entire surface with a color.
    Clear { color: [f32; 4] },
    /// Draw a textured quad (sprite).
    Sprite {
        texture: String,
        src: Rect,
        dst: Rect,
        rotation: f32,
        opacity: f32,
        flip_x: bool,
        flip_y: bool,
    },
    /// Draw a solid colored quad.
    Quad { rect: Rect, color: [f32; 4] },
    /// Draw a filled convex polygon (triangle fan from `points[0]`).
    Poly { points: Vec<Vec2>, color: [f32; 4] },
    /// Draw a line segment.
    Line {
        from: Vec2,
        to: Vec2,
        color: [f32; 4],
        width: f32,
    },
    /// Draw text anchored at `pos`: `align` places the string horizontally
    /// against `pos.x`, and `pos.y` is the vertical middle of the em box
    /// (canvas `textBaseline = "middle"`). `size` is the font size in logical
    /// units; a `weight` of 600 or more is drawn emboldened. A `halo` is a
    /// ground-colored outline drawn first so the label stays legible where it
    /// crosses a mark.
    Text {
        text: String,
        pos: Vec2,
        size: f32,
        color: [f32; 4],
        #[serde(default)]
        align: TextAlign,
        #[serde(default = "default_text_weight")]
        weight: u16,
        #[serde(default)]
        halo: Option<Halo>,
        /// Draw in the monospace face (the theme's `mono` label font).
        #[serde(default)]
        mono: bool,
    },
    /// Draw a vector path: filled, stroked, or both (fill first). Rasterized
    /// anti-aliased in the vector layer.
    Path {
        segments: Vec<PathSegment>,
        #[serde(default)]
        fill: Option<[f32; 4]>,
        #[serde(default)]
        stroke: Option<Stroke>,
    },
    /// A path painted only where every `clip` region covers and no `cut` region does
    /// (canvas `clip()` then `destination-out`): venn regions and hatch fills.
    ClippedPath {
        segments: Vec<PathSegment>,
        #[serde(default)]
        fill: Option<[f32; 4]>,
        #[serde(default)]
        stroke: Option<Stroke>,
        #[serde(default)]
        clip: Vec<Vec<PathSegment>>,
        #[serde(default)]
        cut: Vec<Vec<PathSegment>>,
    },
    /// Draw a grid of tiles.
    TileGrid {
        rows: u32,
        cols: u32,
        tile_width: f32,
        tile_height: f32,
        tiles: Vec<Tile>,
    },
    /// Restrict drawing to a rectangle.
    Scissor { rect: Rect },
    /// Remove the scissor rectangle.
    ResetScissor,
}
