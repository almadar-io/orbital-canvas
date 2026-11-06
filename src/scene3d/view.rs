//! The interactive side of a 3D canvas, shared by every host: a port of three-stdlib's `OrbitControls` as the host
//! configures it (left drag rotates, wheel or middle drag dollies, right drag pans; one finger rotates, two fingers
//! dolly and pan; damping 0.05; distance 2..100; polar angle at most pi/2 - 0.1), picking that resolves a pointer
//! to the scene's own click events, `Canvas3DHost`'s scene-edit gestures (select, drag with a live preview cell,
//! move on drop), `FollowCamera3D`'s easing toward the follow pose, and the host's imperative camera handle.

use super::model::{Camera, Projection, Scene3D};
use super::render::view_projection;
use glam::{Mat4, Vec3};
use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};

const DAMPING: f32 = 0.05;
const MIN_DISTANCE: f32 = 2.0;
const MAX_DISTANCE: f32 = 100.0;
const MAX_POLAR: f32 = PI / 2.0 - 0.1;
const ZOOM_SCALE: f32 = 0.95;
const CLICK_SLOP: f32 = 5.0;
/// `Canvas3DHost`'s edit drag gate: client `|dx| + |dy|` from the pointer-down.
const EDIT_DRAG_GATE: f32 = 5.0;
/// `FollowCamera3D` lerps by `min(1, delta * 5)` each frame.
const FOLLOW_RATE: f32 = 5.0;
/// The host's invisible ground plane spans the board plus two cells on every side.
const GROUND_MARGIN: f32 = 2.0;

/// What a click resolved to, in scene coordinates (scene `y` is the ground axis three calls `z`).
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Unit { id: String, x: f32, z: f32 },
    Tile { x: i64, z: i64 },
}

/// What a release resolved to: a click outside edit mode, or an edit-mode selection or move (scene cells).
#[derive(Debug, Clone, PartialEq)]
pub enum Release {
    Pick(Pick),
    Select { id: Option<String> },
    Move { id: String, x: i64, y: i64 },
}

/// Scene-edit mode as the pattern declares it (`editable`, the controlled `selectedId`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditMode {
    pub selected: Option<String>,
}

/// The cell the dragged drawable previews at; the host re-maps the scene with it until the drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragPreview {
    pub id: String,
    pub x: i64,
    pub y: i64,
}

/// The drawn board's scene bounds; the ground plane picks only within them plus the host's margin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundExtent {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
}

/// A `follow`/`chase` camera: the mapped camera is the pose it eases toward, starting at `start_eye` (the
/// framing position three's canvas is created with) when no camera came before.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Follow {
    pub start_eye: [f32; 3],
}

#[derive(Debug, Clone)]
struct EditDrag {
    id: String,
    start: (f32, f32),
    moved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Middle,
    Secondary,
}

#[derive(Debug, Clone, Copy)]
enum Gesture {
    Rotate,
    Dolly,
    Pan,
    TouchDollyPan,
}

#[derive(Debug, Clone, Copy)]
struct Spherical {
    radius: f32,
    phi: f32,
    theta: f32,
}

impl Spherical {
    fn from_offset(v: Vec3) -> Self {
        let radius = v.length();
        if radius == 0.0 {
            return Self {
                radius,
                phi: 0.0,
                theta: 0.0,
            };
        }
        Self {
            radius,
            theta: v.x.atan2(v.z),
            phi: (v.y / radius).clamp(-1.0, 1.0).acos(),
        }
    }

    fn offset(&self) -> Vec3 {
        let s = self.phi.sin() * self.radius;
        Vec3::new(
            s * self.theta.sin(),
            self.phi.cos() * self.radius,
            s * self.theta.cos(),
        )
    }
}

#[derive(Debug, Default)]
pub struct Scene3DView {
    scene: Option<Scene3D>,
    /// The mapped camera the user's orbit started from; a new pose from the scene resets the orbit.
    base: Option<Camera>,
    eye: Vec3,
    target: Vec3,
    delta_theta: f32,
    delta_phi: f32,
    scale: f32,
    pan: Vec3,
    size: (f32, f32),
    damping: bool,
    pointers: BTreeMap<i32, (f32, f32)>,
    gesture: Option<Gesture>,
    down_at: Option<(f32, f32)>,
    moved: f32,
    pinch: Option<f32>,
    pinch_center: Option<(f32, f32)>,
    edit: Option<EditMode>,
    drag: Option<EditDrag>,
    preview: Option<DragPreview>,
    ground: Option<GroundExtent>,
    follow: Option<Follow>,
}

