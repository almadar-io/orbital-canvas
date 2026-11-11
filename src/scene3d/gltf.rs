//! GLB models as three.js's `GLTFLoader` + `SkeletonUtils.clone` + `AnimationMixer` draw them in `ModelLoader`: meshes
//! per primitive under their node transforms, standard materials, skins bound in attached mode with weights
//! normalized, and one named clip played on `LoopRepeat` after a 0.15 s fade-in from the rest pose. Anything in the
//! file three would draw that this module does not is a typed error naming it, never a silent drop.

use super::model::{Material, MaterialKind, MeshData, MeshInstance, Side};
use super::skin;
use glam::{Mat4, Quat, Vec3, Vec4};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

/// `action.fadeIn(0.15)` in `ModelLoader`.
pub const FADE_IN_SECONDS: f32 = 0.15;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GltfError {
    /// three's `GLTFLoader` fails on this file too.
    #[error("not a loadable glTF: {0}")]
    Invalid(String),
    /// three draws this; orbital-canvas does not yet.
    #[error("{field}: {why}")]
    Unsupported { field: String, why: String },
}

fn unsupported(field: impl Into<String>, why: impl Into<String>) -> GltfError {
    GltfError::Unsupported {
        field: field.into(),
        why: why.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Trs {
    translation: Vec3,
    rotation: Quat,
    scale: Vec3,
}

impl Trs {
    fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

#[derive(Debug, Clone)]
struct Node {
    rest: Trs,
    children: Vec<usize>,
    mesh: Option<usize>,
    skin: Option<usize>,
}

#[derive(Debug, Clone)]
struct Influences {
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
}

#[derive(Debug, Clone)]
struct Primitive {
    mesh: MeshData,
    material: Material,
    influences: Option<Influences>,
}

#[derive(Debug, Clone)]
struct SkinDef {
    joints: Vec<usize>,
    inverse_bind: Vec<Mat4>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Property {
    Translation,
    Rotation,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    Step,
    CubicSpline,
}

/// One keyframe track. `values` holds `stride` floats per key, three per key (in-tangent, value, out-tangent) for
/// cubic splines, as glTF lays them out.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub node: usize,
    pub property: Property,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
}

impl Channel {
    fn stride(&self) -> usize {
        match self.property {
            Property::Rotation => 4,
            Property::Translation | Property::Scale => 3,
        }
    }

    fn value(&self, key: usize) -> Vec<f32> {
        let stride = self.stride();
        let start = match self.interpolation {
            Interpolation::CubicSpline => key * stride * 3 + stride,
            _ => key * stride,
        };
        self.values[start..start + stride].to_vec()
    }

    /// three's `Interpolant.evaluate`: before the first key and from the last key on, the end values hold.
    pub fn sample(&self, t: f32) -> Vec<f32> {
        let last = self.times.len() - 1;
        if t < self.times[0] {
            return self.value(0);
        }
        if t >= self.times[last] {
            return self.value(last);
        }
        let i1 = self.times.partition_point(|&key| key <= t);
        let (t0, t1) = (self.times[i1 - 1], self.times[i1]);
        let stride = self.stride();
        match self.interpolation {
            Interpolation::Step => self.value(i1 - 1),
            Interpolation::Linear => {
                let (a, b) = (self.value(i1 - 1), self.value(i1));
                let w1 = (t - t0) / (t1 - t0);
                if self.property == Property::Rotation {
                    slerp_flat(arr4(&a), arr4(&b), w1).to_vec()
                } else {
                    let w0 = 1.0 - w1;
                    (0..stride).map(|i| a[i] * w0 + b[i] * w1).collect()
                }
            }
            Interpolation::CubicSpline => {
                let td = t1 - t0;
                let p = (t - t0) / td;
                let (pp, ppp) = (p * p, p * p * p);
                let s2 = -2.0 * ppp + 3.0 * pp;
                let s3 = ppp - pp;
                let s0 = 1.0 - s2;
                let s1 = s3 - pp + p;
                let (o1, o0) = (i1 * stride * 3, (i1 - 1) * stride * 3);
                let v = &self.values;
                let mut out: Vec<f32> = (0..stride)
                    .map(|i| {
                        let p0 = v[o0 + i + stride];
                        let m0 = v[o0 + i + stride * 2] * td;
                        let p1 = v[o1 + i + stride];
                        let m1 = v[o1 + i] * td;
                        s0 * p0 + s1 * m0 + s2 * p1 + s3 * m1
                    })
                    .collect();
                if self.property == Property::Rotation {
                    let q = Vec4::from_slice(&out);
                    let len = q.length();
                    let n = if len == 0.0 {
                        Vec4::new(0.0, 0.0, 0.0, 1.0)
                    } else {
                        q / len
                    };
                    out = n.to_array().to_vec();
                }
                out
            }
        }
    }
}

fn arr4(v: &[f32]) -> [f32; 4] {
    [v[0], v[1], v[2], v[3]]
}

/// three's `Quaternion.slerpFlat`.
pub fn slerp_flat(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    if t == 0.0 {
        return a;
    }
    if t == 1.0 {
        return b;
    }
    if a == b {
        return a;
    }
    let (a, b) = (a.map(f64::from), b.map(f64::from));
    let mut t = f64::from(t);
    let mut s = 1.0 - t;
    let cos = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let dir = if cos >= 0.0 { 1.0 } else { -1.0 };
    let sqr_sin = 1.0 - cos * cos;
    if sqr_sin > f64::EPSILON {
        let sin = sqr_sin.sqrt();
        let len = sin.atan2(cos * dir);
        s = (s * len).sin() / sin;
        t = (t * len).sin() / sin;
    }
    let t_dir = t * dir;
    let mut out = [0.0f64; 4];
    for i in 0..4 {
        out[i] = a[i] * s + b[i] * t_dir;
    }
    if s == 1.0 - t {
        let f =
            1.0 / (out[0] * out[0] + out[1] * out[1] + out[2] * out[2] + out[3] * out[3]).sqrt();
        out = out.map(|c| c * f);
    }
    out.map(|c| c as f32)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub name: String,
    /// three's `AnimationClip.resetDuration`: the latest key of any track.
    pub duration: f32,
    pub channels: Vec<Channel>,
}

impl Clip {
    /// The mixer's `LoopRepeat` wrap of an action that has run for `elapsed` seconds.
    pub fn time(&self, elapsed: f32) -> f32 {
        if self.duration == 0.0 || (elapsed >= 0.0 && elapsed < self.duration) {
            return elapsed;
        }
        elapsed - self.duration * (elapsed / self.duration).floor()
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddedImage {
    pub key: String,
    pub bytes: Arc<[u8]>,
    pub mime: String,
}

#[derive(Debug, Clone)]
pub struct Model {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    meshes: Vec<Vec<Primitive>>,
    skins: Vec<SkinDef>,
    pub clips: Vec<Clip>,
    pub images: Vec<EmbeddedImage>,
    rest_bounds: Option<(Vec3, Vec3)>,
}

/// `ModelLoader.detectAssetRoot`: everything through a `/3d/` segment, else the model's own directory.
pub fn asset_root(model_url: &str) -> String {
    match model_url.find("/3d/") {
        Some(i) => model_url[..i + 4].to_string(),
        None => match model_url.rfind('/') {
            Some(i) => model_url[..=i].to_string(),
            None => String::new(),
        },
    }
}

/// three's `LoaderUtils.resolveURL`.
pub fn resolve_url(url: &str, path: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    let lower = |s: &str| s.to_ascii_lowercase();
    let is_http = |s: &str| lower(s).starts_with("http://") || lower(s).starts_with("https://");
    let mut base = path.to_string();
    if is_http(path) && url.starts_with('/') {
        let scheme_end = path.find("//").map_or(0, |i| i + 2);
        let host_end = path[scheme_end..]
            .find('/')
            .map_or(path.len(), |i| scheme_end + i);
        base = path[..host_end].to_string();
    }
    if is_http(url) || url.starts_with("//") {
        return url.to_string();
    }
    if lower(url).starts_with("data:") && url.contains(',') {
        return url.to_string();
    }
    if lower(url).starts_with("blob:") {
        return url.to_string();
    }
    base + url
}

impl Model {
    /// Parse GLB (or self-contained glTF) bytes fetched from `url`; relative image URIs resolve against
    /// [`asset_root`] of it, as `ModelLoader` sets the loader's resource path.
    pub fn from_glb(bytes: &[u8], url: &str) -> Result<Model, GltfError> {
        let gltf =
            ::gltf::Gltf::from_slice(bytes).map_err(|e| GltfError::Invalid(e.to_string()))?;
        let document = &gltf.document;
        if let Some(ext) = document.extensions_used().map(|e| e.to_string()).next() {
            return Err(unsupported(
                format!("glTF extension {ext}"),
                "not yet drawn by orbital-canvas",
            ));
        }
        for buffer in document.buffers() {
            if let ::gltf::buffer::Source::Uri(_) = buffer.source() {
                return Err(unsupported(
                    format!("glTF buffers[{}]", buffer.index()),
                    "an external or data-URI buffer is not yet read by orbital-canvas (GLB binary chunks are)",
                ));
            }
        }
        let blob = gltf.blob.as_deref();
        if document.buffers().next().is_some() && blob.is_none() {
            return Err(GltfError::Invalid(
                "a buffer names the GLB binary chunk, which is missing".into(),
            ));
        }
        let data = move |buffer: ::gltf::Buffer| match buffer.source() {
            ::gltf::buffer::Source::Bin => blob,
            ::gltf::buffer::Source::Uri(_) => None,
        };

        let scene = document
            .default_scene()
            .or_else(|| document.scenes().next())
            .ok_or_else(|| GltfError::Invalid("the file has no scene".into()))?;
        let roots: Vec<usize> = scene.nodes().map(|n| n.index()).collect();

        let nodes: Vec<Node> = document
            .nodes()
            .map(|n| {
                let (t, r, s) = n.transform().decomposed();
                Node {
                    rest: Trs {
                        translation: Vec3::from(t),
                        rotation: Quat::from_array(r),
                        scale: Vec3::from(s),
                    },
                    children: n.children().map(|c| c.index()).collect(),
                    mesh: n.mesh().map(|m| m.index()),
                    skin: n.skin().map(|s| s.index()),
                }
            })
            .collect();

        let mut skinned_meshes = HashSet::new();
        for node in &nodes {
            if let (Some(mesh), Some(_)) = (node.mesh, node.skin) {
                skinned_meshes.insert(mesh);
            }
        }
        for (i, node) in nodes.iter().enumerate() {
            if let (Some(mesh), None) = (node.mesh, node.skin) {
                if skinned_meshes.contains(&mesh) {
                    return Err(unsupported(
                        format!("glTF nodes[{i}]"),
                        format!(
                            "draws skinned meshes[{mesh}] without a skin (three leaves it unbound)"
                        ),
                    ));
                }
            }
        }

        let root = asset_root(url);
        let mut images = Vec::new();
        let mut image_keys = HashMap::new();
        for image in document.images() {
            let key = match image.source() {
                ::gltf::image::Source::View { view, mime_type } => {
                    let start = view.offset();
                    let bytes = blob
                        .and_then(|b| b.get(start..start + view.length()))
                        .ok_or_else(|| {
                            GltfError::Invalid(format!(
                                "images[{}] lies past the binary chunk",
                                image.index()
                            ))
                        })?;
                    let key = format!("{url}#gltf-image-{}", image.index());
                    images.push(EmbeddedImage {
                        key: key.clone(),
                        bytes: bytes.into(),
                        mime: mime_type.to_string(),
                    });
                    key
                }
                ::gltf::image::Source::Uri { uri, .. } => resolve_url(uri, &root),
            };
            image_keys.insert(image.index(), key);
        }

        let mut meshes = Vec::new();
        for mesh in document.meshes() {
            let skinned = skinned_meshes.contains(&mesh.index());
            let mut primitives = Vec::new();
            for (p, primitive) in mesh.primitives().enumerate() {
                let field = format!("glTF meshes[{}].primitives[{p}]", mesh.index());
                primitives.push(read_primitive(
                    &primitive,
                    &field,
                    skinned,
                    blob,
                    &image_keys,
                )?);
            }
            meshes.push(primitives);
        }

        let mut skins = Vec::new();
        for s in document.skins() {
            let joints: Vec<usize> = s.joints().map(|j| j.index()).collect();
            let inverse_bind = match s.reader(data).read_inverse_bind_matrices() {
                Some(m) => m.map(|c| Mat4::from_cols_array_2d(&c)).collect(),
                None => vec![Mat4::IDENTITY; joints.len()],
            };
            if inverse_bind.len() != joints.len() {
                return Err(GltfError::Invalid(format!(
                    "skins[{}] has {} inverse bind matrices for {} joints",
                    s.index(),
                    inverse_bind.len(),
                    joints.len()
                )));
            }
            skins.push(SkinDef {
                joints,
                inverse_bind,
            });
        }

        let mut clips = Vec::new();
        for animation in document.animations() {
            let mut channels: Vec<Channel> = Vec::new();
            for (c, channel) in animation.channels().enumerate() {
                let field = format!("glTF animations[{}].channels[{c}]", animation.index());
                let target = channel.target();
                let property = match target.property() {
                    ::gltf::animation::Property::Translation => Property::Translation,
                    ::gltf::animation::Property::Rotation => Property::Rotation,
                    ::gltf::animation::Property::Scale => Property::Scale,
                    ::gltf::animation::Property::MorphTargetWeights => {
                        return Err(unsupported(
                            field,
                            "morph target weights are not yet drawn by orbital-canvas",
                        ))
                    }
                };
                let node = target.node().index();
                if channels
                    .iter()
                    .any(|c| c.node == node && c.property == property)
                {
                    return Err(unsupported(
                        field,
                        "a second track on one node property (three averages them)",
                    ));
                }
                let interpolation = match channel.sampler().interpolation() {
                    ::gltf::animation::Interpolation::Linear => Interpolation::Linear,
                    ::gltf::animation::Interpolation::Step => Interpolation::Step,
                    ::gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
                };
                let reader = channel.reader(data);
                let times: Vec<f32> = reader
                    .read_inputs()
                    .ok_or_else(|| GltfError::Invalid(format!("{field} has no input")))?
                    .collect();
                let values: Vec<f32> = match reader
                    .read_outputs()
                    .ok_or_else(|| GltfError::Invalid(format!("{field} has no output")))?
                {
                    ::gltf::animation::util::ReadOutputs::Translations(it) => {
                        it.flatten().collect()
                    }
                    ::gltf::animation::util::ReadOutputs::Scales(it) => it.flatten().collect(),
                    ::gltf::animation::util::ReadOutputs::Rotations(it) => {
                        it.into_f32().flatten().collect()
                    }
                    ::gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => {
                        return Err(unsupported(
                            field,
                            "morph target weights are not yet drawn by orbital-canvas",
                        ))
                    }
                };
                let c = Channel {
                    node,
                    property,
                    interpolation,
                    times,
                    values,
                };
                let per_key = c.stride()
                    * if interpolation == Interpolation::CubicSpline {
                        3
                    } else {
                        1
                    };
                if c.times.is_empty() || c.values.len() != c.times.len() * per_key {
                    return Err(GltfError::Invalid(format!(
                        "{field}: {} keys for {} output values",
                        c.times.len(),
                        c.values.len()
                    )));
                }
                channels.push(c);
            }
            let duration = channels
                .iter()
                .map(|c| c.times[c.times.len() - 1])
                .fold(0.0, f32::max);
            clips.push(Clip {
                name: animation.name().unwrap_or("").to_string(),
                duration,
                channels,
            });
        }

        let mut model = Model {
            nodes,
            roots,
            meshes,
            skins,
            clips,
            images,
            rest_bounds: None,
        };
        let world = model.world(&model.rest_pose());
        for (i, s) in model.skins.iter().enumerate() {
            for &joint in &s.joints {
                if world[joint].is_none() {
                    return Err(unsupported(
                        format!("glTF skins[{i}]"),
                        format!(
                            "joint nodes[{joint}] is outside the scene (three never updates it)"
                        ),
                    ));
                }
            }
        }
        model.rest_bounds = model.bounds(&model.instances(&world)?);
        Ok(model)
    }

    /// Put the embedded images where renderers resolve material maps.
    pub fn register_images(&self) {
        for image in &self.images {
            crate::asset::register_asset(&image.key, image.bytes.clone(), Some(&image.mime));
        }
    }

    /// `ModelLoader`'s clip pick: the exact name, else the first case-insensitive match.
    pub fn clip_index(&self, name: &str) -> Option<usize> {
        let wanted = name.to_lowercase();
        self.clips.iter().position(|c| c.name == name).or_else(|| {
            self.clips
                .iter()
                .position(|c| c.name.to_lowercase() == wanted)
        })
    }

    /// `Box3.setFromObject` over the rest pose: each mesh's object-space box, transformed by its node, unioned.
    pub fn rest_bounds(&self) -> Option<(Vec3, Vec3)> {
        self.rest_bounds
    }

    /// Every texture the model's materials name.
    pub fn map_urls(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for p in self.meshes.iter().flatten() {
            if let Some(map) = &p.material.map {
                if !out.contains(map) {
                    out.push(map.clone());
                }
            }
        }
        out
    }

    fn rest_pose(&self) -> Vec<Trs> {
        self.nodes.iter().map(|n| n.rest).collect()
    }

    /// The local transforms after `clip` has played for `elapsed` seconds: the mixer samples the clip at its looped
    /// time and blends it with the rest pose by the fade-in weight.
    fn animated_pose(&self, clip: Option<usize>, elapsed: f32) -> Vec<Trs> {
        let mut pose = self.rest_pose();
        let Some(clip) = clip.and_then(|i| self.clips.get(i)) else {
            return pose;
        };
        let weight = (elapsed / FADE_IN_SECONDS).clamp(0.0, 1.0);
        if weight <= 0.0 {
            return pose;
        }
        let t = clip.time(elapsed);
        for channel in &clip.channels {
            let Some(trs) = pose.get_mut(channel.node) else {
                continue;
            };
            let v = channel.sample(t);
            match channel.property {
                Property::Rotation => {
                    let rest = trs.rotation.to_array();
                    let q = if weight < 1.0 {
                        slerp_flat(arr4(&v), rest, 1.0 - weight)
                    } else {
                        arr4(&v)
                    };
                    trs.rotation = Quat::from_array(q);
                }
                Property::Translation | Property::Scale => {
                    let target = if channel.property == Property::Translation {
                        &mut trs.translation
                    } else {
                        &mut trs.scale
                    };
                    let sampled = Vec3::new(v[0], v[1], v[2]);
                    *target = if weight < 1.0 {
                        sampled * weight + *target * (1.0 - weight)
                    } else {
                        sampled
                    };
                }
            }
        }
        pose
    }

    fn world(&self, pose: &[Trs]) -> Vec<Option<Mat4>> {
        let mut world = vec![None; self.nodes.len()];
        let mut stack: Vec<(usize, Mat4)> = self
            .roots
            .iter()
            .rev()
            .map(|&r| (r, Mat4::IDENTITY))
            .collect();
        while let Some((node, parent)) = stack.pop() {
            if world[node].is_some() {
                continue;
            }
            let m = parent * pose[node].matrix();
            world[node] = Some(m);
            for &child in self.nodes[node].children.iter().rev() {
                stack.push((child, m));
            }
        }
        world
    }

    fn instances(&self, world: &[Option<Mat4>]) -> Result<Vec<MeshInstance>, GltfError> {
        let mut out = Vec::new();
        let mut order = Vec::new();
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(node) = stack.pop() {
            order.push(node);
            stack.extend(self.nodes[node].children.iter().rev());
        }
        for node in order {
            let (Some(mesh), Some(transform)) = (self.nodes[node].mesh, world[node]) else {
                continue;
            };
            let bones = match self.nodes[node].skin {
                None => None,
                Some(s) => {
                    let skin = &self.skins[s];
                    let joint_world: Vec<Mat4> = skin
                        .joints
                        .iter()
                        .map(|&j| world[j].unwrap_or(Mat4::IDENTITY))
                        .collect();
                    Some(skin::bone_matrices(&joint_world, &skin.inverse_bind))
                }
            };
            for (p, primitive) in self.meshes[mesh].iter().enumerate() {
                let data = match (&bones, &primitive.influences) {
                    (Some(bones), Some(inf)) => skin::skin_mesh(
                        &primitive.mesh,
                        &inf.joints,
                        &inf.weights,
                        bones,
                        Mat4::IDENTITY,
                        transform.inverse(),
                    )
                    .map_err(|e| {
                        unsupported(
                            format!("glTF meshes[{mesh}].primitives[{p}]"),
                            e.to_string(),
                        )
                    })?,
                    _ => primitive.mesh.clone(),
                };
                out.push(MeshInstance {
                    mesh: data,
                    transform: transform.to_cols_array(),
                    material: primitive.material.clone(),
                    cast_shadow: true,
                    receive_shadow: true,
                    id: None,
                    glow: None,
                });
            }
        }
        Ok(out)
    }

    fn bounds(&self, instances: &[MeshInstance]) -> Option<(Vec3, Vec3)> {
        let mut acc: Option<(Vec3, Vec3)> = None;
        for instance in instances {
            let mut points = instance.mesh.positions.iter().map(|p| Vec3::from(*p));
            let Some(first) = points.next() else { continue };
            let (lo, hi) = points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
            let m = Mat4::from_cols_array(&instance.transform);
            for i in 0..8 {
                let corner = Vec3::new(
                    if i & 1 == 0 { lo.x } else { hi.x },
                    if i & 2 == 0 { lo.y } else { hi.y },
                    if i & 4 == 0 { lo.z } else { hi.z },
                );
                let c = m.transform_point3(corner);
                acc = Some(match acc {
                    None => (c, c),
                    Some((a, b)) => (a.min(c), b.max(c)),
                });
            }
        }
        acc
    }

    /// The model posed with `clip` after it has played `elapsed` seconds, in model space (the transform the
    /// caller's group applies comes on top). `None` is the rest pose.
    pub fn pose(&self, clip: Option<usize>, elapsed: f32) -> Result<Vec<MeshInstance>, GltfError> {
        let pose = self.animated_pose(clip, elapsed);
        self.instances(&self.world(&pose))
    }
}

fn read_primitive(
    primitive: &::gltf::Primitive,
    field: &str,
    skinned: bool,
    blob: Option<&[u8]>,
    image_keys: &HashMap<usize, String>,
) -> Result<Primitive, GltfError> {
    use ::gltf::mesh::Mode;
    if primitive.morph_targets().next().is_some() {
        return Err(unsupported(
            format!("{field}.targets"),
            "morph targets are not yet drawn by orbital-canvas",
        ));
    }
    if primitive.get(&::gltf::Semantic::Colors(0)).is_some() {
        return Err(unsupported(
            format!("{field}.attributes.COLOR_0"),
            "vertex colors are not yet drawn by orbital-canvas",
        ));
    }
    let reader = primitive.reader(move |buffer: ::gltf::Buffer| match buffer.source() {
        ::gltf::buffer::Source::Bin => blob,
        ::gltf::buffer::Source::Uri(_) => None,
    });
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| GltfError::Invalid(format!("{field} has no POSITION")))?
        .collect();
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|n| n.collect())
        .unwrap_or_default();
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().collect())
        .unwrap_or_default();
    let list: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    let indices = match primitive.mode() {
        Mode::Triangles => list,
        Mode::TriangleStrip => {
            let mut out = Vec::new();
            for i in 0..list.len().saturating_sub(2) {
                if i % 2 == 0 {
                    out.extend([list[i], list[i + 1], list[i + 2]]);
                } else {
                    out.extend([list[i + 2], list[i + 1], list[i]]);
                }
            }
            out
        }
        Mode::TriangleFan => {
            let mut out = Vec::new();
            for i in 1..list.len().saturating_sub(1) {
                out.extend([list[0], list[i], list[i + 1]]);
            }
            out
        }
        other => {
            return Err(unsupported(
                format!("{field}.mode"),
                format!("{other:?} primitives are not yet drawn by orbital-canvas"),
            ))
        }
    };
    if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= positions.len()) {
        return Err(GltfError::Invalid(format!(
            "{field} has indices outside its vertices"
        )));
    }
    let influences = if skinned {
        let joints: Vec<[u16; 4]> = reader
            .read_joints(0)
            .ok_or_else(|| {
                unsupported(
                    format!("{field}.attributes.JOINTS_0"),
                    "a skinned mesh needs it (three binds none)",
                )
            })?
            .into_u16()
            .collect();
        let mut weights: Vec<[f32; 4]> = reader
            .read_weights(0)
            .ok_or_else(|| {
                unsupported(
                    format!("{field}.attributes.WEIGHTS_0"),
                    "a skinned mesh needs it (three binds none)",
                )
            })?
            .into_f32()
            .collect();
        skin::normalize_skin_weights(&mut weights);
        Some(Influences { joints, weights })
    } else {
        None
    };
    let mut material = read_material(&primitive.material(), field, image_keys)?;
    material.flat_shading = normals.is_empty();
    Ok(Primitive {
        mesh: MeshData {
            positions,
            normals,
            uvs,
            indices,
        },
        material,
        influences,
    })
}

/// `GLTFLoader.loadMaterial` into a `MeshStandardMaterial`: factors are linear, the base color texture is sRGB.
fn read_material(
    m: &::gltf::Material,
    primitive: &str,
    image_keys: &HashMap<usize, String>,
) -> Result<Material, GltfError> {
    let field = match m.index() {
        Some(i) => format!("glTF materials[{i}]"),
        None => format!("{primitive}.material"),
    };
    let pbr = m.pbr_metallic_roughness();
    let undrawn =
        |key: &str| unsupported(format!("{field}.{key}"), "not yet drawn by orbital-canvas");
    if pbr.metallic_roughness_texture().is_some() {
        return Err(undrawn("pbrMetallicRoughness.metallicRoughnessTexture"));
    }
    if m.normal_texture().is_some() {
        return Err(undrawn("normalTexture"));
    }
    if m.occlusion_texture().is_some() {
        return Err(undrawn("occlusionTexture"));
    }
    if m.emissive_texture().is_some() {
        return Err(undrawn("emissiveTexture"));
    }
    match m.alpha_mode() {
        ::gltf::material::AlphaMode::Opaque => {}
        ::gltf::material::AlphaMode::Mask => {
            return Err(undrawn("alphaMode MASK (three's alphaTest)"))
        }
        ::gltf::material::AlphaMode::Blend => {
            return Err(undrawn(
                "alphaMode BLEND (three's transparent pass without depth writes)",
            ))
        }
    }
    let map = match pbr.base_color_texture() {
        None => None,
        Some(info) => {
            let tex_field = format!("{field}.pbrMetallicRoughness.baseColorTexture");
            if info.tex_coord() != 0 {
                return Err(unsupported(
                    format!("{tex_field}.texCoord"),
                    "only TEXCOORD_0 is drawn",
                ));
            }
            let texture = info.texture();
            let sampler = texture.sampler();
            for (axis, wrap) in [("wrapS", sampler.wrap_s()), ("wrapT", sampler.wrap_t())] {
                if wrap != ::gltf::texture::WrappingMode::Repeat {
                    return Err(unsupported(
                        format!("{tex_field} sampler {axis}"),
                        format!("{wrap:?} is not yet drawn by orbital-canvas (maps repeat)"),
                    ));
                }
            }
            if sampler.mag_filter() == Some(::gltf::texture::MagFilter::Nearest) {
                return Err(unsupported(
                    format!("{tex_field} sampler magFilter"),
                    "NEAREST is not yet drawn by orbital-canvas (maps filter linearly)",
                ));
            }
            if let Some(
                f @ (::gltf::texture::MinFilter::Nearest
                | ::gltf::texture::MinFilter::NearestMipmapNearest
                | ::gltf::texture::MinFilter::NearestMipmapLinear),
            ) = sampler.min_filter()
            {
                return Err(unsupported(
                    format!("{tex_field} sampler minFilter"),
                    format!("{f:?} is not yet drawn by orbital-canvas (maps filter linearly)"),
                ));
            }
            image_keys
                .get(&texture.source().index())
                .filter(|key| !key.is_empty())
                .cloned()
        }
    };
    let [r, g, b, _] = pbr.base_color_factor();
    Ok(Material {
        kind: MaterialKind::Standard,
        color: [r, g, b],
        opacity: 1.0,
        metalness: pbr.metallic_factor(),
        roughness: pbr.roughness_factor(),
        emissive: m.emissive_factor(),
        emissive_intensity: 1.0,
        transmission: 0.0,
        ior: 1.5,
        flat_shading: false,
        side: if m.double_sided() {
            Side::Double
        } else {
            Side::Front
        },
        map,
        shader: None,
    })
}

/// Where a model URL stands in the load.
#[derive(Debug, Clone)]
pub enum ModelState {
    /// Fetching: three draws its loading ring.
    Loading,
    /// The fetch or the parse failed: three draws its error fallback.
    Failed(String),
    Ready(Arc<Model>),
}

type Parsed = Result<Arc<Model>, GltfError>;

static MODELS: OnceLock<RwLock<HashMap<String, Parsed>>> = OnceLock::new();

/// The model at `url`, fetched through [`crate::asset`] and parsed once; a file three would draw that this module
/// does not is an error, so it is never mistaken for a broken file.
pub fn model_state(url: &str) -> Result<ModelState, GltfError> {
    let cache = MODELS.get_or_init(Default::default);
    if let Some(parsed) = cache.read().expect("model cache poisoned").get(url) {
        return state_of(parsed);
    }
    crate::asset::ensure_asset(url);
    if crate::asset::fetch_failed(url) {
        return Ok(ModelState::Failed(format!("fetch {url} failed")));
    }
    let Some(bytes) = crate::asset::asset_bytes(url) else {
        return Ok(ModelState::Loading);
    };
    let parsed = Model::from_glb(&bytes, url).map(Arc::new);
    if let Ok(model) = &parsed {
        model.register_images();
    }
    let state = state_of(&parsed);
    cache
        .write()
        .expect("model cache poisoned")
        .insert(url.to_string(), parsed);
    state
}

fn state_of(parsed: &Parsed) -> Result<ModelState, GltfError> {
    match parsed {
        Ok(model) => Ok(ModelState::Ready(model.clone())),
        Err(GltfError::Invalid(why)) => Ok(ModelState::Failed(why.clone())),
        Err(e) => Err(e.clone()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    fn close(a: [f32; 3], b: [f32; 3]) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-4, "{a:?} != {b:?}");
        }
    }

    fn f32s(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn pad(bytes: &mut Vec<u8>, with: u8) {
        while bytes.len() % 4 != 0 {
            bytes.push(with);
        }
    }

    /// GLB container: header, JSON chunk (space padded), BIN chunk (zero padded).
    pub(crate) fn glb(json: &serde_json::Value, bin: &[u8]) -> Vec<u8> {
        let mut json = serde_json::to_vec(json).unwrap();
        pad(&mut json, b' ');
        let mut bin = bin.to_vec();
        pad(&mut bin, 0);
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(0x4E4F_534Au32.to_le_bytes());
        out.extend(&json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(0x004E_4942u32.to_le_bytes());
        out.extend(&bin);
        out
    }

    fn png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// A triangle skinned to two joints (joint 1 at x = 1), a "Walk" clip turning joint 1 a quarter turn about Z
    /// over one second, and an embedded base color texture. `material` overrides the material.
    pub(crate) fn rig(material: serde_json::Value) -> Vec<u8> {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let mut bin = Vec::new();
        bin.extend(f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]));
        bin.extend(f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
        for j in [[0u16, 0, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0]] {
            bin.extend(j.iter().flat_map(|v| v.to_le_bytes()));
        }
        bin.extend(f32s(&[
            1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0,
        ]));
        bin.extend([0u16, 1, 2].iter().flat_map(|v| v.to_le_bytes()));
        bin.extend([0, 0]);
        bin.extend(f32s(&Mat4::IDENTITY.to_cols_array()));
        bin.extend(f32s(
            &Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0)).to_cols_array(),
        ));
        bin.extend(f32s(&[0.0, 1.0]));
        bin.extend(f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s, s]));
        assert_eq!(bin.len(), 320);
        let image = png();
        bin.extend(&image);
        let view = |offset: usize, length: usize| json!({"buffer": 0, "byteOffset": offset, "byteLength": length});
        let json = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0, 1]}],
            "nodes": [
                {"mesh": 0, "skin": 0},
                {"children": [2]},
                {"translation": [1.0, 0.0, 0.0]}
            ],
            "meshes": [{"primitives": [{
                "attributes": {"POSITION": 0, "NORMAL": 1, "JOINTS_0": 2, "WEIGHTS_0": 3, "TEXCOORD_0": 8},
                "indices": 4,
                "material": 0
            }]}],
            "skins": [{"joints": [1, 2], "inverseBindMatrices": 5}],
            "animations": [{"name": "Walk", "channels": [{"sampler": 0, "target": {"node": 2, "path": "rotation"}}],
                "samplers": [{"input": 6, "output": 7, "interpolation": "LINEAR"}]}],
            "materials": [material],
            "textures": [{"source": 0}],
            "images": [{"bufferView": 8, "mimeType": "image/png"}],
            "buffers": [{"byteLength": bin.len() + (4 - bin.len() % 4) % 4}],
            "bufferViews": [view(0, 36), view(36, 36), view(72, 24), view(96, 48), view(144, 6), view(152, 128),
                view(280, 8), view(288, 32), view(320, image.len())],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 2, "componentType": 5123, "count": 3, "type": "VEC4"},
                {"bufferView": 3, "componentType": 5126, "count": 3, "type": "VEC4"},
                {"bufferView": 4, "componentType": 5123, "count": 3, "type": "SCALAR"},
                {"bufferView": 5, "componentType": 5126, "count": 2, "type": "MAT4"},
                {"bufferView": 6, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0], "max": [1]},
                {"bufferView": 7, "componentType": 5126, "count": 2, "type": "VEC4"},
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC2"}
            ]
        });
        glb(&json, &bin)
    }

    pub(crate) fn rig_material() -> serde_json::Value {
        json!({"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.5, 0.25, 1.0], "metallicFactor": 0.2,
            "roughnessFactor": 0.7, "baseColorTexture": {"index": 0}}})
    }

    #[test]
    fn a_glb_loads_its_mesh_material_skin_clip_and_embedded_image() {
        let model =
            Model::from_glb(&rig(rig_material()), "https://cdn.test/3d/units/knight.glb").unwrap();
        assert_eq!(model.clips.len(), 1);
        assert_eq!(
            (model.clips[0].name.as_str(), model.clips[0].duration),
            ("Walk", 1.0)
        );
        assert_eq!(model.clip_index("Walk"), Some(0));
        assert_eq!(
            model.clip_index("walk"),
            Some(0),
            "case-insensitive fallback"
        );
        assert_eq!(
            model.clip_index("run"),
            None,
            "control: an unknown clip plays nothing"
        );
        assert_eq!(model.images.len(), 1);
        assert_eq!(
            model.images[0].key,
            "https://cdn.test/3d/units/knight.glb#gltf-image-0"
        );
        assert_eq!(model.images[0].mime, "image/png");

        let rest = model.pose(None, 0.0).unwrap();
        assert_eq!(rest.len(), 1);
        let m = &rest[0].material;
        assert_eq!(m.color, [1.0, 0.5, 0.25]);
        assert_eq!((m.metalness, m.roughness, m.opacity), (0.2, 0.7, 1.0));
        assert_eq!(
            m.map.as_deref(),
            Some("https://cdn.test/3d/units/knight.glb#gltf-image-0")
        );
        assert!(!m.flat_shading);
        close(rest[0].mesh.positions[2], [0.0, 1.0, 0.0]);
        assert!(rest[0].cast_shadow && rest[0].receive_shadow);
        let (lo, hi) = model.rest_bounds().unwrap();
        close(lo.to_array(), [0.0, 0.0, 0.0]);
        close(hi.to_array(), [1.0, 1.0, 0.0]);
    }

    #[test]
    fn posing_plays_the_clip_through_the_skin() {
        let model = Model::from_glb(&rig(rig_material()), "m.glb").unwrap();
        let posed = model.pose(Some(0), 0.5).unwrap();
        let p = &posed[0].mesh.positions;
        close(p[0], [0.0, 0.0, 0.0]);
        close(p[1], [1.0, 0.0, 0.0]);
        // (0,1,0) relative to joint 1 is (-1,1,0); turned 45 degrees it is (-sqrt2, 0, 0); back at x = 1
        close(p[2], [1.0 - std::f32::consts::SQRT_2, 0.0, 0.0]);
        let looped = model.pose(Some(0), 1.5).unwrap();
        close(looped[0].mesh.positions[2], p[2]);
        let control = model.pose(None, 0.5).unwrap();
        close(control[0].mesh.positions[2], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn the_clip_fades_in_from_the_rest_pose() {
        let model = Model::from_glb(&rig(rig_material()), "m.glb").unwrap();
        let start = model.pose(Some(0), 0.0).unwrap();
        close(start[0].mesh.positions[2], [0.0, 1.0, 0.0]);
        // at 0.05 s the clip is 4.5 degrees in and weighs a third: 1.5 degrees about joint 1
        let early = model.pose(Some(0), 0.05).unwrap();
        let a = 1.5f32.to_radians();
        let (s, c) = a.sin_cos();
        close(early[0].mesh.positions[2], [1.0 - c - s, -s + c, 0.0]);
    }

    #[test]
    fn urls_resolve_like_three() {
        assert_eq!(asset_root("https://h/3d/units/a.glb"), "https://h/3d/");
        assert_eq!(asset_root("https://h/models/a.glb"), "https://h/models/");
        assert_eq!(asset_root("a.glb"), "");
        assert_eq!(
            resolve_url("Textures/c.png", "https://h/3d/"),
            "https://h/3d/Textures/c.png"
        );
        assert_eq!(resolve_url("/c.png", "https://h/3d/"), "https://h/c.png");
        assert_eq!(resolve_url("//x/c.png", "https://h/3d/"), "//x/c.png");
        assert_eq!(
            resolve_url("data:image/png;base64,AA", "p/"),
            "data:image/png;base64,AA"
        );
        assert_eq!(resolve_url("", "p/"), "");
    }

    fn channel(
        property: Property,
        interpolation: Interpolation,
        times: &[f32],
        values: &[f32],
    ) -> Channel {
        Channel {
            node: 0,
            property,
            interpolation,
            times: times.to_vec(),
            values: values.to_vec(),
        }
    }

    #[test]
    fn interpolation_matches_the_mixer() {
        let linear = channel(
            Property::Translation,
            Interpolation::Linear,
            &[0.0, 2.0],
            &[0.0, 0.0, 0.0, 4.0, 2.0, 0.0],
        );
        assert_eq!(linear.sample(0.5), vec![1.0, 0.5, 0.0]);
        assert_eq!(
            linear.sample(-1.0),
            vec![0.0, 0.0, 0.0],
            "before the first key"
        );
        assert_eq!(
            linear.sample(9.0),
            vec![4.0, 2.0, 0.0],
            "after the last key"
        );
        let step = channel(
            Property::Scale,
            Interpolation::Step,
            &[0.0, 1.0],
            &[1.0, 1.0, 1.0, 2.0, 2.0, 2.0],
        );
        assert_eq!(step.sample(0.99), vec![1.0, 1.0, 1.0]);
        assert_eq!(step.sample(1.0), vec![2.0, 2.0, 2.0]);
        // keys [in, value, out]: value 0 -> 1, out-tangent 2 at the first key, in-tangent 0 at the second
        let cubic = channel(
            Property::Translation,
            Interpolation::CubicSpline,
            &[0.0, 1.0],
            &[
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
                0.0, 0.0,
            ],
        );
        let v = cubic.sample(0.5);
        assert!((v[0] - 0.75).abs() < 1e-6, "{v:?}");
        assert_eq!(cubic.sample(1.0), vec![1.0, 0.0, 0.0]);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let rot = channel(
            Property::Rotation,
            Interpolation::Linear,
            &[0.0, 1.0],
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s, s],
        );
        let q = rot.sample(0.5);
        let half = std::f32::consts::FRAC_PI_8;
        assert!(
            (q[2] - half.sin()).abs() < 1e-6 && (q[3] - half.cos()).abs() < 1e-6,
            "{q:?}"
        );
    }

    #[test]
    fn clip_time_loops_like_loop_repeat() {
        let clip = Clip {
            name: "a".into(),
            duration: 1.0,
            channels: vec![],
        };
        assert_eq!(clip.time(0.25), 0.25);
        assert_eq!(clip.time(1.25), 0.25);
        assert_eq!(clip.time(1.0), 0.0);
    }

    #[test]
    fn control_undrawn_material_features_error_by_name_and_garbage_is_invalid() {
        let masked = Model::from_glb(&rig(json!({"alphaMode": "MASK"})), "m.glb").unwrap_err();
        assert_eq!(
            masked,
            GltfError::Unsupported {
                field: "glTF materials[0].alphaMode MASK (three's alphaTest)".into(),
                why: "not yet drawn by orbital-canvas".into()
            }
        );
        let plain = Model::from_glb(&rig(json!({})), "m.glb").unwrap();
        let m = &plain.pose(None, 0.0).unwrap()[0].material;
        assert_eq!(
            (m.color, m.metalness, m.roughness, m.map.clone()),
            ([1.0; 3], 1.0, 1.0, None)
        );
        assert!(matches!(
            Model::from_glb(b"not a glb", "m.glb"),
            Err(GltfError::Invalid(_))
        ));
    }
}
