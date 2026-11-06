//! The interactive side of a 3D canvas, shared by every host: a port of three-stdlib's `OrbitControls` as the host
//! configures it (left drag rotates, wheel or middle drag dollies, right drag pans; one finger rotates, two fingers
//! dolly and pan; damping 0.05; distance 2..100; polar angle at most pi/2 - 0.1) and picking that resolves a pointer
//! to the scene's own click events.

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

/// What a click resolved to, in scene coordinates (scene `y` is the ground axis three calls `z`).
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Unit { id: String, x: f32, z: f32 },
    Tile { x: i64, z: i64 },
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

    pub fn set_scene(&mut self, scene: Scene3D) {
        if self.base.as_ref() != Some(&scene.camera) {
            self.base = Some(scene.camera);
            self.eye = Vec3::from(scene.camera.eye);
            self.target = Vec3::from(scene.camera.target);
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

    fn orbit_enabled(&self) -> bool {
        self.scene.as_ref().is_some_and(|s| s.interaction.orbit)
    }

    pub fn pointer_down(&mut self, id: i32, x: f32, y: f32, button: Button, touch: bool) {
        self.pointers.insert(id, (x, y));
        if self.pointers.len() == 1 {
            self.down_at = Some((x, y));
            self.moved = 0.0;
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

    /// True when the camera moved.
    pub fn pointer_move(&mut self, id: i32, x: f32, y: f32) -> bool {
        let Some(previous) = self.pointers.get(&id).copied() else {
            return false;
        };
        self.pointers.insert(id, (x, y));
        let (dx, dy) = (x - previous.0, y - previous.1);
        self.moved += dx.abs() + dy.abs();
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

    /// A release that barely moved is a click; it resolves against the scene.
    pub fn pointer_up(
        &mut self,
        id: i32,
        x: f32,
        y: f32,
        unit_events: bool,
        tile_events: bool,
    ) -> Option<Pick> {
        self.pointers.remove(&id);
        if self.pointers.len() < 2 {
            self.pinch = None;
            self.pinch_center = None;
        }
        if !self.pointers.is_empty() {
            return None;
        }
        self.gesture = None;
        let click = self.down_at.take().is_some() && self.moved <= CLICK_SLOP;
        if click {
            self.pick(x, y, unit_events, tile_events)
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

    /// Advance damping; true while the camera is still moving.
    pub fn frame(&mut self) -> bool {
        if !self.damping {
            return false;
        }
        self.update()
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

    /// The scene event under the pointer at (`x`, `y`): the nearest of a mesh that carries an id and the ground
    /// plane, as three's event propagation orders them (an untagged mesh passes the click on).
    pub fn pick(&self, x: f32, y: f32, unit_events: bool, tile_events: bool) -> Option<Pick> {
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
        let direction = (far - near).normalize_or_zero();
        let mut best: Option<(f32, Option<&str>)> = None;
        if unit_events {
            for mesh in &scene.meshes {
                let Some(id) = mesh.id.as_deref() else {
                    continue;
                };
                let model = Mat4::from_cols_array(&mesh.transform);
                for tri in mesh.mesh.indices.chunks_exact(3) {
                    let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| {
                        model.transform_point3(Vec3::from(mesh.mesh.positions[i as usize]))
                    });
                    if let Some(t) = ray_triangle(near, direction, a, b, c) {
                        if best.is_none_or(|(d, _)| t < d) {
                            best = Some((t, Some(id)));
                        }
                    }
                }
            }
        }
        let interaction = &scene.interaction;
        let cell = interaction.cell.max(f32::EPSILON);
        if (unit_events || tile_events) && direction.y.abs() > 1e-6 {
            let t = -near.y / direction.y;
            if t > 0.0 && best.is_none_or(|(d, _)| t < d) {
                best = Some((t, None));
            }
        }
        let (t, id) = best?;
        let point = near + direction * t;
        match id {
            Some(id) => Some(Pick::Unit {
                id: id.to_string(),
                x: point.x / cell + interaction.min_x,
                z: point.z / cell + interaction.min_z,
            }),
            None => {
                let (cx, cz) = (
                    (point.x / cell + interaction.min_x).round() as i64,
                    (point.z / cell + interaction.min_z).round() as i64,
                );
                match interaction.cells.iter().find(|c| c.x == cx && c.y == cz) {
                    Some(hit) if unit_events => Some(Pick::Unit {
                        id: hit.id.clone(),
                        x: cx as f32,
                        z: cz as f32,
                    }),
                    _ if tile_events => Some(Pick::Tile { x: cx, z: cz }),
                    _ => None,
                }
            }
        }
    }
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
}
