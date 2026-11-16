//! The walk over a 3D canvas's drawables: placement, animation, groups and meshes.

use super::*;

/// `create3DProjector`: scene `x`/`y` scale by the cell onto world X/Z (offset by the bounds anchor at the top
/// level, unanchored inside a group); scene `z` is world Y as authored.
#[derive(Debug, Clone, Copy)]
pub(super) struct Placement {
    pub(super) cell: f64,
    pub(super) off_x: f64,
    pub(super) off_z: f64,
}

impl Placement {
    pub(super) fn world(self, p: [f64; 3]) -> DVec3 {
        DVec3::new(
            p[0] * self.cell + self.off_x,
            p[2],
            p[1] * self.cell + self.off_z,
        )
    }

    /// An animation offset in cells on scene axes, projected like a position: `x` to X, `y` to Z, `z` to Y, all
    /// three scaled by the cell (unlike a position's `z`).
    pub(super) fn offset(self, [x, y, z]: [f64; 3]) -> DVec3 {
        DVec3::new(x * self.cell, z * self.cell, y * self.cell)
    }

    pub(super) fn inner(self) -> Self {
        Self {
            cell: self.cell,
            off_x: 0.0,
            off_z: 0.0,
        }
    }
}

pub(super) struct Walk<'a> {
    pub(super) theme: &'a SceneTheme,
    pub(super) meshes: Vec<MeshInstance>,
    /// The innermost enclosing drawable id, as three's click walks up to it.
    pub(super) pick_id: Option<String>,
    /// The canvas clock; `None` leaves animations at rest.
    pub(super) elapsed_ms: Option<f64>,
    /// The page's wall clock, when the host gives it.
    pub(super) epoch_ms: Option<f64>,
    pub(super) animated: bool,
    /// `BoneStore`s: the canvas's at 0, one per `skeleton: true` group after it; name → bone world matrix.
    pub(super) scopes: Vec<HashMap<String, DMat4>>,
    pub(super) scope: usize,
    /// Skinned meshes, bound once every bone in the tree is known (three re-binds on each registration).
    pub(super) skins: Vec<PendingSkin>,
}

pub(super) struct PendingSkin {
    pub(super) mesh: usize,
    /// The outline shell's mesh and how far it stands off the posed surface.
    pub(super) hull: Option<(usize, f32)>,
    pub(super) scope: usize,
    pub(super) path: String,
    pub(super) bones: Vec<String>,
    pub(super) inverse_bind: Vec<DMat4>,
    pub(super) joints: Vec<[u16; 4]>,
    pub(super) weights: Vec<[f32; 4]>,
    pub(super) bind_inverse: DMat4,
}

pub(super) const SPRITE_KEYS: &[&str] = &[
    "type",
    "id",
    "position",
    "asset",
    "anchor",
    "width",
    "height",
    "frame",
    "animation",
    "loop",
    "clockMs",
    "flipX",
    "rotation",
    "opacity",
    "shadow",
];

pub(super) const SKINNED_MESH_KEYS: &[&str] = &[
    "type",
    "id",
    "position",
    "asset",
    "mesh",
    "meshUrl",
    "bones",
    "pose",
    "clip",
    "clipName",
    "frame",
    "opacity",
    "weightsOverlay",
    "ghost",
];

pub(super) const MESH_KEYS: &[&str] = &[
    "type",
    "id",
    "shape",
    "position",
    "width",
    "height",
    "depth",
    "radius",
    "radiusTop",
    "radiusBottom",
    "tube",
    "segments",
    "vertices",
    "faces",
    "uvs",
    "skin",
    "rotation",
    "pivot",
    "material",
    "outline",
    "castShadow",
    "receiveShadow",
    "animation",
    "opacity",
];

pub(super) const GROUP_KEYS: &[&str] = &[
    "type",
    "id",
    "position",
    "scale",
    "rotate",
    "rotation",
    "bone",
    "skeleton",
    "animation",
    "opacity",
    "clip",
    "items",
];

pub(super) const MATERIAL_KEYS: &[&str] = &[
    "kind",
    "color",
    "metalness",
    "roughness",
    "emissive",
    "emissiveIntensity",
    "opacity",
    "transmission",
    "ior",
    "flatShading",
    "side",
    "map",
    "wgsl",
    "uniforms",
    "anchor",
    "blend",
];

