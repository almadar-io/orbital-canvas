//! 2D projector — logical scene coordinates → pixel space for one projection
//! (iso/hex/flat/free). Exact port of `@almadar/ui`'s
//! `lib/drawable/projector.ts` (`create2DProjector`) grounded in the canonical
//! pure math in `lib/isometric.ts` (`isoToScreen` / `screenToIso`, constants
//! `TILE_WIDTH` / `FLOOR_HEIGHT` / `DIAMOND_TOP_Y`). Drawable primitives stay
//! projection-agnostic: they hand a `ScenePos` here and get pixels back.

/// Base tile width in px before scale (`isometric.ts` TILE_WIDTH).
pub const TILE_WIDTH: f32 = 256.0;
/// Floor diamond height — the walkable surface portion (`isometric.ts` FLOOR_HEIGHT).
pub const FLOOR_HEIGHT: f32 = 128.0;
/// Measured Y offset from sprite top to the diamond top vertex within a Kenney
/// 256×512 tile sprite (`isometric.ts` DIAMOND_TOP_Y).
pub const DIAMOND_TOP_Y: f32 = 374.0;

/// Projection modes the 2D host supports. `.lolo` `side` maps to `Free` at the
/// call site (world-pixel-direct), matching `projector.ts` (`Projection2D = TileLayout | 'free'`).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Projection {
    Isometric,
    Hex,
    Flat,
    Free,
}

impl Projection {
    /// Parse the `.lolo` `projection` prop. Unknown strings → gap log + `Free`
    /// would be a heuristic, so callers must only pass the four authored values.
    pub fn from_str(s: &str) -> Option<Projection> {
        match s {
            "isometric" | "iso" => Some(Projection::Isometric),
            "hex" => Some(Projection::Hex),
            "flat" => Some(Projection::Flat),
            "free" | "side" => Some(Projection::Free),
            _ => None,
        }
    }
}

/// Logical scene position. Grounded mirror of `@almadar/core`'s `ScenePos`
/// (`{ x: number; y: number; z?: number }` — `z` is 3D-only, unused here).
#[derive(Debug, Copy, Clone, Default)]
pub struct ScenePos {
    pub x: f32,
    pub y: f32,
}

/// How a drawable aligns to its projected position
/// (`lib/drawable/contract.ts` `DrawableAnchor`).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    Ground,
    Center,
}

impl Anchor {
    pub fn from_str(s: &str) -> Option<Anchor> {
        match s {
            "top-left" => Some(Anchor::TopLeft),
            "ground" => Some(Anchor::Ground),
            "center" => Some(Anchor::Center),
            _ => None,
        }
    }
}

/// Maps logical scene coordinates to pixel space for one projection.
/// Owns ALL projection geometry (`projector.ts` `Projector`).
pub struct Projector {
    pub tile_width: f32,
    pub floor_height: f32,
    pub diamond_top_y: f32,
    pub scale: f32,
    pub base_offset_x: f32,
    pub layout: Projection,
    pub square_grid: bool,
    /// A world-axis plot (`MathCanvas`'s drawables projector) instead of a tile grid.
    pub plot: Option<PlotAxes>,
}

/// World to canvas pixels of a plot: `x = ox + wx * sx`, `y = oy - wy * sy` (y up).
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PlotAxes {
    pub ox: f32,
    pub oy: f32,
    pub sx: f32,
    pub sy: f32,
}

impl Projector {
    pub fn new(scale: f32, base_offset_x: f32, layout: Projection) -> Projector {
        Projector {
            tile_width: TILE_WIDTH * scale,
            floor_height: FLOOR_HEIGHT * scale,
            diamond_top_y: DIAMOND_TOP_Y * scale,
            scale,
            base_offset_x,
            layout,
            square_grid: matches!(layout, Projection::Flat | Projection::Free),
            plot: None,
        }
    }

    /// `MathCanvas`'s projector: one world-x unit is `sx` pixels, a square-grid cell is one
    /// world-x unit, and anchors offset in pixels like any square grid.
    pub fn plot(axes: PlotAxes) -> Projector {
        Projector {
            tile_width: axes.sx,
            floor_height: axes.sx / 2.0,
            diamond_top_y: 0.0,
            scale: 1.0,
            base_offset_x: 0.0,
            layout: Projection::Flat,
            square_grid: true,
            plot: Some(axes),
        }
    }

    /// Cell top-left in pixels (`isoToScreen`).
    pub fn project(&self, pos: ScenePos) -> (f32, f32) {
        if let Some(a) = self.plot {
            return (a.ox + pos.x * a.sx, a.oy - pos.y * a.sy);
        }
        match self.layout {
            Projection::Free => (pos.x, pos.y),
            Projection::Hex => (
                pos.x * self.tile_width
                    + ((pos.y as i64 & 1) as f32) * (self.tile_width / 2.0)
                    + self.base_offset_x,
                pos.y * (self.floor_height * 0.75),
            ),
            Projection::Flat => (
                pos.x * self.tile_width + self.base_offset_x,
                pos.y * self.tile_width,
            ),
            Projection::Isometric => (
                (pos.x - pos.y) * (self.tile_width / 2.0) + self.base_offset_x,
                (pos.x + pos.y) * (self.floor_height / 2.0),
            ),
        }
    }

