//! Sprites and models in 3D: glTF models and their fallbacks, skinned meshes and polyhedra.

use super::*;

impl Walk<'_> {
    /// `SkinnedPolyhedron3D`: a `SkinnedMesh` under a group at the mesh's position, bound with an identity bind
    /// matrix in attached mode, so the named bones alone place every vertex.
    pub(super) fn skinned_polyhedron(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        for key in ["rotation", "pivot", "animation"] {
            if present(f, key).is_some() {
                return Err(bad(
                    &join(path, key),
                    "a skinned mesh is placed by its bones alone (the three host ignores it)",
                ));
            }
        }
        let world = place.world(required_pos(f, path)?);
        let (mesh, _) = polyhedron(f, path, place.cell)?;
        let skin_path = join(path, "skin");
        let skin =
            opt_object(f, "skin", path)?.ok_or_else(|| bad(&skin_path, "must be an object"))?;
        check_keys(
            skin,
            &["bones", "inverseBindMatrices", "indices", "weights"],
            &skin_path,
        )?;
        let bones = skin_list(skin, "bones", &skin_path)?
            .iter()
            .enumerate()
            .map(|(i, b)| {
                b.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| bad(&format!("{skin_path}.bones[{i}]"), "must be a bone name"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ibm_path = join(&skin_path, "inverseBindMatrices");
        let mut inverse_bind = Vec::new();
        for (i, m) in skin_list(skin, "inverseBindMatrices", &skin_path)?
            .iter()
            .enumerate()
        {
            let field = format!("{ibm_path}[{i}]");
            let values = m
                .as_array()
                .filter(|v| v.len() == 16)
                .ok_or_else(|| bad(&field, "must be 16 numbers, column-major"))?;
            let mut cols = [0.0; 16];
            for (k, v) in values.iter().enumerate() {
                cols[k] = number(v, &field)?;
            }
            // translations are authored in cells, like the vertices
            for c in &mut cols[12..15] {
                *c *= place.cell;
            }
            inverse_bind.push(DMat4::from_cols_array(&cols));
        }
        if inverse_bind.len() != bones.len() {
            return Err(bad(
                &ibm_path,
                format!(
                    "{} matrices for {} bones (the three host never binds the skin)",
                    inverse_bind.len(),
                    bones.len()
                ),
            ));
        }
        let n = mesh.positions.len();
        let mut joints = Vec::with_capacity(n);
        for (i, row) in skin_rows(skin, "indices", &skin_path, n)?
            .into_iter()
            .enumerate()
        {
            let mut j = [0u16; 4];
            for k in 0..4 {
                if row[k].fract() != 0.0 || !(0.0..=65535.0).contains(&row[k]) {
                    return Err(bad(
                        &format!("{skin_path}.indices[{i}]"),
                        "bone indices must be integers in 0..=65535",
                    ));
                }
                j[k] = row[k] as u16;
            }
            joints.push(j);
        }
        let weights: Vec<[f32; 4]> = skin_rows(skin, "weights", &skin_path, n)?
            .into_iter()
            .map(|row| row.map(|w| w as f32))
            .collect();

        let material_fields = opt_object(f, "material", path)?;
        let material_opacity = match material_fields {
            Some(m) => opt_number(m, "opacity", &join(path, "material"))?.unwrap_or(1.0),
            None => 1.0,
        };
        let opacity =
            opt_number(f, "opacity", path)?.unwrap_or(1.0) * material_opacity * group_opacity;
        let material = material(
            material_fields,
            &join(path, "material"),
            opacity,
            self.theme,
            self.epoch_ms,
        )?;
        self.animated |= material.shader.is_some();
        let transform = parent * DMat4::from_translation(world);
        let outline = outline(f, path, self.theme)?;
        self.meshes.push(MeshInstance {
            mesh: mesh.clone(),
            transform: transform.as_mat4().to_cols_array(),
            material,
            cast_shadow: opt_bool(f, "castShadow", path)?.unwrap_or(true),
            receive_shadow: opt_bool(f, "receiveShadow", path)?.unwrap_or(true),
            id: self.pick_id.clone(),
            glow: None,
        });
        let skinned = self.meshes.len() - 1;
        // three draws no hull on a skinned mesh; ours is the posed surface pushed out along its normals
        let hull = outline.map(|o| {
            let width = (o.width * place.cell) as f32;
            self.meshes
                .push(o.instance(mesh, transform, opacity, self.pick_id.clone()));
            (self.meshes.len() - 1, width)
        });
        self.skins.push(PendingSkin {
            hull,
            mesh: skinned,
            scope: self.scope,
            path: path.to_string(),
            bones,
            inverse_bind,
            joints,
            weights,
            bind_inverse: transform.inverse(),
        });
        Ok(())
    }

    /// Pose each skinned mesh by its scope's bones: three's bone matrices are bone world × inverse bind, and the
    /// attached bind mode's `bindMatrixInverse` is the inverse of the mesh's world matrix.
    pub(super) fn bind_skins(&mut self) -> Result<(), CanvasSceneError> {
        for pending in std::mem::take(&mut self.skins) {
            let scope = &self.scopes[pending.scope];
            let mut bones = Vec::with_capacity(pending.bones.len());
            for (i, name) in pending.bones.iter().enumerate() {
                let world = scope.get(name).ok_or_else(|| {
                    bad(
                        &format!("{}.skin.bones[{i}]", pending.path),
                        format!("no draw-group in this skeleton scope declares bone `{name}` (the three host never binds the skin)"),
                    )
                })?;
                bones.push((*world * pending.inverse_bind[i]).as_mat4());
            }
            let instance = &mut self.meshes[pending.mesh];
            instance.mesh = skin::skin_mesh(
                &instance.mesh,
                &pending.joints,
                &pending.weights,
                &bones,
                Mat4::IDENTITY,
                pending.bind_inverse.as_mat4(),
            )
            .map_err(|e| bad(&join(&pending.path, "skin"), e))?;
            if let Some((hull, width)) = pending.hull {
                let shell = tessellate::inflate(&self.meshes[pending.mesh].mesh, width)
                    .map_err(|e| bad(&join(&pending.path, "outline"), e))?;
                self.meshes[hull].mesh = shell;
            }
        }
        Ok(())
    }

    /// `Sprite3D`: the fallback box when the asset has no url (`spriteAssetState`), a model when it is 3D, else a
    /// flat image.
    pub(super) fn sprite(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, SPRITE_KEYS, path)?;
        let asset_path = join(path, "asset");
        let asset = opt_object(f, "asset", path)?;
        // an asset is a record of the program's art data: its declared fields are read, a row's own extras are its own

        let url = match asset {
            Some(a) => opt_str(a, "url", &asset_path)?.filter(|u| !u.is_empty()),
            None => None,
        };
        let (Some(asset), Some(url)) = (asset, url) else {
            return self.sprite_fallback(f, path, parent, place);
        };
        match opt_str(asset, "dimension", &asset_path)? {
            Some("3d") => self.model_sprite(f, path, parent, place, group_opacity, asset, url),
            None | Some("2d") => {
                self.flat_sprite(f, path, parent, place, group_opacity, asset, url)
            }
            Some(_) => Err(bad(&join(&asset_path, "dimension"), "must be 2d or 3d")),
        }
    }

    /// `Sprite3D` with `asset.dimension: "3d"`: `ModelLoader` at the sprite's position, turned by `rotation` (which
    /// it reads in degrees), scaled to the sprite's size over the model's rest-pose extent, playing `animation`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn model_sprite(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
        asset: &Fields,
        url: &str,
    ) -> Result<(), CanvasSceneError> {
        let asset_path = join(path, "asset");
        for key in ["atlas", "sprite"] {
            if present(asset, key).is_some() {
                return Err(bad(
                    &join(&asset_path, key),
                    "a 3D model has no atlas (the three host ignores it)",
                ));
            }
        }
        for key in [
            "anchor", "frame", "loop", "clockMs", "flipX", "opacity", "shadow",
        ] {
            if present(f, key).is_some() {
                return Err(bad(
                    &join(path, key),
                    "a 3D model does not draw it (the three host ignores it)",
                ));
            }
        }
        let width = opt_number(f, "width", path)?;
        let height = opt_number(f, "height", path)?;
        if width.is_none() && height.is_some() {
            return Err(bad(
                &join(path, "height"),
                "a 3D model reads height only alongside width (the three host ignores it)",
            ));
        }
        if group_opacity != 1.0 {
            return Err(bad(
                path,
                "an enclosing draw-group's opacity does not reach a 3D model's own materials (the three host ignores it)",
            ));
        }
        let rotation = opt_number(f, "rotation", path)?.unwrap_or(0.0);
        let animation = opt_str(f, "animation", path)?;
        let at = parent * DMat4::from_translation(place.world(required_pos(f, path)?));

        let model = match model_state(url)
            .map_err(|e| bad(&join(&asset_path, "url"), format!("{url}: {e}")))?
        {
            ModelState::Loading => return Ok(()),
            ModelState::Failed(why) => {
                tracing::warn!(target: "almadar::scene3d", url, %why, "model failed; drawing the error fallback");
                return self.model_fallback(at);
            }
            ModelState::Ready(model) => model,
        };
        // GLTFLoader resolves only once every texture settled; a texture that failed leaves its material unmapped.
        let mut dropped = Vec::new();
        for map in model.map_urls() {
            if asset::image_info(&map).is_some() {
                continue;
            }
            if asset::image_failed(&map) {
                dropped.push(map);
                continue;
            }
            return Ok(());
        }
        let clip = animation.and_then(|name| model.clip_index(name));
        if clip.is_some() {
            self.animated = true;
        }
        let posed = match self.elapsed_ms {
            Some(ms) => model.pose(clip, (ms / 1000.0) as f32),
            None => model.pose(None, 0.0),
        }
        .map_err(|e| bad(&join(&asset_path, "url"), format!("{url}: {e}")))?;
        let norm = match model.rest_bounds() {
            Some((lo, hi)) => {
                let max = f64::from((hi - lo).max_element());
                if !max.is_finite() || max < 0.05 {
                    1.0
                } else {
                    1.0 / max
                }
            }
            None => 1.0,
        };
        let size = match (width, height) {
            (None, _) => DVec3::ONE,
            (Some(w), None) => DVec3::splat(w * place.cell),
            (Some(w), Some(h)) => DVec3::new(w * place.cell, h * place.cell, w * place.cell),
        };
        let world =
            at * DMat4::from_rotation_y(rotation.to_radians()) * DMat4::from_scale(size * norm);
        for mut instance in posed {
            let local = Mat4::from_cols_array(&instance.transform).as_dmat4();
            instance.transform = (world * local).as_mat4().to_cols_array();
            if instance
                .material
                .map
                .as_ref()
                .is_some_and(|m| dropped.contains(m))
            {
                instance.material.map = None;
            }
            instance.id = self.pick_id.clone();
            self.meshes.push(instance);
        }
        Ok(())
    }

    /// `ModelLoader`'s error fallback (`fallbackGeometry: 'box'`): a 0.8 red standard box lifted 0.5, unrotated.
    pub(super) fn model_fallback(&mut self, at: DMat4) -> Result<(), CanvasSceneError> {
        let mesh = geometry::cuboid(0.8, 0.8, 0.8).map_err(|e| bad("model fallback", e.0))?;
        let color = rgb(&Value::String("#ff4444".into()), "model fallback")?;
        self.meshes.push(MeshInstance {
            mesh,
            transform: (at * DMat4::from_translation(DVec3::new(0.0, 0.5, 0.0)))
                .as_mat4()
                .to_cols_array(),
            material: Material {
                color,
                ..Material::default()
            },
            cast_shadow: false,
            receive_shadow: false,
            id: self.pick_id.clone(),
            glow: None,
        });
        Ok(())
    }

    /// The three host skips `draw-skinned-mesh` in 3D; this draws its 2D semantics on the scene plane instead: the
    /// rig the 2D painter poses (same mesh, bones, pose, clip and frame resolution), its scene `x`/`y` laid on world
    /// X/Z at the position's height, unlit and double-sided as a canvas paints it.
    pub(super) fn skinned_mesh(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, SKINNED_MESH_KEYS, path)?;
        if present(f, "ghost").is_some() {
            return Err(undrawn(
                &format!("{path}.ghost (an onion skin coplanar with the live pass has no depth order in 3D)"),
                STEP_PARITY,
            ));
        }
        let height = required_pos(f, path)?[2];
        let projector = Projector::new(1.0, 0.0, Layout2D::Flat);
        let unit = f64::from(projector.tile_width);
        let Some(commands) =
            skinned_mesh_2d::paint(&Value::Object(f.clone()), &projector, &|x, y| (x, y))
                .map_err(|e| bad(path, e))?
        else {
            return Ok(());
        };
        let mut batches: Vec<(Material, MeshData)> = Vec::new();
        for command in commands {
            let (points, uv, material) = match command {
                DrawCommand::TexturedTriangle {
                    texture,
                    points,
                    uv,
                    uv_in_pixels,
                    opacity,
                } => {
                    let (w, h) = asset::image_info(&texture)
                        .ok_or_else(|| bad(&join(path, "asset.url"), "the texture has no size"))?;
                    let pixel = uv.map(|c| {
                        if uv_in_pixels {
                            c
                        } else {
                            [c[0] * w as f32, c[1] * h as f32]
                        }
                    });
                    let area = (pixel[1][0] - pixel[0][0]) * (pixel[2][1] - pixel[0][1])
                        - (pixel[1][1] - pixel[0][1]) * (pixel[2][0] - pixel[0][0]);
                    // the 2D renderer skips a triangle that cuts no texture area
                    if area.abs() < f32::EPSILON {
                        continue;
                    }
                    let uv = pixel.map(|c| [c[0] / w.max(1) as f32, c[1] / h.max(1) as f32]);
                    let material = Material {
                        kind: MaterialKind::Basic,
                        opacity: (f64::from(opacity.clamp(0.0, 1.0)) * group_opacity) as f32,
                        side: Side::Double,
                        map: Some(texture),
                        ..Material::default()
                    };
                    (points.to_vec(), uv, material)
                }
                DrawCommand::Poly { points, color } => {
                    let material = Material {
                        kind: MaterialKind::Basic,
                        color: srgb_to_linear_rgb([color[0], color[1], color[2]]),
                        opacity: (f64::from(color[3]) * group_opacity) as f32,
                        side: Side::Double,
                        ..Material::default()
                    };
                    (points, [[0.0; 2]; 3], material)
                }
                _ => {
                    return Err(bad(
                        path,
                        "the 2D rig painted something other than a triangle",
                    ))
                }
            };
            if points.len() != 3 {
                return Err(bad(
                    path,
                    "the 2D rig painted something other than a triangle",
                ));
            }
            if batches.last().is_none_or(|(m, _)| *m != material) {
                batches.push((material, MeshData::default()));
            }
            let (_, mesh) = batches.last_mut().expect("a batch was just pushed");
            for (i, p) in points.iter().enumerate() {
                let at = place.world([f64::from(p.x) / unit, f64::from(p.y) / unit, height]);
                mesh.indices.push(mesh.positions.len() as u32);
                mesh.positions.push(f32x3(at.to_array()));
                mesh.normals.push([0.0, 1.0, 0.0]);
                mesh.uvs.push(uv[i]);
            }
        }
        for (material, mesh) in batches {
            self.meshes.push(MeshInstance {
                mesh,
                transform: parent.as_mat4().to_cols_array(),
                material,
                cast_shadow: false,
                receive_shadow: false,
                id: self.pick_id.clone(),
                glow: None,
            });
        }
        Ok(())
    }
}