impl Scene3DView {
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            size: (1.0, 1.0),
            damping: true,
            ..Default::default()
        }
    }

    /// The host's reduced-motion preference turns damping off, as the host does for OrbitControls.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.damping = !reduced;
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.size = (width.max(1.0), height.max(1.0));
    }

    /// A new pose from the scene resets the orbit; a following camera keeps where it is and eases toward it.
    pub fn set_scene(&mut self, scene: Scene3D) {
        if self.base.as_ref() != Some(&scene.camera) {
            match (self.follow, self.base) {
                (Some(_), Some(_)) => {}
                (Some(follow), None) => {
                    self.eye = Vec3::from(follow.start_eye);
                    self.target = Vec3::from(scene.camera.target);
                }
                (None, _) => {
                    self.eye = Vec3::from(scene.camera.eye);
                    self.target = Vec3::from(scene.camera.target);
                }
            }
            self.base = Some(scene.camera);
            self.delta_theta = 0.0;
            self.delta_phi = 0.0;
            self.scale = 1.0;
            self.pan = Vec3::ZERO;
        }
        self.scene = Some(scene);
    }

    pub fn scene(&self) -> Option<&Scene3D> {
        self.scene.as_ref()
    }

    /// The camera as the user has moved it.
    pub fn camera(&self) -> Option<Camera> {
        self.base.map(|base| Camera {
            eye: self.eye.into(),
            target: self.target.into(),
            ..base
        })
    }

    /// Set before [`Self::set_scene`]: `None` leaves the camera to the orbit.
    pub fn set_follow(&mut self, follow: Option<Follow>) {
        self.follow = follow;
    }

    /// Edit mode suppresses clicks and the orbit; leaving it drops any drag in flight.
    pub fn set_edit(&mut self, edit: Option<EditMode>) {
        if edit.is_none() {
            self.drag = None;
            self.preview = None;
        }
        self.edit = edit;
    }

    /// `None` picks on the unbounded ground plane.
    pub fn set_ground_extent(&mut self, ground: Option<GroundExtent>) {
        self.ground = ground;
    }

    /// The live drag preview, for the host to re-map the scene with (`withPreviewPosition`).
    pub fn preview(&self) -> Option<&DragPreview> {
        self.preview.as_ref()
    }

    fn orbit_enabled(&self) -> bool {
        self.edit.is_none()
            && self.follow.is_none()
            && self.scene.as_ref().is_some_and(|s| s.interaction.orbit)
    }

    pub fn pointer_down(&mut self, id: i32, x: f32, y: f32, button: Button, touch: bool) {
        self.pointers.insert(id, (x, y));
        if self.pointers.len() == 1 {
            self.down_at = Some((x, y));
            self.moved = 0.0;
            if self.edit.is_some() {
                self.drag = self.tagged_under(x, y).map(|id| EditDrag {
                    id,
                    start: (x, y),
                    moved: false,
                });
            }
            self.gesture = Some(if touch {
                Gesture::Rotate
            } else {
                match button {
                    Button::Primary => Gesture::Rotate,
                    Button::Middle => Gesture::Dolly,
                    Button::Secondary => Gesture::Pan,
                }
            });
        } else if touch && self.pointers.len() == 2 {
            self.gesture = Some(Gesture::TouchDollyPan);
            self.pinch = Some(self.pinch_distance());
            self.pinch_center = Some(self.pinch_midpoint());
        }
    }

    fn pinch_distance(&self) -> f32 {
        let mut points = self.pointers.values();
        match (points.next(), points.next()) {
            (Some(a), Some(b)) => ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt(),
            _ => 0.0,
        }
    }

    fn pinch_midpoint(&self) -> (f32, f32) {
        let mut points = self.pointers.values();
        match (points.next(), points.next()) {
            (Some(a), Some(b)) => ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0),
            _ => (0.0, 0.0),
        }
    }

    /// True when the camera moved, or in edit mode when the drag preview changed.
    pub fn pointer_move(&mut self, id: i32, x: f32, y: f32) -> bool {
        let Some(previous) = self.pointers.get(&id).copied() else {
            return false;
        };
        self.pointers.insert(id, (x, y));
        let (dx, dy) = (x - previous.0, y - previous.1);
        self.moved += dx.abs() + dy.abs();
        if self.edit.is_some() {
            return self.edit_move(x, y);
        }
        if !self.orbit_enabled() {
            return false;
        }
        let height = self.size.1;
        match self.gesture {
            Some(Gesture::Rotate) => {
                self.delta_theta -= TAU * dx / height;
                self.delta_phi -= TAU * dy / height;
            }
            Some(Gesture::Dolly) => {
                if dy > 0.0 {
                    self.scale /= ZOOM_SCALE;
                } else if dy < 0.0 {
                    self.scale *= ZOOM_SCALE;
                }
            }
            Some(Gesture::Pan) => self.pan_by(dx, dy),
            Some(Gesture::TouchDollyPan) => {
                let distance = self.pinch_distance();
                if let Some(start) = self.pinch.filter(|d| *d > 0.0) {
                    self.scale *= start / distance.max(f32::EPSILON);
                }
                self.pinch = Some(distance);
                let center = self.pinch_midpoint();
                if let Some(previous) = self.pinch_center {
                    self.pan_by(center.0 - previous.0, center.1 - previous.1);
                }
                self.pinch_center = Some(center);
            }
            None => return false,
        }
        self.update()
    }

    /// The ground plane under the pointer while a drag is in flight, past the gate.
    fn edit_move(&mut self, x: f32, y: f32) -> bool {
        let Some(cell) = self.ground_cell(x, y) else {
            return false;
        };
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        if !drag.moved && (x - drag.start.0).abs() + (y - drag.start.1).abs() <= EDIT_DRAG_GATE {
            return false;
        }
        drag.moved = true;
        let next = DragPreview {
            id: drag.id.clone(),
            x: cell.0,
            y: cell.1,
        };
        if self.preview.as_ref() == Some(&next) {
            return false;
        }
        self.preview = Some(next);
        true
    }

    /// A release that barely moved is a click; it resolves against the scene. Edit-mode releases resolve only
    /// through [`Self::pointer_release`].
    pub fn pointer_up(
        &mut self,
        id: i32,
        x: f32,
        y: f32,
        unit_events: bool,
        tile_events: bool,
    ) -> Option<Pick> {
        match self.pointer_release(id, x, y, unit_events, tile_events)? {
            Release::Pick(pick) => Some(pick),
            Release::Select { .. } | Release::Move { .. } => None,
        }
    }

    /// The last pointer up: in edit mode a drop that moved is a move (once), a drag that never left the gate
    /// toggles the selection, and a press on no tagged drawable deselects; otherwise a click as [`Self::pick`].
    pub fn pointer_release(
        &mut self,
        id: i32,
        x: f32,
        y: f32,
        unit_events: bool,
        tile_events: bool,
    ) -> Option<Release> {
        self.pointers.remove(&id);
        if self.pointers.len() < 2 {
            self.pinch = None;
            self.pinch_center = None;
        }
        if !self.pointers.is_empty() {
            return None;
        }
        self.gesture = None;
        if let Some(selected) = self.edit.as_ref().map(|e| e.selected.clone()) {
            self.down_at = None;
            let preview = self.preview.take();
            return match self.drag.take() {
                None => Some(Release::Select { id: None }),
                Some(drag) if drag.moved => {
                    preview.filter(|p| p.id == drag.id).map(|p| Release::Move {
                        id: p.id,
                        x: p.x,
                        y: p.y,
                    })
                }
                Some(drag) => Some(Release::Select {
                    id: if selected.as_deref() == Some(drag.id.as_str()) {
                        None
                    } else {
                        Some(drag.id)
                    },
                }),
            };
        }
        let click = self.down_at.take().is_some() && self.moved <= CLICK_SLOP;
        if click {
            self.pick(x, y, unit_events, tile_events).map(Release::Pick)
        } else {
            None
        }
    }

    pub fn wheel(&mut self, delta_y: f32) -> bool {
        if !self.orbit_enabled() {
            return false;
        }
        if delta_y < 0.0 {
            self.scale *= ZOOM_SCALE;
        } else if delta_y > 0.0 {
            self.scale /= ZOOM_SCALE;
        }
        self.update()
    }

    fn pan_by(&mut self, dx: f32, dy: f32) {
        let Some(base) = self.base else { return };
        let offset = self.eye - self.target;
        let height = self.size.1;
        let (left, up) = match base.projection {
            Projection::Perspective { fov_y_degrees } => {
                let distance = offset.length() * (fov_y_degrees.to_radians() / 2.0).tan();
                (2.0 * dx * distance / height, 2.0 * dy * distance / height)
            }
            Projection::Orthographic { height: span } => (dx * span / height, dy * span / height),
        };
        let view = Mat4::look_at_rh(self.eye, self.target, Vec3::from(base.up)).inverse();
        let right = view.x_axis.truncate();
        let camera_up = view.y_axis.truncate();
        self.pan += -right * left + camera_up * up;
    }

    /// Advance damping; true while the camera is still moving. A following camera moves only in [`Self::advance`].
    pub fn frame(&mut self) -> bool {
        if !self.damping || self.follow.is_some() {
            return false;
        }
        self.update()
    }

    /// One frame of `dt_seconds`: a following camera's eye and look point each lerp toward the mapped pose by
    /// `min(1, dt * 5)` (all the way under reduced motion), else [`Self::frame`]. True when the camera moved.
    pub fn advance(&mut self, dt_seconds: f32) -> bool {
        let (Some(_), Some(goal)) = (self.follow, self.base) else {
            return self.frame();
        };
        let t = if self.damping {
            (dt_seconds.max(0.0) * FOLLOW_RATE).min(1.0)
        } else {
            1.0
        };
        let before = (self.eye, self.target);
        self.eye = self.eye.lerp(Vec3::from(goal.eye), t);
        self.target = self.target.lerp(Vec3::from(goal.target), t);
        (self.eye - before.0).length_squared() > 1e-10
            || (self.target - before.1).length_squared() > 1e-10
    }

    /// The host handle's `getCameraPosition`.
    pub fn camera_position(&self) -> Option<[f32; 3]> {
        self.base.map(|_| self.eye.into())
    }

    /// `setCameraPosition`: the eye moves there and `OrbitControls.update()` applies its limits.
    pub fn set_camera_position(&mut self, position: [f32; 3]) -> bool {
        if self.base.is_none() {
            return false;
        }
        self.eye = Vec3::from(position);
        self.update();
        true
    }

    /// `lookAt`: the orbit target moves there and `OrbitControls.update()` re-aims.
    pub fn look_at(&mut self, target: [f32; 3]) -> bool {
        if self.base.is_none() {
            return false;
        }
        self.target = Vec3::from(target);
        self.update();
        true
    }

    /// `resetCamera`: back to the mapped pose, the orbit's motion dropped.
    pub fn reset_camera(&mut self) -> bool {
        let Some(base) = self.base else {
            return false;
        };
        self.eye = Vec3::from(base.eye);
        self.target = Vec3::from(base.target);
        self.delta_theta = 0.0;
        self.delta_phi = 0.0;
        self.scale = 1.0;
        self.pan = Vec3::ZERO;
        true
    }

    fn update(&mut self) -> bool {
        let before = (self.eye, self.target);
        let mut spherical = Spherical::from_offset(self.eye - self.target);
        let k = if self.damping { DAMPING } else { 1.0 };
        spherical.theta += self.delta_theta * k;
        spherical.phi += self.delta_phi * k;
        spherical.phi = spherical.phi.clamp(1e-6, MAX_POLAR);
        spherical.radius = (spherical.radius * self.scale).clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.target += self.pan * k;
        self.eye = self.target + spherical.offset();
        if self.damping {
            self.delta_theta *= 1.0 - DAMPING;
            self.delta_phi *= 1.0 - DAMPING;
            self.pan *= 1.0 - DAMPING;
        } else {
            self.delta_theta = 0.0;
            self.delta_phi = 0.0;
            self.pan = Vec3::ZERO;
        }
        self.scale = 1.0;
        (self.eye - before.0).length_squared() > 1e-10
            || (self.target - before.1).length_squared() > 1e-10
    }

    /// The pointer's ray in world space: its near-plane origin and unit direction.
    fn ray(&self, x: f32, y: f32) -> Option<(Vec3, Vec3)> {
        let scene = self.scene.as_ref()?;
        let camera = self.camera()?;
        let framed = Scene3D {
            camera,
            ..scene.clone()
        };
        let inverse = view_projection(&framed, self.size.0 / self.size.1).inverse();
        let ndc = (2.0 * x / self.size.0 - 1.0, 1.0 - 2.0 * y / self.size.1);
        let near = inverse.project_point3(Vec3::new(ndc.0, ndc.1, 0.0));
        let far = inverse.project_point3(Vec3::new(ndc.0, ndc.1, 1.0));
        Some((near, (far - near).normalize_or_zero()))
    }

    /// The nearest mesh along the ray that carries an id; untagged meshes pass the ray on.
    fn nearest_tagged(&self, near: Vec3, direction: Vec3) -> Option<(f32, &str)> {
        let scene = self.scene.as_ref()?;
        let mut best: Option<(f32, &str)> = None;
        for mesh in &scene.meshes {
            let Some(id) = mesh.id.as_deref() else {
                continue;
            };
            let model = Mat4::from_cols_array(&mesh.transform);
            for tri in mesh.mesh.indices.chunks_exact(3) {
                let [a, b, c] = [tri[0], tri[1], tri[2]]
                    .map(|i| model.transform_point3(Vec3::from(mesh.mesh.positions[i as usize])));
                if let Some(t) = ray_triangle(near, direction, a, b, c) {
                    if best.is_none_or(|(d, _)| t < d) {
                        best = Some((t, id));
                    }
                }
            }
        }
        best
    }

    /// Where the ray meets the y = 0 ground plane, in scene units, within the ground extent when one is set.
    fn ground_hit(&self, near: Vec3, direction: Vec3) -> Option<(f32, f32, f32)> {
        let interaction = &self.scene.as_ref()?.interaction;
        if direction.y.abs() <= 1e-6 {
            return None;
        }
        let t = -near.y / direction.y;
        if t <= 0.0 {
            return None;
        }
        let point = near + direction * t;
        let cell = interaction.cell.max(f32::EPSILON);
        let (sx, sz) = (
            point.x / cell + interaction.min_x,
            point.z / cell + interaction.min_z,
        );
        if let Some(g) = self.ground {
            let inside = (g.min_x - GROUND_MARGIN..=g.max_x + GROUND_MARGIN).contains(&sx)
                && (g.min_z - GROUND_MARGIN..=g.max_z + GROUND_MARGIN).contains(&sz);
            if !inside {
                return None;
            }
        }
        Some((t, sx, sz))
    }

    fn tagged_under(&self, x: f32, y: f32) -> Option<String> {
        let (near, direction) = self.ray(x, y)?;
        self.nearest_tagged(near, direction)
            .map(|(_, id)| id.to_string())
    }

    /// The scene cell of the ground under the pointer (`groundPointToScene`), for the edit preview and tile info.
    pub fn ground_cell(&self, x: f32, y: f32) -> Option<(i64, i64)> {
        let (near, direction) = self.ray(x, y)?;
        let (_, sx, sz) = self.ground_hit(near, direction)?;
        Some((js_round(sx) as i64, js_round(sz) as i64))
    }

    /// The scene event under the pointer at (`x`, `y`): the nearest of a mesh that carries an id and the ground
    /// plane, as three's event propagation orders them (an untagged mesh passes the click on).
    pub fn pick(&self, x: f32, y: f32, unit_events: bool, tile_events: bool) -> Option<Pick> {
        let (near, direction) = self.ray(x, y)?;
        let interaction = &self.scene.as_ref()?.interaction;
        let cell = interaction.cell.max(f32::EPSILON);
        let tagged = if unit_events {
            self.nearest_tagged(near, direction)
        } else {
            None
        };
        let ground = if unit_events || tile_events {
            self.ground_hit(near, direction)
        } else {
            None
        };
        match (tagged, ground) {
            (Some((t, id)), ground) if ground.is_none_or(|(g, ..)| t <= g) => {
                let point = near + direction * t;
                Some(Pick::Unit {
                    id: id.to_string(),
                    x: point.x / cell + interaction.min_x,
                    z: point.z / cell + interaction.min_z,
                })
            }
            (_, Some((_, sx, sz))) => {
                let (cx, cz) = (js_round(sx) as i64, js_round(sz) as i64);
                // `buildHitIndex`: a later drawable on the same cell takes it
                match interaction
                    .cells
                    .iter()
                    .rev()
                    .find(|c| c.x == cx && c.y == cz)
                {
                    Some(hit) if unit_events => Some(Pick::Unit {
                        id: hit.id.clone(),
                        x: cx as f32,
                        z: cz as f32,
                    }),
                    _ if tile_events => Some(Pick::Tile { x: cx, z: cz }),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// JavaScript's `Math.round`: halves round toward positive infinity.
pub(crate) fn js_round(v: f32) -> f32 {
    (v + 0.5).floor()
}

/// Möller–Trumbore, both faces; the distance along the ray.
fn ray_triangle(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = direction.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = direction.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene3d::geometry;
    use crate::scene3d::model::*;

    fn scene_with_box(orbit: bool) -> Scene3D {
        Scene3D {
            camera: Camera {
                eye: [0.0, 5.0, 10.0],
                target: [0.0, 0.0, 0.0],
                up: [0.0, 1.0, 0.0],
                projection: Projection::Perspective {
                    fov_y_degrees: 45.0,
                },
                near: 0.1,
                far: 1000.0,
            },
            background: [0.0; 4],
            lights: Lights::default(),
            shadows: false,
            tone_mapping: ToneMapping::None,
            meshes: vec![MeshInstance {
                mesh: geometry::cuboid(2.0, 2.0, 2.0).unwrap(),
                transform: Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0)).to_cols_array(),
                material: Material::default(),
                cast_shadow: true,
                receive_shadow: true,
                id: Some("crate".into()),
            }],
            grid: None,
            interaction: Interaction {
                orbit,
                cell: 1.0,
                min_x: 0.0,
                min_z: 0.0,
                cells: vec![CellId {
                    x: 3,
                    y: 0,
                    id: "flag".into(),
                }],
            },
            animated: false,
        }
    }

    fn view(orbit: bool) -> Scene3DView {
        let mut v = Scene3DView::new();
        v.resize(400.0, 400.0);
        v.set_reduced_motion(true);
        v.set_scene(scene_with_box(orbit));
        v
    }

    fn screen(v: &Scene3DView, world: Vec3) -> (f32, f32) {
        let scene = Scene3D {
            camera: v.camera().unwrap(),
            ..v.scene().unwrap().clone()
        };
        let ndc = view_projection(&scene, 1.0).project_point3(world);
        ((ndc.x + 1.0) * 200.0, (1.0 - ndc.y) * 200.0)
    }

    #[test]
    fn a_click_on_a_tagged_mesh_is_a_unit_click_and_on_open_ground_a_tile_click() {
        let mut v = view(true);
        let (x, y) = screen(&v, Vec3::new(0.0, 1.0, 0.0));
        v.pointer_down(1, x, y, Button::Primary, false);
        assert!(
            matches!(v.pointer_up(1, x, y, true, true), Some(Pick::Unit { ref id, .. }) if id == "crate")
        );
        let (x, y) = screen(&v, Vec3::new(-3.0, 0.0, 2.0));
        v.pointer_down(1, x, y, Button::Primary, false);
        assert_eq!(
            v.pointer_up(1, x, y, true, true),
            Some(Pick::Tile { x: -3, z: 2 })
        );
    }

    #[test]
    fn the_ground_cell_of_an_id_is_a_unit_click_and_control_without_unit_events_a_tile() {
        let v = view(true);
        let (x, y) = screen(&v, Vec3::new(3.0, 0.0, 0.0));
        assert!(
            matches!(v.pick(x, y, true, true), Some(Pick::Unit { ref id, .. }) if id == "flag")
        );
        assert_eq!(v.pick(x, y, false, true), Some(Pick::Tile { x: 3, z: 0 }));
        assert_eq!(v.pick(x, y, false, false), None);
    }

    #[test]
    fn a_drag_orbits_and_is_not_a_click_and_control_with_orbit_off_the_camera_stays() {
        let mut v = view(true);
        let before = v.camera().unwrap().eye;
        v.pointer_down(1, 200.0, 200.0, Button::Primary, false);
        assert!(v.pointer_move(1, 260.0, 200.0));
        assert_eq!(
            v.pointer_up(1, 260.0, 200.0, true, true),
            None,
            "a drag is not a click"
        );
        assert_ne!(v.camera().unwrap().eye, before);
        let mut fixed = view(false);
        fixed.pointer_down(1, 200.0, 200.0, Button::Primary, false);
        assert!(!fixed.pointer_move(1, 260.0, 200.0));
        assert_eq!(fixed.camera().unwrap().eye, before);
    }

    #[test]
    fn the_orbit_keeps_three_s_limits() {
        let mut v = view(true);
        for _ in 0..40 {
            v.wheel(-1.0);
        }
        let c = v.camera().unwrap();
        assert!((Vec3::from(c.eye) - Vec3::from(c.target)).length() >= MIN_DISTANCE - 1e-4);
        v.pointer_down(1, 200.0, 200.0, Button::Primary, false);
        v.pointer_move(1, 200.0, 0.0);
        v.pointer_move(1, 200.0, -400.0);
        let c = v.camera().unwrap();
        let offset = Vec3::from(c.eye) - Vec3::from(c.target);
        assert!(
            offset.y / offset.length() >= MAX_POLAR.cos() - 1e-4,
            "never below the polar limit"
        );
    }

    #[test]
    fn a_new_camera_pose_from_the_scene_resets_the_orbit_and_control_the_same_pose_keeps_it() {
        let mut v = view(true);
        v.pointer_down(1, 200.0, 200.0, Button::Primary, false);
        v.pointer_move(1, 300.0, 200.0);
        v.pointer_up(1, 300.0, 200.0, false, false);
        let orbited = v.camera().unwrap().eye;
        v.set_scene(scene_with_box(true));
        assert_eq!(
            v.camera().unwrap().eye,
            orbited,
            "the same scene pose keeps the user's orbit"
        );
        let mut moved = scene_with_box(true);
        moved.camera.eye = [10.0, 5.0, 0.0];
        v.set_scene(moved);
        assert_eq!(v.camera().unwrap().eye, [10.0, 5.0, 0.0]);
    }

    fn editing(selected: Option<&str>) -> Scene3DView {
        let mut v = view(true);
        v.set_edit(Some(EditMode {
            selected: selected.map(str::to_string),
        }));
        v
    }

    #[test]
    fn an_edit_press_on_a_tagged_mesh_selects_it_and_control_a_second_press_deselects() {
        let mut v = editing(None);
        let (x, y) = screen(&v, Vec3::new(0.0, 1.0, 0.0));
        v.pointer_down(1, x, y, Button::Primary, false);
        assert!(!v.pointer_move(1, x + 3.0, y), "inside the 5px gate");
        assert_eq!(
            v.pointer_release(1, x + 3.0, y, true, true),
            Some(Release::Select {
                id: Some("crate".into())
            })
        );
        let mut v = editing(Some("crate"));
        v.pointer_down(1, x, y, Button::Primary, false);
        assert_eq!(
            v.pointer_release(1, x, y, true, true),
            Some(Release::Select { id: None })
        );
    }

    #[test]
    fn an_edit_press_on_open_ground_deselects_and_control_never_clicks() {
        let mut v = editing(Some("crate"));
        let (x, y) = screen(&v, Vec3::new(-3.0, 0.0, 2.0));
        v.pointer_down(1, x, y, Button::Primary, false);
        assert_eq!(
            v.pointer_release(1, x, y, true, true),
            Some(Release::Select { id: None })
        );
        v.pointer_down(1, x, y, Button::Primary, false);
        assert_eq!(
            v.pointer_up(1, x, y, true, true),
            None,
            "edit mode emits no click"
        );
    }

    #[test]
    fn a_drag_past_the_gate_previews_the_ground_cell_and_moves_once_on_drop() {
        let mut v = editing(None);
        let (x, y) = screen(&v, Vec3::new(0.0, 1.0, 0.0));
        let (gx, gy) = screen(&v, Vec3::new(-3.0, 0.0, 2.0));
        let eye = v.camera().unwrap().eye;
        v.pointer_down(1, x, y, Button::Primary, false);
        assert!(v.pointer_move(1, gx, gy));
        assert_eq!(
            v.preview(),
            Some(&DragPreview {
                id: "crate".into(),
                x: -3,
                y: 2
            })
        );
        assert!(
            !v.pointer_move(1, gx, gy),
            "the same cell is no new preview"
        );
        assert_eq!(v.camera().unwrap().eye, eye, "edit mode never orbits");
        assert!(!v.wheel(-1.0));
        assert_eq!(
            v.pointer_release(1, gx, gy, true, true),
            Some(Release::Move {
                id: "crate".into(),
                x: -3,
                y: 2
            })
        );
        assert_eq!(v.preview(), None);
        let (ox, oy) = screen(&v, Vec3::new(-4.0, 0.0, 4.0));
        v.pointer_down(1, gx, gy, Button::Primary, false);
        assert!(
            !v.pointer_move(1, ox, oy),
            "a drag from open ground moves nothing"
        );
        assert_eq!(v.preview(), None);
    }

    #[test]
    fn the_ground_extent_bounds_picking_and_control_without_one_the_plane_is_unbounded() {
        let mut v = view(true);
        let (x, y) = screen(&v, Vec3::new(-3.0, 0.0, 2.0));
        assert_eq!(v.pick(x, y, false, true), Some(Pick::Tile { x: -3, z: 2 }));
        v.set_ground_extent(Some(GroundExtent {
            min_x: 0.0,
            max_x: 3.0,
            min_z: 0.0,
            max_z: 3.0,
        }));
        assert_eq!(v.pick(x, y, false, true), None, "three cells off the board");
        let (x, y) = screen(&v, Vec3::new(-1.8, 0.0, 2.0));
        assert_eq!(v.pick(x, y, false, true), Some(Pick::Tile { x: -2, z: 2 }));
    }

    #[test]
    fn a_later_drawable_on_a_cell_takes_its_click_and_halves_round_up() {
        let mut scene = scene_with_box(true);
        scene.interaction.cells.push(CellId {
            x: 3,
            y: 0,
            id: "banner".into(),
        });
        let mut v = view(true);
        v.set_scene(scene);
        let (x, y) = screen(&v, Vec3::new(3.0, 0.0, 0.0));
        assert!(
            matches!(v.pick(x, y, true, true), Some(Pick::Unit { ref id, .. }) if id == "banner")
        );
        assert_eq!(
            (js_round(-2.5), js_round(2.5), js_round(-2.6)),
            (-2.0, 3.0, -3.0)
        );
    }

    fn following() -> Scene3DView {
        let mut v = Scene3DView::new();
        v.resize(400.0, 400.0);
        v.set_follow(Some(Follow {
            start_eye: [0.0, 10.0, 20.0],
        }));
        v.set_scene(scene_with_box(false));
        v
    }

    #[test]
    fn a_follow_camera_starts_at_the_framing_pose_and_eases_at_three_s_rate() {
        let mut v = following();
        assert_eq!(v.camera().unwrap().eye, [0.0, 10.0, 20.0]);
        assert!(v.advance(0.1));
        assert_eq!(v.camera().unwrap().eye, [0.0, 7.5, 15.0], "lerp by 0.1 * 5");
        assert!(v.advance(1.0));
        assert_eq!(v.camera().unwrap().eye, [0.0, 5.0, 10.0], "t caps at 1");
        assert!(!v.advance(0.016), "settled");
    }

    #[test]
    fn a_new_follow_pose_eases_instead_of_snapping_and_control_reduced_motion_snaps() {
        let mut v = following();
        v.advance(1.0);
        let mut moved = scene_with_box(false);
        moved.camera.eye = [10.0, 5.0, 10.0];
        moved.camera.target = [10.0, 0.0, 0.0];
        v.set_scene(moved.clone());
        assert_eq!(v.camera().unwrap().eye, [0.0, 5.0, 10.0]);
        v.advance(0.1);
        assert_eq!(v.camera().unwrap().eye, [5.0, 5.0, 10.0]);
        assert_eq!(v.camera().unwrap().target, [5.0, 0.0, 0.0]);
        assert!(!v.frame(), "orbit damping never moves a following camera");
        let mut still = following();
        still.set_reduced_motion(true);
        still.advance(0.001);
        assert_eq!(still.camera().unwrap().eye, [0.0, 5.0, 10.0]);
    }

    #[test]
    fn the_handle_moves_aims_and_resets_the_camera_within_the_orbit_limits() {
        let mut v = view(true);
        assert!(v.set_camera_position([0.0, 6.0, 8.0]));
        let eye = v.camera_position().unwrap();
        assert!((Vec3::from(eye) - Vec3::new(0.0, 6.0, 8.0)).length() < 1e-4);
        assert!(v.set_camera_position([0.0, 0.5, 0.5]));
        let c = v.camera().unwrap();
        assert!(
            (Vec3::from(c.eye) - Vec3::from(c.target)).length() >= MIN_DISTANCE - 1e-4,
            "update() keeps the minimum distance"
        );
        assert!(v.look_at([1.0, 0.0, 1.0]));
        assert_eq!(v.camera().unwrap().target, [1.0, 0.0, 1.0]);
        assert!(v.reset_camera());
        assert_eq!(v.camera().unwrap().eye, [0.0, 5.0, 10.0]);
        assert_eq!(v.camera().unwrap().target, [0.0, 0.0, 0.0]);
        let mut empty = Scene3DView::new();
        assert!(!empty.set_camera_position([1.0, 1.0, 1.0]));
        assert_eq!(empty.camera_position(), None);
    }
}
