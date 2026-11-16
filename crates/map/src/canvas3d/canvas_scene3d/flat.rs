//! 2D drawables laid flat in a 3D scene: shapes, text, layers and fx layers, outlined and lifted off the ground.

use super::*;

pub(super) const SHAPE_KEYS: &[&str] = &[
    "type",
    "id",
    "shape",
    "position",
    "anchor",
    "width",
    "height",
    "radiusX",
    "radiusY",
    "offsetX",
    "offsetY",
    "points",
    "d",
    "fill",
    "gradient",
    "fillPattern",
    "stroke",
    "strokeWidth",
    "strokeGradient",
    "strokeDash",
    "strokeDashOffset",
    "blendMode",
    "blur",
    "rotate",
    "pivot",
    "shadow",
    "animation",
    "opacity",
];

pub(super) const TEXT_KEYS: &[&str] = &[
    "type", "id", "text", "position", "anchor", "offsetX", "offsetY", "color", "font", "align",
    "baseline", "opacity",
];

pub(super) const LAYER_KEYS: &[&str] = &["type", "id", "items"];

pub(super) const FX_LAYER_KEYS: &[&str] = &[
    "type",
    "id",
    "items",
    "presets",
    "art",
    "sprites",
    "tickMs",
    "textColor",
];

/// The shape props a keyframe animates, all drawn here.
pub(super) const SHAPE_TRACKS: &[&str] = &[
    "offsetX",
    "offsetY",
    "rotate",
    "opacity",
    "radiusX",
    "radiusY",
    "width",
    "height",
    "strokeWidth",
    "fill",
    "stroke",
];

/// `Shape3D` lifts every shape off the ground; a stroke sits a hair above its own fill so the two never depth-fight.
pub(super) const SHAPE_LIFT: f64 = 0.02;

pub(super) const STROKE_LIFT: f64 = 0.001;

/// `Shape3D`'s `ringGeometry(radiusX - 0.08, radiusX, 32)` and `circleGeometry(radiusX, 32)`; a cell is 0.95 wide.
pub(super) const RING_WIDTH: f64 = 0.08;

pub(super) const SHAPE_SEGMENTS: usize = 32;

pub(super) const CELL_FILL: f64 = 0.95;

/// A path flattens to within a quarter of the 2D painter's pixel at one cell.
pub(super) const PATH_TOLERANCE: f32 = 0.25 / TILE_WIDTH;

/// `SpriteBillboard`: a ground decal floats 0.01 off the ground, its loading placeholder 0.02, at 0.6 opacity.
pub(super) const DECAL_LIFT: f64 = 0.01;

pub(super) const PLACEHOLDER_LIFT: f64 = 0.02;

pub(super) const PLACEHOLDER_OPACITY: f64 = 0.6;

/// `Text3D`: drei `<Text>` in a `<Billboard>` 1.2 above the position, font size 0.32, a 0.02 black outline.
pub(super) const LABEL_LIFT: f64 = 1.2;

pub(super) const LABEL_EM: f32 = 0.32;

pub(super) const LABEL_OUTLINE: f32 = 0.02;

/// `Mesh3D`'s outline: an unlit back-face shell in `color`, standing `width` cells off the surface.
pub(super) struct Outline {
    pub(super) color: [f32; 3],
    pub(super) width: f64,
}

impl Outline {
    pub(super) fn instance(
        &self,
        mesh: MeshData,
        at: DMat4,
        opacity: f64,
        id: Option<String>,
    ) -> MeshInstance {
        MeshInstance {
            mesh,
            transform: at.as_mat4().to_cols_array(),
            material: Material {
                kind: MaterialKind::Basic,
                color: self.color,
                opacity: opacity as f32,
                side: Side::Back,
                ..Material::default()
            },
            cast_shadow: false,
            receive_shadow: false,
            id,
            glow: None,
        }
    }
}

/// The mesh's authored outline, else the theme's when it draws one (`applySceneMaterials`: the `ink` tone,
/// `--material-outline` × 0.02 wide).
pub(super) fn outline(
    f: &Fields,
    path: &str,
    theme: &SceneTheme,
) -> Result<Option<Outline>, CanvasSceneError> {
    let field = join(path, "outline");
    match opt_object(f, "outline", path)? {
        Some(o) => {
            check_keys(o, &["color", "width"], &field)?;
            let width = opt_number(o, "width", &field)?.unwrap_or(0.05);
            if width < 0.0 {
                return Err(bad(&join(&field, "width"), format!("{width} is negative")));
            }
            let color = match opt_rgb(o, "color", &field)? {
                Some(c) => c,
                None => rgb(&Value::String("#101014".into()), "outline default")?,
            };
            Ok(Some(Outline { color, width }))
        }
        None if theme.outline > 0.0 => {
            let color = match theme.tones.get("ink") {
                Some(css) => rgb(&Value::String(css.clone()), "theme tone ink")?,
                None => [1.0, 1.0, 1.0],
            };
            Ok(Some(Outline {
                color,
                width: f64::from(theme.outline) * 0.02,
            }))
        }
        None => Ok(None),
    }
}