pub(super) fn skin_list<'a>(
    skin: &'a Fields,
    key: &str,
    path: &str,
) -> Result<&'a Vec<Value>, CanvasSceneError> {
    present(skin, key)
        .and_then(Value::as_array)
        .ok_or_else(|| bad(&join(path, key), "is required as an array"))
}

/// Per-vertex rows of up to four numbers; three reads `[k] ?? 0`, so a short row pads with zeros.
pub(super) fn skin_rows(
    skin: &Fields,
    key: &str,
    path: &str,
    vertices: usize,
) -> Result<Vec<[f64; 4]>, CanvasSceneError> {
    let field = join(path, key);
    let items = skin_list(skin, key, path)?;
    if items.len() != vertices {
        return Err(bad(
            &field,
            format!(
                "{} entries for {vertices} vertices (the three host drops the skin)",
                items.len()
            ),
        ));
    }
    items
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let row_field = format!("{field}[{i}]");
            let row = row.as_array().filter(|r| r.len() <= 4).ok_or_else(|| {
                bad(
                    &row_field,
                    "must be up to 4 numbers (the three host reads four)",
                )
            })?;
            let mut out = [0.0; 4];
            for (k, v) in row.iter().enumerate() {
                out[k] = number(v, &row_field)?;
            }
            Ok(out)
        })
        .collect()
}