impl Walk<'_> {
    /// The node's declared `animation`, and its state on the canvas clock when there is one.
    pub(super) fn animation(
        &mut self,
        f: &Fields,
        path: &str,
    ) -> Result<Option<(Animation, Option<AnimatedState>)>, CanvasSceneError> {
        let Some(value) = present(f, "animation") else {
            return Ok(None);
        };
        let declared =
            Animation::parse(value, &join(path, "animation")).map_err(animation_error)?;
        self.animated = true;
        let state = self.elapsed_ms.map(|t| animation::sample(&declared, t));
        Ok(Some((declared, state)))
    }

    pub(super) fn drawable(
        &mut self,
        node: &Value,
        path: &str,
        parent: DMat4,
        place: Placement,
        opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        let fields = node
            .as_object()
            .ok_or_else(|| bad(path, "not a drawable node"))?;
        let ty = fields.get("type").and_then(Value::as_str).unwrap_or("");
        let outer = self.pick_id.clone();
        if let Some(id) = fields.get("id").and_then(Value::as_str) {
            self.pick_id = Some(id.to_string());
        }
        let drawn = match ty {
            "draw-mesh" => self.mesh(fields, path, parent, place, opacity),
            "draw-group" => self.group(fields, path, parent, place, opacity),
            "draw-sprite" => self.sprite(fields, path, parent, place, opacity),
            "draw-skinned-mesh" => self.skinned_mesh(fields, path, parent, place, opacity),
            "draw-shape" => self.shape(fields, path, parent, place, opacity),
            "draw-text" => self.text(fields, path, parent, place, opacity),
            "draw-sprite-layer" => self.layer(fields, path, parent, place, opacity, "draw-sprite"),
            "draw-shape-layer" => self.layer(fields, path, parent, place, opacity, "draw-shape"),
            "draw-text-layer" => self.layer(fields, path, parent, place, opacity, "draw-text"),
            "draw-fx-layer" => self.fx_layer(fields, path, parent, place, opacity),
            other => Err(bad(
                &join(path, "type"),
                format!("`{other}` is not a drawable"),
            )),
        };
        self.pick_id = outer;
        drawn
    }

    pub(super) fn group(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, GROUP_KEYS, path)?;
        if present(f, "clip").is_some() {
            return Err(undrawn(&join(path, "clip"), STEP_PARITY));
        }
        let skeleton = opt_bool(f, "skeleton", path)? == Some(true);
        let bone = match opt_str(f, "bone", path)? {
            Some("") => {
                return Err(bad(
                    &join(path, "bone"),
                    "is empty (the three host mounts no bone)",
                ))
            }
            other => other,
        };
        let world = place.world(required_pos(f, path)?);
        let scale = opt_number(f, "scale", path)?.unwrap_or(1.0);
        let rotate = opt_number(f, "rotate", path)?;
        let rotation = present(f, "rotation")
            .map(|v| triple(v, &join(path, "rotation")))
            .transpose()?;
        if rotate.is_some() && rotation.is_some() {
            return Err(undrawn(
                &format!("{path}.rotate alongside rotation"),
                STEP_PARITY,
            ));
        }
        let rotation = rotation.unwrap_or([0.0, -rotate.unwrap_or(0.0), 0.0]);
        let animated = self.animation(f, path)?;
        if let Some((declared, _)) = &animated {
            // `Group3D` animates the transform alone; a material or opacity track belongs on a leaf mesh.
            for track in MATERIAL_TRACKS {
                if let Some((i, _)) = declared.values(track).first() {
                    return Err(bad(
                        &format!("{path}.animation.keyframes[{i}].{track}"),
                        "a group animates only its transform (the three host ignores it)",
                    ));
                }
            }
        }
        let state = animated.and_then(|(_, state)| state);
        let (offset, delta, scale) = match &state {
            Some(s) => (place.offset(s.offset), s.rotate, scale * s.scale),
            None => (DVec3::ZERO, [0.0; 3], scale),
        };
        let matrix = parent
            * DMat4::from_translation(world + offset)
            * euler_xyz([
                rotation[0] + delta[0],
                rotation[1] + delta[1],
                rotation[2] + delta[2],
            ])
            * DMat4::from_scale(DVec3::splat(scale));
        let opacity = opt_number(f, "opacity", path)?.unwrap_or(1.0) * opacity;
        let items = present(f, "items").ok_or_else(|| bad(&join(path, "items"), "is required"))?;
        if !items.is_array() {
            return Err(bad(&join(path, "items"), "must be an array"));
        }
        let outer_scope = self.scope;
        if skeleton {
            self.scopes.push(HashMap::new());
            self.scope = self.scopes.len() - 1;
        }
        if let Some(name) = bone {
            if self.scopes[self.scope]
                .insert(name.to_string(), matrix)
                .is_some()
            {
                return Err(bad(
                    &join(path, "bone"),
                    format!("`{name}` is already a bone in this skeleton scope (the three host keeps whichever registers last)"),
                ));
            }
        }
        let items_path = join(path, "items");
        for (i, item) in drawable_nodes(items, &items_path)?.iter().enumerate() {
            self.drawable(
                item,
                &format!("{items_path}[{i}]"),
                matrix,
                place.inner(),
                opacity,
            )?;
        }
        self.scope = outer_scope;
        Ok(())
    }

    pub(super) fn mesh(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, MESH_KEYS, path)?;
        let outline = outline(f, path, self.theme)?;
        let world = place.world(required_pos(f, path)?);
        let shape = match present(f, "shape") {
            None => "box",
            Some(v) => v
                .as_str()
                .ok_or_else(|| bad(&join(path, "shape"), "must be a string"))?,
        };
        let dims: &[&str] = match shape {
            "box" => &["width", "height", "depth"],
            "sphere" | "circle" => &["radius", "segments"],
            "capsule" | "cone" => &["radius", "height", "segments"],
            "cylinder" => &["radius", "radiusTop", "radiusBottom", "height", "segments"],
            "torus" => &["radius", "tube", "segments"],
            "plane" => &["width", "depth"],
            "polyhedron" => &[],
            _ => {
                return Err(bad(
                    &join(path, "shape"),
                    format!("`{shape}` is not a mesh shape"),
                ))
            }
        };
        for key in [
            "width",
            "height",
            "depth",
            "radius",
            "radiusTop",
            "radiusBottom",
            "tube",
            "segments",
        ] {
            if present(f, key).is_some() && !dims.contains(&key) {
                return Err(bad(
                    &join(path, key),
                    format!("shape `{shape}` has no {key} (the three host ignores it)"),
                ));
            }
        }
        if shape != "polyhedron" && present(f, "skin").is_some() {
            return Err(bad(
                &join(path, "skin"),
                format!(
                    "only a polyhedron is skinned; shape is `{shape}` (the three host ignores it)"
                ),
            ));
        }
        if shape == "polyhedron" && present(f, "skin").is_some() {
            return self.skinned_polyhedron(f, path, parent, place, group_opacity);
        }
        if shape != "polyhedron" {
            for key in ["vertices", "faces", "uvs"] {
                if present(f, key).is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty)) {
                    return Err(bad(
                        &join(path, key),
                        format!("only a polyhedron has {key}; shape is `{shape}`"),
                    ));
                }
            }
        }

        let cell = place.cell;
        let dim = |key: &str, default: f64| -> Result<f64, CanvasSceneError> {
            Ok(opt_number(f, key, path)?.unwrap_or(default))
        };
        let radius = dim("radius", 0.4)?;
        let r = radius * cell;
        let w = dim("width", 1.0)?;
        let h = dim("height", 1.0)? * cell;
        let d = dim("depth", w)? * cell;
        let w = w * cell;
        let seg = clamp_segments(opt_number(f, "segments", path)?);
        let geo = |result: Result<MeshData, geometry::GeometryError>| {
            result.map_err(|e| bad(path, format!("{shape} geometry: {}", e.0)))
        };
        // `size` is `meshGeometry`'s representative extent, which scales the outline shell; a polyhedron has none
        let (mesh, lift, size) = match shape {
            "box" => (
                geo(geometry::cuboid(w as f32, h as f32, d as f32))?,
                h / 2.0,
                Some(w.max(h).max(d)),
            ),
            "sphere" => (geo(geometry::sphere(r as f32, seg, seg))?, r, Some(r * 2.0)),
            "capsule" => {
                let caps = js_round(seg as f64 / 2.0).max(2.0) as usize;
                (
                    geo(geometry::capsule(r as f32, h as f32, caps, seg))?,
                    h / 2.0 + r,
                    Some(h + r * 2.0),
                )
            }
            "cylinder" => {
                let top = dim("radiusTop", radius)? * cell;
                let bottom = dim("radiusBottom", radius)? * cell;
                (
                    geo(geometry::cylinder(top as f32, bottom as f32, h as f32, seg))?,
                    h / 2.0,
                    Some(h.max(r * 2.0)),
                )
            }
            "cone" => (
                geo(geometry::cylinder(0.0, r as f32, h as f32, seg))?,
                h / 2.0,
                Some(h.max(r * 2.0)),
            ),
            "torus" => {
                let tube = dim("tube", radius / 3.0)? * cell;
                let radial = js_round(seg as f64 / 2.0).max(3.0) as usize;
                (
                    geo(geometry::torus(r as f32, tube as f32, radial, seg))?,
                    r + tube,
                    Some((r + tube) * 2.0),
                )
            }
            "plane" => (
                geo(geometry::plane(w as f32, d as f32))?,
                0.0,
                Some(w.max(d)),
            ),
            "circle" => (geo(geometry::circle(r as f32, seg))?, 0.0, Some(r * 2.0)),
            _ => {
                let (mesh, lift) = polyhedron(f, path, cell)?;
                (mesh, lift, None)
            }
        };
        let lift = match opt_str(f, "pivot", path)? {
            None | Some("bottom") => lift,
            Some("center") => 0.0,
            Some(_) => return Err(bad(&join(path, "pivot"), "must be bottom or center")),
        };
        let [rx, ry, rz] = present(f, "rotation")
            .map(|v| triple(v, &join(path, "rotation")))
            .transpose()?
            .unwrap_or([0.0; 3]);
        let animated = self.animation(f, path)?;
        let state = animated.as_ref().and_then(|(_, state)| state.as_ref());
        let [ax, ay, az] = state.map_or([0.0; 3], |s| s.rotate);
        // three's GROUND_ROTATION lays plane and circle flat; it adds to the authored Euler angles.
        let ground = if matches!(shape, "plane" | "circle") {
            -FRAC_PI_2
        } else {
            0.0
        };
        let oriented = euler_xyz([ground + rx + ax, ry + ay, rz + az]);
        // `Mesh3D` moves and scales its outer group, so the scale also carries the pivot lift.
        let (offset, scale) =
            state.map_or((DVec3::ZERO, 1.0), |s| (place.offset(s.offset), s.scale));
        let group = parent
            * DMat4::from_translation(world + offset)
            * DMat4::from_scale(DVec3::splat(scale))
            * DMat4::from_translation(DVec3::new(0.0, lift, 0.0));
        let transform = group * oriented;

        let material_fields = opt_object(f, "material", path)?;
        let material_opacity = match material_fields {
            Some(m) => opt_number(m, "opacity", &join(path, "material"))?.unwrap_or(1.0),
            None => 1.0,
        };
        let node_opacity = opt_number(f, "opacity", path)?;
        let opacity = state
            .and_then(|s| s.opacity)
            .or(node_opacity)
            .unwrap_or(1.0)
            * material_opacity
            * group_opacity;
        let still_opacity = node_opacity.unwrap_or(1.0) * material_opacity * group_opacity;
        let mut material = material(
            material_fields,
            &join(path, "material"),
            opacity,
            self.theme,
            self.epoch_ms,
        )?;
        // a shader material may read the canvas clock, so the host keeps drawing
        self.animated |= material.shader.is_some();
        if let Some((declared, _)) = &animated {
            let anim_path = join(path, "animation");
            animated_colors(declared, &anim_path)?;
            let base_emissive = material_fields.is_some_and(|m| present(m, "emissive").is_some());
            for track in ["emissive", "emissiveIntensity"] {
                let Some((i, _)) = declared.values(track).first().copied() else {
                    continue;
                };
                let field = format!("{anim_path}.keyframes[{i}].{track}");
                if material.kind == MaterialKind::Basic {
                    return Err(bad(
                        &field,
                        "material kind `basic` has no emissive (the three host ignores it)",
                    ));
                }
                if track == "emissiveIntensity" && !base_emissive && !declared.declares("emissive")
                {
                    return Err(bad(
                        &field,
                        "has no effect without material.emissive or an emissive track (the three host ignores it)",
                    ));
                }
            }
        }
        if let Some(s) = state {
            if let Some(color) = &s.color {
                material.color = rgb(
                    &Value::String(color.clone()),
                    &join(path, "animation.color"),
                )?;
            }
            if let Some(emissive) = &s.emissive {
                material.emissive = rgb(
                    &Value::String(emissive.clone()),
                    &join(path, "animation.emissive"),
                )?;
            }
            if let Some(intensity) = s.emissive_intensity {
                material.emissive_intensity = intensity as f32;
            }
        }
        // `Mesh3D`'s hull is a sibling of the mesh in the animated group: it keeps the authored rotation and opacity
        let hull = match (&outline, size) {
            (Some(o), Some(size)) if size > 0.0 => Some((
                mesh.clone(),
                group
                    * euler_xyz([ground + rx, ry, rz])
                    * DMat4::from_scale(DVec3::splat(1.0 + o.width * cell / size)),
            )),
            (Some(_), Some(_)) => Some((mesh.clone(), group * euler_xyz([ground + rx, ry, rz]))),
            (Some(o), None) => Some((
                tessellate::inflate(&mesh, (o.width * cell) as f32)
                    .map_err(|e| bad(&join(path, "outline"), e))?,
                group * euler_xyz([ground + rx, ry, rz]),
            )),
            (None, _) => None,
        };
        self.meshes.push(MeshInstance {
            mesh,
            transform: transform.as_mat4().to_cols_array(),
            material,
            cast_shadow: opt_bool(f, "castShadow", path)?.unwrap_or(true),
            receive_shadow: opt_bool(f, "receiveShadow", path)?.unwrap_or(true),
            id: self.pick_id.clone(),
            glow: None,
        });
        if let (Some(o), Some((mesh, at))) = (outline, hull) {
            self.meshes
                .push(o.instance(mesh, at, still_opacity, self.pick_id.clone()));
        }
        Ok(())
    }
}
