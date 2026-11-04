//! The 2D puppet-rig projection shared by native canvas hosts.

use crate::asset;
use crate::packet::{DrawCommand, Vec2};
use crate::projector::{Projector, ScenePos};
use crate::skinning_2d::{self, BoneDef as Bone, Pose, SkinError, SkinMesh as Mesh};
#[cfg(test)]
use crate::skinning_2d::{SkinVertex as Vertex, SkinWeight as Weight};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkinMeshError {
    #[error(transparent)]
    Skin(#[from] SkinError),
    #[error("draw-skinned-mesh is missing a valid position")]
    Position,
    #[error("draw-skinned-mesh is missing asset.url")]
    Texture,
    #[error("draw-skinned-mesh texture failed to load: {0}")]
    TextureFailed(String),
    #[error("draw-skinned-mesh requires mesh or meshUrl")]
    Mesh,
    #[error("draw-skinned-mesh mesh failed to load: {0}")]
    MeshFailed(String),
    #[error("draw-skinned-mesh mesh is malformed: {0}")]
    InvalidMesh(String),
    #[error("draw-skinned-mesh requires bones or rigbundle bones")]
    Bones,
}

#[derive(Debug, Clone, Deserialize)]
struct Clip {
    frames: Vec<Pose>,
}

#[derive(Debug, Clone, Deserialize)]
struct RigBundle {
    mesh: Option<Mesh>,
    #[serde(default)]
    bones: Vec<Bone>,
    #[serde(default)]
    clips: HashMap<String, Clip>,
}

fn parse<T: for<'de> Deserialize<'de>>(value: &Value) -> Result<T, SkinMeshError> {
    serde_json::from_value(value.clone()).map_err(|err| SkinMeshError::InvalidMesh(err.to_string()))
}

fn frame_pose<'a>(clip: &'a Clip, frame: Option<u64>) -> Option<&'a Pose> {
    frame.and_then(|frame| {
        (!clip.frames.is_empty()).then(|| &clip.frames[frame as usize % clip.frames.len()])
    })
}

fn hue_color(bone: usize) -> [f32; 4] {
    let hue = (bone % 10) as f32 / 10.0;
    let chroma = 0.7 * (1.0 - (2.0 * 0.55 - 1.0f32).abs());
    let x = chroma * (1.0 - ((hue * 6.0) % 2.0 - 1.0).abs());
    let m = 0.55 - chroma / 2.0;
    let rgb = match (hue * 6.0) as usize {
        0 => [chroma, x, 0.0],
        1 => [x, chroma, 0.0],
        2 => [0.0, chroma, x],
        3 => [0.0, x, chroma],
        4 => [x, 0.0, chroma],
        _ => [chroma, 0.0, x],
    };
    [rgb[0] + m, rgb[1] + m, rgb[2] + m, 1.0]
}