/// `collectDrawnItems` over the top-level list: each drawable's scene position and hit id for the bounds and the
/// hit index, a layer's items by their own, an fx layer none (its effects are never click targets). A skinned mesh's
/// position is offset by its posed hull's local minimum (`skinnedMeshDrawnItem`).
/// A drawable's scene position and its hit id.
pub(super) type DrawnItem = ([f64; 3], Option<String>);

pub(super) fn drawn_items(nodes: &[(String, Value)]) -> Result<Vec<DrawnItem>, CanvasSceneError> {
    let id_of = |f: &Fields| f.get("id").and_then(Value::as_str).map(str::to_string);
    let mut out = Vec::new();
    for (path, node) in nodes {
        let Some(f) = node.as_object() else { continue };
        match f.get("type").and_then(Value::as_str) {
            Some("draw-mesh" | "draw-group" | "draw-sprite" | "draw-shape" | "draw-text") => {
                out.push((required_pos(f, path)?, id_of(f)));
            }
            Some("draw-skinned-mesh") => out.push((skinned_mesh_pos(f, path)?, id_of(f))),
            Some("draw-sprite-layer" | "draw-shape-layer" | "draw-text-layer") => {
                let items = present(f, "items").and_then(Value::as_array);
                for (i, item) in items.into_iter().flatten().enumerate() {
                    // a malformed item is refused by name when the walk draws it
                    let Some(it) = item.as_object() else { continue };
                    out.push((required_pos(it, &format!("{path}.items[{i}]"))?, id_of(it)));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

pub(super) fn skinned_mesh_pos(f: &Fields, path: &str) -> Result<[f64; 3], CanvasSceneError> {
    let pos = required_pos(f, path)?;
    let bones = present(f, "bones")
        .and_then(Value::as_array)
        .filter(|b| !b.is_empty());
    let (Some(mesh), Some(bones)) = (present(f, "mesh"), bones) else {
        return Ok(pos);
    };
    let parse = |e: serde_json::Error| bad(path, format!("draw-skinned-mesh: {e}"));
    let mesh: SkinMesh = orbital_core_base::json_value::from_json_value(mesh).map_err(parse)?;
    let bones: Vec<BoneDef> =
        orbital_core_base::json_value::from_json_value(&(Value::Array(bones.clone())))
            .map_err(parse)?;
    let frames: Option<Vec<Pose>> = present(f, "clip")
        .and_then(|c| c.get("frames"))
        .map(orbital_core_base::json_value::from_json_value)
        .transpose()
        .map_err(parse)?;
    let frame = present(f, "frame").and_then(Value::as_u64);
    let pose = match (frames.as_deref(), frame) {
        (Some(frames), Some(i)) if !frames.is_empty() => frames[i as usize % frames.len()].clone(),
        _ => present(f, "pose")
            .map(orbital_core_base::json_value::from_json_value::<Pose>)
            .transpose()
            .map_err(parse)?
            .unwrap_or_default(),
    };
    let skin_error = |e: skinning_2d::SkinError| bad(path, format!("draw-skinned-mesh: {e}"));
    let bind = skinning_2d::compute_world_matrices(&bones, &Pose::new()).map_err(skin_error)?;
    let current = skinning_2d::compute_world_matrices(&bones, &pose).map_err(skin_error)?;
    let points = skinning_2d::skin_vertices(&mesh, &bind, &current).map_err(skin_error)?;
    Ok(match skinning_2d::vertex_bounds(&points) {
        Some(b) => [pos[0] + b[0], pos[1] + b[1], pos[2]],
        None => pos,
    })
}

/// `DrawMesh3D.polyhedronGeometry`: scene `[x, y, z]` (z up) to three `[x, z, y]` scaled by the cell; that swap
/// mirrors space, so each face `[a, b, c]` is emitted `[a, c, b]` to keep it outward.
pub(super) fn polyhedron(
    f: &Fields,
    path: &str,
    cell: f64,
) -> Result<(MeshData, f64), CanvasSceneError> {
    let v_path = join(path, "vertices");
    let vertices = present(f, "vertices")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(&v_path, "is required as an array"))?;
    if vertices.len() < 3 {
        return Err(bad(&v_path, "a polyhedron needs at least 3 vertices"));
    }
    let scene = vertices
        .iter()
        .enumerate()
        .map(|(i, v)| triple(v, &format!("{v_path}[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let min_z = scene.iter().map(|v| v[2]).fold(f64::INFINITY, f64::min);
    let positions = scene
        .iter()
        .map(|v| f32x3([v[0] * cell, v[2] * cell, v[1] * cell]))
        .collect::<Vec<_>>();

    let f_path = join(path, "faces");
    let faces = present(f, "faces")
        .and_then(Value::as_array)
        .filter(|faces| !faces.is_empty())
        .ok_or_else(|| bad(&f_path, "is required as a non-empty array"))?;
    let mut indices = Vec::with_capacity(faces.len());
    for (i, face) in faces.iter().enumerate() {
        let face_path = format!("{f_path}[{i}]");
        let tri = face
            .as_array()
            .filter(|t| t.len() == 3)
            .ok_or_else(|| bad(&face_path, "must be 3 vertex indices"))?;
        let mut abc = [0u32; 3];
        for (k, index) in tri.iter().enumerate() {
            abc[k] = index
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (*n as usize) < scene.len())
                .ok_or_else(|| {
                    bad(
                        &face_path,
                        format!("indices must be integers below {}", scene.len()),
                    )
                })?;
        }
        let [a, b, c] = abc;
        if a == b || b == c || a == c {
            return Err(bad(
                &face_path,
                "repeats a vertex (the three host skips the face)",
            ));
        }
        indices.push([a, c, b]);
    }

    let uv_path = join(path, "uvs");
    let uvs = match present(f, "uvs").and_then(Value::as_array) {
        None => None,
        Some(list) if list.is_empty() => None,
        Some(list) => {
            if list.len() != scene.len() {
                return Err(bad(
                    &uv_path,
                    format!(
                        "{} uvs for {} vertices (the three host drops them)",
                        list.len(),
                        scene.len()
                    ),
                ));
            }
            Some(
                list.iter()
                    .enumerate()
                    .map(|(i, uv)| {
                        let pair = uv
                            .as_array()
                            .filter(|p| p.len() == 2)
                            .ok_or_else(|| bad(&format!("{uv_path}[{i}]"), "must be [u, v]"))?;
                        Ok([
                            number(&pair[0], &format!("{uv_path}[{i}]"))? as f32,
                            number(&pair[1], &format!("{uv_path}[{i}]"))? as f32,
                        ])
                    })
                    .collect::<Result<Vec<_>, CanvasSceneError>>()?,
            )
        }
    };
    if present(f, "uvs").is_some_and(|v| !v.is_array()) {
        return Err(bad(&uv_path, "must be an array"));
    }
    let mesh = geometry::polyhedron(positions, &indices, uvs)
        .map_err(|e| bad(path, format!("polyhedron geometry: {}", e.0)))?;
    Ok((mesh, -min_z * cell))
}