/// An unlit, double-sided, blended surface: how `Shape3D` and `SpriteBillboard` draw.
pub(super) fn flat_material(
    flat: Flat,
    srgb: [f32; 3],
    opacity: f64,
    map: Option<String>,
) -> Material {
    Material {
        kind: MaterialKind::Flat(flat),
        color: srgb_to_linear_rgb(srgb),
        opacity: opacity as f32,
        side: Side::Double,
        map,
        ..Material::default()
    }
}

pub(super) fn flat_instance(
    mesh: MeshData,
    at: DMat4,
    material: Material,
    id: Option<String>,
) -> MeshInstance {
    MeshInstance {
        mesh,
        transform: at.as_mat4().to_cols_array(),
        material,
        cast_shadow: false,
        receive_shadow: false,
        id,
        glow: None,
    }
}

/// A shape's `shadow` as straight sRGB RGBA and its blur in cells; `None` when canvas draws none (no blur, since a
/// shadow here has no offset, or a transparent color).
pub(super) fn shadow_of(
    f: &Fields,
    path: &str,
) -> Result<Option<([f32; 4], f64)>, CanvasSceneError> {
    let Some(v) = present(f, "shadow").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let field = join(path, "shadow");
    let o = v
        .as_object()
        .ok_or_else(|| bad(&field, "must be {color, blur}"))?;
    check_keys(o, &["color", "blur"], &field)?;
    let color =
        opt_css(o, "color", &field)?.ok_or_else(|| bad(&join(&field, "color"), "is required"))?;
    let blur =
        opt_number(o, "blur", &field)?.ok_or_else(|| bad(&join(&field, "blur"), "is required"))?;
    if !(blur.is_finite() && blur >= 0.0) {
        return Err(bad(
            &join(&field, "blur"),
            format!("{blur} is not a finite blur of zero or more"),
        ));
    }
    Ok((blur > 0.0 && color[3] > 0.0).then_some((color, blur)))
}

/// `a` and `b` as one mesh.
pub(super) fn merged(mut a: MeshData, b: &MeshData) -> MeshData {
    let base = a.positions.len() as u32;
    a.positions.extend_from_slice(&b.positions);
    a.normals.extend_from_slice(&b.normals);
    a.uvs.extend_from_slice(&b.uvs);
    a.indices.extend(b.indices.iter().map(|i| i + base));
    a
}

/// three's flat shapes are built in XY and laid on the ground facing up: local +Y runs to world −Z.
pub(super) fn lay_flat() -> DMat4 {
    DMat4::from_rotation_x(-FRAC_PI_2)
}

/// An optional CSS color as straight sRGB RGBA, alpha included.
pub(super) fn opt_css(
    f: &Fields,
    key: &str,
    path: &str,
) -> Result<Option<[f32; 4]>, CanvasSceneError> {
    present(f, key)
        .map(|v| css(v, &join(path, key)))
        .transpose()
}

pub(super) fn tess<'a>(
    path: &'a str,
    what: &str,
) -> impl Fn(tessellate::TessError) -> CanvasSceneError + 'a {
    let what = what.to_owned();
    move |e| bad(path, format!("{what}: {e}"))
}