fn emit_pass(
    mesh: &Mesh,
    bones: &[Bone],
    pose: &Pose,
    origin: (f32, f32),
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    texture: &str,
    pixel_uv: bool,
    overlay: bool,
    opacity: f32,
    out: &mut Vec<DrawCommand>,
) -> Result<(), SkinError> {
    let bind = skinning_2d::compute_world_matrices(bones, &Pose::new())?;
    let current = skinning_2d::compute_world_matrices(bones, pose)?;
    let points = skinning_2d::skin_vertices(mesh, &bind, &current)?;
    for (index, triangle) in mesh.triangles.iter().enumerate() {
        let missing = |vertex: usize| SkinError::TriangleIndex {
            triangle: index,
            index: vertex,
        };
        let vertices = triangle
            .iter()
            .map(|i| mesh.vertices.get(*i).ok_or_else(|| missing(*i)))
            .collect::<Result<Vec<_>, _>>()?;
        let skinned = triangle
            .iter()
            .map(|i| points.get(*i).ok_or_else(|| missing(*i)))
            .collect::<Result<Vec<_>, _>>()?;
        let points: [Vec2; 3] = std::array::from_fn(|i| {
            let (x, y) = to_screen(
                origin.0 + skinned[i][0] as f32 * projector.tile_width,
                origin.1 + skinned[i][1] as f32 * projector.tile_width,
            );
            Vec2 { x, y }
        });
        if overlay {
            let bone = vertices[0]
                .weights
                .iter()
                .filter(|weight| weight.w > 0.0)
                .max_by(|a, b| a.w.total_cmp(&b.w))
                .map(|weight| weight.bone)
                .ok_or(SkinError::NoPositiveWeight {
                    vertex: triangle[0],
                })?;
            let mut color = hue_color(bone);
            color[3] = opacity;
            out.push(DrawCommand::Poly {
                points: points.to_vec(),
                color,
            });
        } else {
            let uv = std::array::from_fn(|i| [vertices[i].u as f32, vertices[i].v as f32]);
            out.push(DrawCommand::TexturedTriangle {
                texture: texture.to_string(),
                points,
                uv,
                uv_in_pixels: pixel_uv,
                opacity,
            });
        }
    }
    Ok(())
}