    /// The pixel reference point for an anchor.
    pub fn anchor_point(&self, pos: ScenePos, anchor: Anchor) -> (f32, f32) {
        let base = self.project(pos);
        if anchor == Anchor::TopLeft {
            return base;
        }
        let cx = base.0 + self.tile_width / 2.0;
        if anchor == Anchor::Ground {
            let gy = if self.square_grid {
                base.1 + self.tile_width * 0.92
            } else {
                base.1 + self.diamond_top_y + self.floor_height * 0.5
            };
            return (cx, gy);
        }
        let cy = if self.square_grid {
            base.1 + self.tile_width / 2.0
        } else {
            base.1 + self.diamond_top_y + self.floor_height * 0.5
        };
        (cx, cy)
    }

    /// The cell footprint polygon — a rect (square grids) or a diamond (iso/hex).
    pub fn cell_path(&self, pos: ScenePos) -> [(f32, f32); 4] {
        let base = self.project(pos);
        if self.square_grid {
            return [
                (base.0, base.1),
                (base.0 + self.tile_width, base.1),
                (base.0 + self.tile_width, base.1 + self.tile_width),
                (base.0, base.1 + self.tile_width),
            ];
        }
        let cx = base.0 + self.tile_width / 2.0;
        let top_y = base.1 + self.diamond_top_y;
        [
            (cx, top_y),
            (base.0 + self.tile_width, top_y + self.floor_height / 2.0),
            (cx, top_y + self.floor_height),
            (base.0, top_y + self.floor_height / 2.0),
        ]
    }

    /// Inverse of `project` at the projected-pixel level (`screenToIso`).
    /// Returns the rounded scene cell. Pointer→scene at the canvas level must
    /// first invert the camera transform (see the canvas host).
    pub fn unproject(&self, screen_x: f32, screen_y: f32) -> (i32, i32) {
        match self.layout {
            Projection::Free => (screen_x.round() as i32, screen_y.round() as i32),
            Projection::Hex => {
                let row = (screen_y / (self.floor_height * 0.75)).round() as i32;
                let col =
                    ((screen_x - ((row & 1) as f32) * self.tile_width / 2.0 - self.base_offset_x)
                        / self.tile_width)
                        .round() as i32;
                (col, row)
            }
            Projection::Flat => (
                ((screen_x - self.base_offset_x) / self.tile_width).round() as i32,
                (screen_y / self.tile_width).round() as i32,
            ),
            Projection::Isometric => {
                let ax = screen_x - self.base_offset_x;
                let tile_x =
                    (ax / (self.tile_width / 2.0) + screen_y / (self.floor_height / 2.0)) / 2.0;
                let tile_y =
                    (screen_y / (self.floor_height / 2.0) - ax / (self.tile_width / 2.0)) / 2.0;
                (tile_x.round() as i32, tile_y.round() as i32)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < EPS, "{a} != {b}");
    }

    #[test]
    fn iso_project_matches_reference() {
        let p = Projector::new(1.0, 0.0, Projection::Isometric);
        let (x, y) = p.project(ScenePos { x: 1.0, y: 1.0 });
        close(x, 0.0);
        close(y, 128.0);
        let (x, y) = p.project(ScenePos { x: 2.0, y: 0.0 });
        close(x, 256.0);
        close(y, 128.0);
    }

    #[test]
    fn flat_project_matches_reference() {
        let p = Projector::new(1.0, 0.0, Projection::Flat);
        let (x, y) = p.project(ScenePos { x: 2.0, y: 3.0 });
        close(x, 512.0);
        close(y, 768.0);
    }

    #[test]
    fn hex_project_matches_reference() {
        let p = Projector::new(1.0, 0.0, Projection::Hex);
        let (x, y) = p.project(ScenePos { x: 1.0, y: 1.0 });
        close(x, 256.0 + 128.0);
        close(y, 96.0);
    }

    #[test]
    fn free_is_identity() {
        let p = Projector::new(0.4, 0.0, Projection::Free);
        let (x, y) = p.project(ScenePos { x: 17.5, y: -3.0 });
        close(x, 17.5);
        close(y, -3.0);
    }

    #[test]
    fn anchor_ground_uses_diamond_top_for_iso() {
        let p = Projector::new(1.0, 0.0, Projection::Isometric);
        let base = p.project(ScenePos { x: 1.0, y: 1.0 });
        let (x, y) = p.anchor_point(ScenePos { x: 1.0, y: 1.0 }, Anchor::Ground);
        close(x, base.0 + 128.0);
        close(y, base.1 + 374.0 + 64.0);
    }

    #[test]
    fn cell_path_diamond_for_iso() {
        let p = Projector::new(1.0, 0.0, Projection::Isometric);
        let pts = p.cell_path(ScenePos { x: 0.0, y: 0.0 });
        close(pts[0].0, 128.0);
        close(pts[0].1, 374.0);
        close(pts[1].0, 256.0);
        close(pts[1].1, 374.0 + 64.0);
        close(pts[2].0, 128.0);
        close(pts[2].1, 374.0 + 128.0);
        close(pts[3].0, 0.0);
        close(pts[3].1, 374.0 + 64.0);
    }

    #[test]
    fn unproject_round_trips_all_layouts() {
        for layout in [
            Projection::Isometric,
            Projection::Hex,
            Projection::Flat,
            Projection::Free,
        ] {
            let p = Projector::new(0.4, 123.0, layout);
            for (sx, sy) in [(3.0, 5.0), (0.0, 0.0), (7.0, 2.0)] {
                let (px, py) = p.project(ScenePos { x: sx, y: sy });
                let (rx, ry) = p.unproject(px, py);
                assert_eq!(
                    (rx, ry),
                    (sx as i32, sy as i32),
                    "layout {layout:?} ({sx},{sy})"
                );
            }
        }
    }
}