/// A shape `animation`: the keyframe contract `DrawShape` plays, its tracks restricted to what draws here.
pub(super) fn shape_animation(value: &Value, path: &str) -> Result<(), CanvasSceneError> {
    let a = value
        .as_object()
        .ok_or_else(|| bad(path, "must be an object"))?;
    check_keys(a, &["durationMs", "loop", "keyframes"], path)?;
    let duration = opt_number(a, "durationMs", path)?
        .ok_or_else(|| bad(&join(path, "durationMs"), "is required"))?;
    if duration <= 0.0 {
        return Err(bad(
            &join(path, "durationMs"),
            format!("{duration} must be above zero (the three host does not play it)"),
        ));
    }
    opt_bool(a, "loop", path)?;
    let frames_path = join(path, "keyframes");
    let frames = present(a, "keyframes")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(&frames_path, "is required as an array"))?;
    if frames.is_empty() {
        return Err(bad(
            &frames_path,
            "is empty (the three host does not play it)",
        ));
    }
    for (i, frame) in frames.iter().enumerate() {
        let frame_path = format!("{frames_path}[{i}]");
        let k = frame
            .as_object()
            .ok_or_else(|| bad(&frame_path, "must be an object"))?;
        if present(k, "at").is_none() {
            return Err(bad(&join(&frame_path, "at"), "is required"));
        }
        for (key, v) in k {
            let field = join(&frame_path, key);
            match key.as_str() {
                "fill" | "stroke" => {
                    css(v, &field)?;
                }
                "at" => {
                    number(v, &field)?;
                }
                "strokeDashOffset" | "blur" | "shadow" => {
                    return Err(undrawn(&field, STEP_PARITY));
                }
                track if SHAPE_TRACKS.contains(&track) => {
                    number(v, &field)?;
                }
                _ => return Err(bad(&field, "not a declared keyframe prop")),
            }
        }
    }
    Ok(())
}

/// A sprite `frame`: `{x, y, w, h}` image pixels, a positive size.
pub(super) fn frame_rect(f: &Fields, path: &str) -> Result<[f64; 4], CanvasSceneError> {
    check_keys(f, &["x", "y", "w", "h"], path)?;
    let mut out = [0.0; 4];
    for (i, key) in ["x", "y", "w", "h"].into_iter().enumerate() {
        out[i] = opt_number(f, key, path)?.ok_or_else(|| bad(&join(path, key), "is required"))?;
    }
    if out[2] <= 0.0 || out[3] <= 0.0 {
        return Err(bad(path, "w and h must be above zero"));
    }
    Ok(out)
}

/// Where a flat sprite's pixels come from, in the 2D painter's order.
pub(super) enum Source {
    Rect([f64; 4]),
    Whole,
    /// The atlas is still loading.
    Pending,
}