/// Project one authored puppet rig. `Ok(None)` means its mesh or texture is still loading.
pub fn paint(
    node: &Value,
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
) -> Result<Option<Vec<DrawCommand>>, SkinMeshError> {
    let position = node
        .get("position")
        .and_then(|value| {
            Some(ScenePos {
                x: value.get("x")?.as_f64()? as f32,
                y: value.get("y")?.as_f64()? as f32,
            })
        })
        .ok_or(SkinMeshError::Position)?;
    let texture = node
        .get("asset")
        .and_then(|asset| asset.get("url"))
        .and_then(Value::as_str)
        .ok_or(SkinMeshError::Texture)?;
    let overlay = node
        .get("weightsOverlay")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !overlay {
        if asset::image_failed(texture) {
            return Err(SkinMeshError::TextureFailed(texture.to_string()));
        }
        if asset::image_info(texture).is_none() {
            return Ok(None);
        }
    }

    let mesh_url = node.get("meshUrl").and_then(Value::as_str);
    let fetched = if let Some(url) = mesh_url {
        asset::ensure_asset(url);
        if asset::fetch_failed(url) {
            return Err(SkinMeshError::MeshFailed(url.to_string()));
        }
        let Some(bytes) = asset::asset_bytes(url) else {
            return Ok(None);
        };
        Some(
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|err| SkinMeshError::InvalidMesh(err.to_string()))?,
        )
    } else {
        None
    };
    let bundle = fetched
        .as_ref()
        .filter(|value| value.get("mesh").is_some() || value.get("bones").is_some())
        .map(parse::<RigBundle>)
        .transpose()?;
    let mesh_value = node
        .get("mesh")
        .or_else(|| {
            bundle
                .as_ref()
                .and_then(|b| b.mesh.as_ref())
                .map(|_| fetched.as_ref().unwrap().get("mesh").unwrap())
        })
        .or_else(|| fetched.as_ref().filter(|_| bundle.is_none()))
        .ok_or(SkinMeshError::Mesh)?;
    let mesh: Mesh = parse(mesh_value)?;
    let bones: Vec<Bone> = match node.get("bones").and_then(Value::as_array) {
        Some(items) if !items.is_empty() => parse(&Value::Array(items.clone()))?,
        _ => bundle.as_ref().map(|b| b.bones.clone()).unwrap_or_default(),
    };
    if bones.is_empty() {
        return Err(SkinMeshError::Bones);
    }

    let authored_clip = node.get("clip").map(parse::<Clip>).transpose()?;
    let clip = authored_clip.as_ref().or_else(|| {
        node.get("clipName")
            .and_then(Value::as_str)
            .and_then(|name| bundle.as_ref()?.clips.get(name))
    });
    let pose = node
        .get("pose")
        .map(parse::<Pose>)
        .transpose()?
        .unwrap_or_default();
    let frame = node.get("frame").and_then(Value::as_u64);
    let live_pose = clip
        .and_then(|clip| frame_pose(clip, frame))
        .unwrap_or(&pose);
    let origin = projector.project(position);
    let pixel_uv = mesh
        .vertices
        .iter()
        .any(|vertex| vertex.u < 0.0 || vertex.u > 1.0 || vertex.v < 0.0 || vertex.v > 1.0);
    let mut out = Vec::new();
    if let (Some(ghost), Some(clip)) = (node.get("ghost"), clip) {
        if let Some(ghost_pose) = frame_pose(clip, ghost.get("frame").and_then(Value::as_u64)) {
            let opacity = ghost.get("opacity").and_then(Value::as_f64).unwrap_or(1.0) as f32;
            emit_pass(
                &mesh, &bones, ghost_pose, origin, projector, to_screen, texture, pixel_uv,
                overlay, opacity, &mut out,
            )?;
        }
    }
    let opacity = node.get("opacity").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    emit_pass(
        &mesh, &bones, live_pose, origin, projector, to_screen, texture, pixel_uv, overlay,
        opacity, &mut out,
    )?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projector::Projection;
    use serde_json::json;

    #[test]
    fn authored_pose_and_ghost_project_two_textured_triangles() {
        let node = json!({
            "position": {"x": 0, "y": 0}, "asset": {"url": "unused"},
            "mesh": {"vertices": [
                {"x":0,"y":0,"u":0,"v":0,"weights":[{"bone":0,"w":1}]},
                {"x":1,"y":0,"u":1,"v":0,"weights":[{"bone":0,"w":1}]},
                {"x":0,"y":1,"u":0,"v":1,"weights":[{"bone":0,"w":1}]}
            ], "triangles": [[0,1,2]]},
            "bones": [{"name":"root","pivot":[0,0],"tip":[1,0]}],
            "clip": {"frames":[{"root":0},{"root":90}]},
            "frame":1, "ghost":{"frame":0,"opacity":0.25},
            "weightsOverlay":true
        });
        let projector = Projector::new(1.0, 0.0, Projection::Flat);
        let commands = paint(&node, &projector, &|x, y| (x, y)).unwrap().unwrap();
        assert_eq!(commands.len(), 2);
        let DrawCommand::Poly { points, color } = &commands[0] else {
            panic!("ghost polygon")
        };
        assert_eq!(color[3], 0.25);
        assert!((points[1].x - projector.tile_width).abs() < 1e-5);
        let DrawCommand::Poly { points, color } = &commands[1] else {
            panic!("live polygon")
        };
        assert_eq!(color[3], 1.0);
        assert!(points[1].x.abs() < 1e-4);
        assert!((points[1].y - projector.tile_width).abs() < 1e-4);
    }

    #[test]
    fn malformed_inline_mesh_has_exact_error() {
        let node = json!({"position":{"x":0,"y":0},"asset":{"url":"unused"},
            "weightsOverlay":true,"mesh":{"vertices":[]},"bones":[{"name":"root","pivot":[0,0],"tip":[0,0]}]});
        let projector = Projector::new(1.0, 0.0, Projection::Flat);
        assert!(matches!(
            paint(&node, &projector, &|x, y| (x, y)),
            Err(SkinMeshError::InvalidMesh(_))
        ));
    }

    #[test]
    fn child_bone_rotation_and_pixel_uv_reach_triangle_packet() {
        let mesh = Mesh {
            vertices: vec![
                Vertex {
                    x: 2.0,
                    y: 0.0,
                    u: 0.0,
                    v: 0.0,
                    weights: vec![Weight { bone: 1, w: 1.0 }],
                },
                Vertex {
                    x: 3.0,
                    y: 0.0,
                    u: 64.0,
                    v: 0.0,
                    weights: vec![Weight { bone: 1, w: 1.0 }],
                },
                Vertex {
                    x: 2.0,
                    y: 1.0,
                    u: 0.0,
                    v: 64.0,
                    weights: vec![Weight { bone: 1, w: 1.0 }],
                },
            ],
            triangles: vec![[0, 1, 2]],
        };
        let bones = vec![
            Bone {
                name: "root".into(),
                parent: None,
                pivot: [0.0, 0.0],
                tip: [1.0, 0.0],
            },
            Bone {
                name: "child".into(),
                parent: Some("root".into()),
                pivot: [1.0, 0.0],
                tip: [2.0, 0.0],
            },
        ];
        let pose = Pose::from([("root".into(), 90.0), ("child".into(), 0.0)]);
        let projector = Projector::new(1.0, 0.0, Projection::Flat);
        let mut commands = Vec::new();
        emit_pass(
            &mesh,
            &bones,
            &pose,
            (0.0, 0.0),
            &projector,
            &|x, y| (x, y),
            "texture.png",
            true,
            false,
            0.4,
            &mut commands,
        )
        .unwrap();
        let [DrawCommand::TexturedTriangle {
            texture,
            points,
            uv,
            uv_in_pixels,
            opacity,
        }] = commands.as_slice()
        else {
            panic!("one textured triangle")
        };
        assert_eq!(texture, "texture.png");
        assert_eq!(*uv, [[0.0, 0.0], [64.0, 0.0], [0.0, 64.0]]);
        assert!(*uv_in_pixels);
        assert_eq!(*opacity, 0.4);
        assert!(points[0].x.abs() < 1e-4);
        assert!((points[0].y - 2.0 * projector.tile_width).abs() < 1e-4);
    }

    #[test]
    fn missing_rig_source_reports_exact_error() {
        let node = json!({"position":{"x":0,"y":0},"asset":{"url":"unused"},
            "weightsOverlay":true,"bones":[{"name":"root","pivot":[0,0],"tip":[0,0]}]});
        let projector = Projector::new(1.0, 0.0, Projection::Flat);
        assert!(matches!(
            paint(&node, &projector, &|x, y| (x, y)),
            Err(SkinMeshError::Mesh)
        ));
    }

    fn one_triangle(weights: Value, triangle: Value) -> Value {
        json!({
            "position": {"x": 0, "y": 0}, "asset": {"url": "unused"},
            "weightsOverlay": true,
            "mesh": {"vertices": [
                {"x":0,"y":0,"u":0,"v":0,"weights":weights},
                {"x":1,"y":0,"u":1,"v":0,"weights":[{"bone":0,"w":1}]},
                {"x":0,"y":1,"u":0,"v":1,"weights":[{"bone":0,"w":1}]}
            ], "triangles": [triangle]},
            "bones": [{"name":"root","pivot":[0,0],"tip":[1,0]}]
        })
    }

    #[test]
    fn out_of_range_triangle_and_weightless_overlay_vertex_are_errors() {
        let projector = Projector::new(1.0, 0.0, Projection::Flat);
        let run = |node: Value| paint(&node, &projector, &|x, y| (x, y));
        assert!(run(one_triangle(json!([{"bone":0,"w":1}]), json!([0, 1, 2]))).is_ok());
        assert_eq!(
            run(one_triangle(json!([{"bone":0,"w":1}]), json!([0, 1, 9]))).err(),
            Some(SkinMeshError::Skin(SkinError::TriangleIndex {
                triangle: 0,
                index: 9
            }))
        );
        assert_eq!(
            run(one_triangle(json!([]), json!([0, 1, 2]))).err(),
            Some(SkinMeshError::Skin(SkinError::NoPositiveWeight {
                vertex: 0
            }))
        );
        assert_eq!(
            run(one_triangle(json!([{"bone":5,"w":1}]), json!([0, 1, 2]))).err(),
            Some(SkinMeshError::Skin(SkinError::BoneIndex {
                vertex: 0,
                bone: 5
            }))
        );
    }
}