impl Walk<'_> {
    /// The clock a sprite sheet plays on when the sprite names none: the page's wall clock as the 2D painter uses it,
    /// else the canvas clock, else rest.
    pub(super) fn paint_clock(&self) -> f64 {
        self.epoch_ms.or(self.elapsed_ms).unwrap_or(0.0)
    }

    /// `Sprite3D`'s unresolvable-asset fallback: a `#9b8f7f` standard box at the position, `width` (0.6) square and
    /// `height` (0.6) tall.
    pub(super) fn sprite_fallback(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
    ) -> Result<(), CanvasSceneError> {
        let side = opt_number(f, "width", path)?.unwrap_or(0.6) * place.cell;
        let height = opt_number(f, "height", path)?.unwrap_or(0.6) * place.cell;
        let mesh = geometry::cuboid(side as f32, height as f32, side as f32)
            .map_err(|e| bad(path, format!("fallback box: {}", e.0)))?;
        let color = rgb(&Value::String("#9b8f7f".into()), "sprite fallback")?;
        self.meshes.push(MeshInstance {
            mesh,
            transform: (parent * DMat4::from_translation(place.world(required_pos(f, path)?)))
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

    /// `SpriteBillboard`: a `top-left` sprite (the default) is a ground decal `width` × `height` cells with its corner
    /// at the position; a `ground` or `center` one stands up as a camera-facing billboard `height` cells tall and
    /// `width` (or the image's aspect) wide. Nearest-sampled, cut out under alpha 0.1, grey while loading and red when
    /// the image fails. Beyond three: the 2D painter's frame/atlas/sheet source, `rotation` about the center,
    /// `flipX`, and the enclosing group's opacity on the billboard too.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn flat_sprite(
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
        if present(f, "shadow").is_some() {
            return Err(undrawn(&join(path, "shadow"), STEP_PARITY));
        }
        let decal = match opt_str(f, "anchor", path)? {
            None | Some("top-left") => true,
            Some("ground" | "center") => false,
            Some(_) => {
                return Err(bad(
                    &join(path, "anchor"),
                    "must be top-left, ground or center",
                ))
            }
        };
        let looping = opt_bool(f, "loop", path)?;
        let clock_ms = opt_number(f, "clockMs", path)?;
        let flip = opt_bool(f, "flipX", path)?.unwrap_or(false);
        let animation = opt_str(f, "animation", path)?;
        let sprite_name = opt_str(asset, "sprite", &asset_path)?;
        let source = match opt_object(f, "frame", path)? {
            Some(frame) => Source::Rect(frame_rect(frame, &join(path, "frame"))?),
            None => match opt_str(asset, "atlas", &asset_path)?.filter(|u| !u.is_empty()) {
                None => Source::Whole,
                Some(atlas_url) => match atlas::parsed_atlas(atlas_url) {
                    None if asset::fetch_failed(atlas_url)
                        || asset::asset_bytes(atlas_url).is_some() =>
                    {
                        return Err(bad(
                            &join(&asset_path, "atlas"),
                            format!("`{atlas_url}` does not load as an atlas"),
                        ))
                    }
                    None => Source::Pending,
                    Some(parsed) => match (&*parsed, animation, sprite_name) {
                        (atlas::ParsedAtlas::SpriteSheet(_), Some(row), _) => {
                            if clock_ms.is_none() {
                                self.animated = true;
                            }
                            let at = clock_ms.unwrap_or_else(|| self.paint_clock());
                            let r = atlas::sprite_sheet_frame(&parsed, row, looping, at)
                                .ok_or_else(|| {
                                    bad(
                                        &join(path, "animation"),
                                        format!("atlas `{atlas_url}` has no animation `{row}`"),
                                    )
                                })?;
                            Source::Rect([r.x, r.y, r.w, r.h].map(f64::from))
                        }
                        (_, _, Some(name)) => {
                            let r = atlas::sub_rect_for(&parsed, name).ok_or_else(|| {
                                bad(
                                    &join(&asset_path, "sprite"),
                                    format!("atlas `{atlas_url}` has no sprite `{name}`"),
                                )
                            })?;
                            Source::Rect([r.x, r.y, r.w, r.h].map(f64::from))
                        }
                        _ => Source::Whole,
                    },
                },
            },
        };
        let dims = asset::image_info(url);
        let failed = dims.is_none() && asset::image_failed(url);
        let (iw, ih) = dims.map_or((1.0, 1.0), |(w, h)| (f64::from(w), f64::from(h)));
        let crop = match source {
            Source::Rect(r) => Some(r),
            Source::Whole | Source::Pending => None,
        };
        let cell = place.cell;
        let width = opt_number(f, "width", path)?;
        let height = opt_number(f, "height", path)?;
        let (w, h) = if decal {
            (width.unwrap_or(1.0) * cell, height.unwrap_or(1.0) * cell)
        } else {
            let tall = height.unwrap_or(1.0);
            let aspect = crop.map_or(iw / ih, |r| r[2] / r[3]);
            (width.unwrap_or(tall * aspect) * cell, tall * cell)
        };
        let rotation = opt_number(f, "rotation", path)?.unwrap_or(0.0);
        let world = place.world(required_pos(f, path)?);
        let at = if decal {
            let lift = if dims.is_some() && !matches!(source, Source::Pending) {
                DECAL_LIFT
            } else {
                PLACEHOLDER_LIFT
            };
            parent
                * DMat4::from_translation(DVec3::new(world.x + w / 2.0, lift, world.z + h / 2.0))
                * DMat4::from_rotation_y(-rotation)
                * lay_flat()
        } else {
            parent * DMat4::from_translation(world + DVec3::new(0.0, h / 2.0, 0.0))
        };
        let quad = |uv: [f32; 4]| {
            let mut mesh =
                tessellate::quad(w as f32, h as f32, uv).map_err(tess(path, "sprite"))?;
            if !decal {
                tessellate::rotate(&mut mesh, -rotation as f32);
            }
            Ok::<_, CanvasSceneError>(mesh)
        };
        let ready = dims.is_some() && !matches!(source, Source::Pending);
        if !ready {
            if failed {
                tracing::warn!(target: "almadar::scene3d", url, "sprite image failed; drawing the error placeholder");
            }
            let hex = if failed { "#ff4444" } else { "#888888" };
            let color = css(&Value::String(hex.into()), "sprite placeholder")?;
            let material = flat_material(
                Flat::default(),
                [color[0], color[1], color[2]],
                PLACEHOLDER_OPACITY,
                None,
            );
            self.meshes.push(flat_instance(
                quad([0.0, 0.0, 1.0, 1.0])?,
                at,
                material,
                self.pick_id.clone(),
            ));
            return Ok(());
        }
        let [x0, y0, x1, y1] = match crop {
            Some([x, y, cw, ch]) => [x / iw, y / ih, (x + cw) / iw, (y + ch) / ih],
            None => [0.0, 0.0, 1.0, 1.0],
        };
        let uv = if flip {
            [x1, y0, x0, y1]
        } else {
            [x0, y0, x1, y1]
        };
        let opacity = opt_number(f, "opacity", path)?.unwrap_or(1.0) * group_opacity;
        let flat = Flat {
            nearest: true,
            cutout: true,
            additive: false,
            billboard: !decal,
        };
        let material = flat_material(flat, [1.0; 3], opacity, Some(url.to_owned()));
        self.meshes.push(flat_instance(
            quad(uv.map(|v| v as f32))?,
            at,
            material,
            self.pick_id.clone(),
        ));
        Ok(())
    }

    /// `Shape3D`: the shape built flat and laid on the ground 0.02 above the position, unlit, double-sided, blended,
    /// in `fill` (else `stroke`, else white): a cell 0.95 wide, a `width` × `height` rect, an ellipse of `radiusX`
    /// (a 0.08 ring when only stroked), a `points` polygon; and `path` (`d`), which three skips, filled by the nonzero
    /// rule and stroked as the 2D painter defines it. Beyond three: `radiusY`, `offsetX`/`offsetY` and `rotate` about
    /// `pivot` (as three's own animation moves the shape), a stroke drawn alongside a fill, `strokeWidth` in painter
    /// pixels (one cell is `TILE_WIDTH` of them), `lighter` blending, and every keyframe track the 2D painter plays.
    pub(super) fn shape(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, SHAPE_KEYS, path)?;
        for key in [
            "gradient",
            "fillPattern",
            "strokeGradient",
            "strokeDashOffset",
            "blur",
        ] {
            if present(f, key).is_some() {
                return Err(undrawn(&join(path, key), STEP_PARITY));
            }
        }
        if present(f, "strokeDash").is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty)) {
            return Err(undrawn(&join(path, "strokeDash"), STEP_PARITY));
        }
        let additive = match opt_str(f, "blendMode", path)? {
            None | Some("source-over") => false,
            Some("lighter") => true,
            Some(_) => return Err(undrawn(&join(path, "blendMode"), STEP_PARITY)),
        };
        match opt_str(f, "anchor", path)? {
            None | Some("center") => {}
            Some(_) => {
                return Err(bad(
                    &join(path, "anchor"),
                    "a 3D shape is centered on its position (only `center` places it so)",
                ))
            }
        }
        let world = place.world(required_pos(f, path)?);
        let animated;
        let view: &Fields = match present(f, "animation") {
            None => f,
            Some(declared) => {
                shape_animation(declared, &join(path, "animation"))?;
                self.animated = true;
                match self.elapsed_ms {
                    None => f,
                    Some(t) => {
                        let mut at = f.clone();
                        for key in SHAPE_TRACKS {
                            if let Some(v) =
                                orbital_canvas_base::animate::drawable_track(declared, t, key)
                            {
                                at.insert((*key).to_owned(), v);
                            }
                        }
                        animated = at;
                        &animated
                    }
                }
            }
        };
        let shape = opt_str(view, "shape", path)?
            .ok_or_else(|| bad(&join(path, "shape"), "is required"))?;
        let own: &[&str] = match shape {
            "cell" => &[],
            "rect" => &["width", "height"],
            "ellipse" => &["radiusX", "radiusY"],
            "poly" => &["points"],
            "path" => &["d"],
            _ => {
                return Err(bad(
                    &join(path, "shape"),
                    format!("`{shape}` is not a shape"),
                ))
            }
        };
        for key in ["width", "height", "radiusX", "radiusY", "points", "d"] {
            if present(view, key).is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
                && !own.contains(&key)
            {
                return Err(bad(
                    &join(path, key),
                    format!("shape `{shape}` has no {key}"),
                ));
            }
        }
        let cell = place.cell;
        let num = |key: &str| opt_number(view, key, path);
        let fill = opt_css(view, "fill", path)?;
        let stroke = opt_css(view, "stroke", path)?;
        let stroke_width = num("strokeWidth")?;
        if let Some(sw) = stroke_width {
            if sw <= 0.0 {
                return Err(bad(
                    &join(path, "strokeWidth"),
                    format!("{sw} is not above zero"),
                ));
            }
            if stroke.is_none() {
                return Err(bad(
                    &join(path, "strokeWidth"),
                    "sizes no stroke (stroke is unset)",
                ));
            }
            if fill.is_none() && matches!(shape, "cell" | "rect" | "poly") {
                return Err(bad(
                    &join(path, "strokeWidth"),
                    format!("a stroke-only {shape} is filled in its stroke color (three's Shape3D), so a width sizes nothing"),
                ));
            }
        }
        // painter pixels to world units: one cell is TILE_WIDTH of them
        let band = stroke_width.unwrap_or(1.0) * cell / f64::from(TILE_WIDTH);
        let outline_of = |points: Vec<[f32; 2]>| {
            tessellate::stroke(
                &[Subpath {
                    points,
                    closed: true,
                }],
                band as f32,
            )
            .map_err(tess(path, shape))
        };
        let rect_points = |w: f64, h: f64| {
            let (x, y) = ((w / 2.0) as f32, (h / 2.0) as f32);
            vec![[-x, -y], [x, -y], [x, y], [-x, y]]
        };
        let (body, edge) = match shape {
            "cell" | "rect" => {
                let (w, h) = if shape == "cell" {
                    (cell * CELL_FILL, cell * CELL_FILL)
                } else {
                    (
                        num("width")?.unwrap_or(1.0) * cell,
                        num("height")?.unwrap_or(1.0) * cell,
                    )
                };
                // a zero-wide rect is three's degenerate PlaneGeometry: nothing drawn, no error
                if w == 0.0 || h == 0.0 {
                    (None, None)
                } else {
                    let body = geometry::plane(w as f32, h as f32)
                        .map_err(|e| bad(path, format!("{shape}: {}", e.0)))?;
                    let edge = match (fill, stroke) {
                        (Some(_), Some(_)) => Some(outline_of(rect_points(w, h))?),
                        _ => None,
                    };
                    (Some(body), edge)
                }
            }
            "ellipse" => {
                let rx = num("radiusX")?.unwrap_or(0.4);
                let ry = num("radiusY")?.unwrap_or(rx);
                match (fill, stroke) {
                    (None, Some(_)) => {
                        let width = stroke_width.map_or(RING_WIDTH, |_| band);
                        let ring = tessellate::ring(
                            [(rx - width).max(0.0) as f32, (ry - width).max(0.0) as f32],
                            [rx as f32, ry as f32],
                            SHAPE_SEGMENTS,
                        )
                        .map_err(tess(path, shape))?;
                        (Some(ring), None)
                    }
                    _ => {
                        let body = tessellate::ellipse(rx as f32, ry as f32, SHAPE_SEGMENTS)
                            .map_err(tess(path, shape))?;
                        let edge = match (fill, stroke) {
                            (Some(_), Some(_)) => Some(
                                tessellate::ring(
                                    [
                                        (rx - band / 2.0).max(0.0) as f32,
                                        (ry - band / 2.0).max(0.0) as f32,
                                    ],
                                    [(rx + band / 2.0) as f32, (ry + band / 2.0) as f32],
                                    SHAPE_SEGMENTS,
                                )
                                .map_err(tess(path, shape))?,
                            ),
                            _ => None,
                        };
                        (Some(body), edge)
                    }
                }
            }
            "poly" => {
                let p_path = join(path, "points");
                let list = present(view, "points")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad(&p_path, "is required as an array"))?;
                if list.len() < 3 {
                    return Err(bad(
                        &p_path,
                        format!("{} points enclose no area (a poly needs 3)", list.len()),
                    ));
                }
                let mut points = Vec::with_capacity(list.len());
                for (i, p) in list.iter().enumerate() {
                    let field = format!("{p_path}[{i}]");
                    let o = p.as_object().ok_or_else(|| bad(&field, "must be {x, y}"))?;
                    check_keys(o, &["x", "y"], &field)?;
                    let axis = |k: &str| {
                        opt_number(o, k, &field)?
                            .ok_or_else(|| bad(&join(&field, k), "is required"))
                    };
                    points.push([axis("x")? as f32, axis("y")? as f32]);
                }
                let body = tessellate::fill(&[points.clone()]).map_err(tess(path, shape))?;
                let edge = match (fill, stroke) {
                    (Some(_), Some(_)) => Some(outline_of(points)?),
                    _ => None,
                };
                (Some(body), edge)
            }
            _ => {
                let d = opt_str(view, "d", path)?
                    .ok_or_else(|| bad(&join(path, "d"), "is required"))?;
                let segments = parse_path_data(d, Xf::identity()).ok_or_else(|| {
                    bad(
                        &join(path, "d"),
                        "is not SVG path data the 2D painter reads",
                    )
                })?;
                let subpaths =
                    tessellate::flatten(&segments, PATH_TOLERANCE).map_err(tess(path, shape))?;
                let body = match (fill, stroke) {
                    (None, Some(_)) => None,
                    _ => {
                        let rings: Vec<Vec<[f32; 2]>> =
                            subpaths.iter().map(|s| s.points.clone()).collect();
                        Some(tessellate::fill(&rings).map_err(tess(path, shape))?)
                    }
                };
                let edge = match stroke {
                    Some(_) => Some(
                        tessellate::stroke(&subpaths, band as f32).map_err(tess(path, shape))?,
                    ),
                    None => None,
                };
                (body, edge)
            }
        };
        let offset = DVec3::new(
            num("offsetX")?.unwrap_or(0.0) * cell,
            SHAPE_LIFT,
            num("offsetY")?.unwrap_or(0.0) * cell,
        );
        let pivot = match opt_object(f, "pivot", path)? {
            None => DVec3::ZERO,
            Some(p) => {
                let field = join(path, "pivot");
                check_keys(p, &["x", "y"], &field)?;
                let axis = |k: &str| {
                    opt_number(p, k, &field)?.ok_or_else(|| bad(&join(&field, k), "is required"))
                };
                DVec3::new((axis("x")? - 0.5) * cell, 0.0, (axis("y")? - 0.5) * cell)
            }
        };
        let at = parent
            * DMat4::from_translation(world + offset + pivot)
            * DMat4::from_rotation_y(-num("rotate")?.unwrap_or(0.0))
            * DMat4::from_translation(-pivot)
            * lay_flat();
        let opacity = num("opacity")?.unwrap_or(1.0) * group_opacity;
        let flat = Flat {
            additive,
            ..Flat::default()
        };
        let paint =
            |c: [f32; 4]| flat_material(flat, [c[0], c[1], c[2]], opacity * f64::from(c[3]), None);
        let white = [1.0; 4];
        if let Some((color, blur)) = shadow_of(view, path)? {
            let coverage = body
                .iter()
                .chain(edge.iter())
                .fold(MeshData::default(), merged);
            if !coverage.indices.is_empty() {
                // canvas shadowBlur is twice the Gaussian sigma; `blur` is in cells
                let mut glow = flat_instance(coverage, at, paint(color), None);
                glow.glow = Some((blur * cell / 2.0) as f32);
                self.meshes.push(glow);
            }
        }
        if let Some(mesh) = body {
            let color = match shape {
                "path" => fill.unwrap_or(white),
                _ => fill.or(stroke).unwrap_or(white),
            };
            self.meshes
                .push(flat_instance(mesh, at, paint(color), self.pick_id.clone()));
        }
        if let (Some(mesh), Some(color)) = (edge, stroke) {
            let lifted = at * DMat4::from_translation(DVec3::new(0.0, 0.0, STROKE_LIFT));
            self.meshes.push(flat_instance(
                mesh,
                lifted,
                paint(color),
                self.pick_id.clone(),
            ));
        }
        Ok(())
    }

    /// `Text3D`: drei `<Text>` (size 0.32, centered, a 0.02 black outline) in a camera-facing `<Billboard>` 1.2 above
    /// the position, in `color` (white when unset). The glyphs are the 2D painter's bundled faces, not troika's
    /// Roboto. Beyond three: `opacity`, the 2D `font` (its pixels at `TILE_WIDTH` per cell) and weight, `align`,
    /// `baseline`, and `offsetX`/`offsetY` in cells across and down the label's own plane.
    pub(super) fn text(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        group_opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, TEXT_KEYS, path)?;
        let world = place.world(required_pos(f, path)?);
        match opt_str(f, "anchor", path)? {
            None | Some("center") => {}
            Some(_) => {
                return Err(bad(
                    &join(path, "anchor"),
                    "a 3D label stands over its position (only `center` places it so)",
                ))
            }
        }
        let color = match opt_str(f, "color", path)? {
            None | Some("") => [1.0; 4],
            Some(c) => css(&Value::String(c.to_owned()), &join(path, "color"))?,
        };
        let (em, bold, family) = match opt_str(f, "font", path)? {
            None => (
                LABEL_EM,
                false,
                orbital_canvas_base::packet::TextFamily::Sans,
            ),
            Some(font) => {
                let (px, weight, family) = crate::game_canvas_2d::parse_text_font(Some(font))
                    .ok_or_else(|| {
                        bad(
                            &join(path, "font"),
                            format!("`{font}` is not a font the bundled faces draw"),
                        )
                    })?;
                (
                    (f64::from(px) * place.cell / f64::from(TILE_WIDTH)) as f32,
                    weight >= 600,
                    family,
                )
            }
        };
        let across = match opt_str(f, "align", path)? {
            None | Some("center") => 0.0,
            Some("left" | "start") => 0.5,
            Some("right" | "end") => -0.5,
            Some(_) => {
                return Err(bad(
                    &join(path, "align"),
                    "must be center, left, start, right or end",
                ))
            }
        };
        let baseline = opt_str(f, "baseline", path)?;
        if let Some(b) =
            baseline.filter(|b| !matches!(*b, "middle" | "alphabetic" | "top" | "bottom"))
        {
            return Err(match b {
                "hanging" | "ideographic" => undrawn(&join(path, "baseline"), STEP_PARITY),
                _ => bad(&join(path, "baseline"), "not a declared baseline"),
            });
        }
        let opacity = opt_number(f, "opacity", path)?.unwrap_or(1.0) * group_opacity;
        let (ox, oy) = (
            opt_number(f, "offsetX", path)?.unwrap_or(0.0) * place.cell,
            opt_number(f, "offsetY", path)?.unwrap_or(0.0) * place.cell,
        );
        let text = opt_str(f, "text", path)?.unwrap_or("");
        if text.is_empty() {
            return Ok(());
        }
        let style = LabelStyle {
            em,
            bold,
            family,
            color,
            halo: LABEL_OUTLINE,
            halo_color: [0.0, 0.0, 0.0, 1.0],
        };
        let l = label(text, &style).map_err(|e| bad(&join(path, "text"), e))?;
        let (asc, desc) = (l.ascent, l.descent);
        let up = match baseline {
            Some("alphabetic") => (asc + desc) / 2.0,
            Some("top") => -(asc - desc) / 2.0,
            Some("bottom") => (asc - desc) / 2.0,
            _ => 0.0,
        };
        let mut mesh = tessellate::quad(l.width, l.height, [0.0, 0.0, 1.0, 1.0])
            .map_err(tess(path, "label"))?;
        tessellate::translate(
            &mut mesh,
            [across * l.text_width + ox as f32, up - oy as f32],
        );
        let flat = Flat {
            billboard: true,
            ..Flat::default()
        };
        let at = parent * DMat4::from_translation(world + DVec3::new(0.0, LABEL_LIFT, 0.0));
        let material = flat_material(flat, [1.0; 3], opacity, Some(l.key));
        self.meshes
            .push(flat_instance(mesh, at, material, self.pick_id.clone()));
        Ok(())
    }

    /// `draw-sprite-layer` / `draw-shape-layer` / `draw-text-layer`: each item drawn as its atom, picked by its own id.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn layer(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        opacity: f64,
        atom: &str,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, LAYER_KEYS, path)?;
        let items_path = join(path, "items");
        let items = present(f, "items")
            .ok_or_else(|| bad(&items_path, "is required"))?
            .as_array()
            .ok_or_else(|| bad(&items_path, "must be an array"))?;
        let outer = self.pick_id.clone();
        for (i, item) in items.iter().enumerate() {
            let item_path = format!("{items_path}[{i}]");
            let fields = item
                .as_object()
                .ok_or_else(|| bad(&item_path, "not a drawable node"))?;
            if fields.get("type").and_then(Value::as_str) != Some(atom) {
                return Err(bad(&join(&item_path, "type"), format!("must be `{atom}`")));
            }
            self.pick_id = fields
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| outer.clone());
            let drawn = match atom {
                "draw-sprite" => self.sprite(fields, &item_path, parent, place, opacity),
                "draw-shape" => self.shape(fields, &item_path, parent, place, opacity),
                _ => self.text(fields, &item_path, parent, place, opacity),
            };
            self.pick_id = outer.clone();
            drawn?;
        }
        Ok(())
    }

    /// `FxLayer3D`: the layer expanded by the shared projection (`fx_projection`, as the 2D painter expands it) on the
    /// page clock, its pieces drawn as their atoms; a live layer keeps the host mapping frames.
    pub(super) fn fx_layer(
        &mut self,
        f: &Fields,
        path: &str,
        parent: DMat4,
        place: Placement,
        opacity: f64,
    ) -> Result<(), CanvasSceneError> {
        check_keys(f, FX_LAYER_KEYS, path)?;
        let layer = orbital_canvas_base::fx_projection::with_host_defaults(f);
        if layer["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            self.animated = true;
        }
        let pieces = orbital_canvas_base::fx_projection::expand_layer(
            &layer,
            self.epoch_ms.unwrap_or(0.0),
            "3d",
            f64::from(TILE_WIDTH),
        )?;
        for (i, piece) in pieces.iter().enumerate() {
            self.drawable(piece, &format!("{path}.fx[{i}]"), parent, place, opacity)?;
        }
        Ok(())
    }
}
