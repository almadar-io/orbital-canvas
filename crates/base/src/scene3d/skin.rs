//! Linear-blend skinning on the CPU, exactly as three.js's `SkinnedMesh` shader chunks compute it (`skinbase`,
//! `skinnormal`, `skinning`): the posed mesh comes out in the mesh's own object space, so the renderer draws it with
//! the mesh's world transform like any other mesh and needs no skinning pipeline (it works the same on WebGL2).

use super::model::MeshData;
use glam::{Mat4, Vec4};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkinError {
    #[error("{entries} skin {what} for {vertices} vertices")]
    Count {
        what: &'static str,
        entries: usize,
        vertices: usize,
    },
    #[error("vertex {vertex} weights joint {joint}, past the {joints} bones")]
    Joint {
        vertex: usize,
        joint: usize,
        joints: usize,
    },
}

/// three's `Skeleton.update`: each bone matrix is the bone's world matrix times its inverse bind matrix.
pub fn bone_matrices(bone_world: &[Mat4], inverse_bind: &[Mat4]) -> Vec<Mat4> {
    bone_world
        .iter()
        .zip(inverse_bind)
        .map(|(world, inverse)| *world * *inverse)
        .collect()
}

/// three's `SkinnedMesh.normalizeSkinWeights`: weights scale to a unit Manhattan length; an all-zero vertex binds
/// fully to its first joint.
pub fn normalize_skin_weights(weights: &mut [[f32; 4]]) {
    for w in weights {
        let scale = 1.0 / (w[0].abs() + w[1].abs() + w[2].abs() + w[3].abs());
        *w = if scale.is_finite() {
            w.map(|c| c * scale)
        } else {
            [1.0, 0.0, 0.0, 0.0]
        };
    }
}

/// Pose `mesh` (bind-pose object space) by `bones` (three's `boneMatrices`). `bind` and `bind_inverse` are the
/// mesh's `bindMatrix` and `bindMatrixInverse`; in three's attached bind mode the inverse is the inverse of the
/// mesh's current world matrix. All four influences contribute, a zero weight included, as in the shader.
pub fn skin_mesh(
    mesh: &MeshData,
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    bones: &[Mat4],
    bind: Mat4,
    bind_inverse: Mat4,
) -> Result<MeshData, SkinError> {
    let vertices = mesh.positions.len();
    for (what, entries) in [("joints", joints.len()), ("weights", weights.len())] {
        if entries != vertices {
            return Err(SkinError::Count {
                what,
                entries,
                vertices,
            });
        }
    }
    let mut positions = Vec::with_capacity(vertices);
    let mut normals = Vec::with_capacity(mesh.normals.len());
    for (vertex, position) in mesh.positions.iter().enumerate() {
        let mut matrices = [Mat4::ZERO; 4];
        for k in 0..4 {
            let joint = joints[vertex][k] as usize;
            matrices[k] = match bones.get(joint) {
                Some(m) => *m,
                None if weights[vertex][k] == 0.0 => Mat4::ZERO,
                None => {
                    return Err(SkinError::Joint {
                        vertex,
                        joint,
                        joints: bones.len(),
                    })
                }
            };
        }
        let w = weights[vertex];
        let skin_vertex = bind * Vec4::new(position[0], position[1], position[2], 1.0);
        let mut skinned = Vec4::ZERO;
        for k in 0..4 {
            skinned += matrices[k] * skin_vertex * w[k];
        }
        positions.push((bind_inverse * skinned).truncate().to_array());
        if let Some(normal) = mesh.normals.get(vertex) {
            let mut skin_matrix = Mat4::ZERO;
            for k in 0..4 {
                skin_matrix += matrices[k] * w[k];
            }
            let skin_matrix = bind_inverse * skin_matrix * bind;
            let n = skin_matrix * Vec4::new(normal[0], normal[1], normal[2], 0.0);
            normals.push(n.truncate().to_array());
        }
    }
    Ok(MeshData {
        positions,
        normals,
        uvs: mesh.uvs.clone(),
        indices: mesh.indices.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};

    fn close(a: [f32; 3], b: [f32; 3]) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-5, "{a:?} != {b:?}");
        }
    }

    fn quad() -> MeshData {
        MeshData {
            positions: vec![[1.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
            normals: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 1.0]],
            indices: vec![],
        }
    }

    #[test]
    fn a_rotated_bone_moves_its_vertices_by_hand_computed_amounts() {
        // bone 0 at rest at (0, 1, 0), turned 90 degrees about Z and moved to (5, 1, 0)
        let rest = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
        let posed = Mat4::from_rotation_translation(
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Vec3::new(5.0, 1.0, 0.0),
        );
        let bones = bone_matrices(&[posed, Mat4::IDENTITY], &[rest.inverse(), Mat4::IDENTITY]);
        let out = skin_mesh(
            &quad(),
            &[[0, 0, 0, 0], [0, 1, 0, 0]],
            &[[1.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0]],
            &bones,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
        )
        .unwrap();
        // (1,0,0) - rest = (1,-1,0); rotated (1,1,0); + (5,1,0) = (6,2,0)
        close(out.positions[0], [6.0, 2.0, 0.0]);
        // (0,2,0): bone 0 gives (0,1,0)->rot(-1,0,0)+(5,1,0)=(4,1,0); bone 1 keeps (0,2,0); half each
        close(out.positions[1], [2.0, 1.5, 0.0]);
        close(out.normals[0], [0.0, 1.0, 0.0]);
        // (0.5 * rot + 0.5 * I) * (0,1,0) = (-0.5, 0.5, 0)
        close(out.normals[1], [-0.5, 0.5, 0.0]);
        assert_eq!(out.uvs, quad().uvs);
    }

    #[test]
    fn the_bind_inverse_takes_the_result_back_into_object_space() {
        let world = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let out = skin_mesh(
            &quad(),
            &[[0, 0, 0, 0]; 2],
            &[[1.0, 0.0, 0.0, 0.0]; 2],
            &[world],
            Mat4::IDENTITY,
            world.inverse(),
        )
        .unwrap();
        close(out.positions[0], [1.0, 0.0, 0.0]);
        let control = skin_mesh(
            &quad(),
            &[[0, 0, 0, 0]; 2],
            &[[1.0, 0.0, 0.0, 0.0]; 2],
            &[world],
            Mat4::IDENTITY,
            Mat4::IDENTITY,
        )
        .unwrap();
        close(control.positions[0], [11.0, 0.0, 0.0]);
    }

    #[test]
    fn weights_normalize_like_three_and_a_zero_vertex_binds_to_its_first_joint() {
        let mut w = [[2.0, 2.0, 0.0, 0.0], [0.0; 4], [0.25, 0.25, 0.25, 0.25]];
        normalize_skin_weights(&mut w);
        assert_eq!(
            w,
            [
                [0.5, 0.5, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
                [0.25, 0.25, 0.25, 0.25]
            ]
        );
    }

    #[test]
    fn control_an_unknown_joint_errors_only_when_it_carries_weight() {
        let bones = [Mat4::IDENTITY];
        let weightless = skin_mesh(
            &quad(),
            &[[0, 7, 0, 0]; 2],
            &[[1.0, 0.0, 0.0, 0.0]; 2],
            &bones,
            Mat4::IDENTITY,
            Mat4::IDENTITY,
        );
        assert!(weightless.is_ok());
        assert_eq!(
            skin_mesh(
                &quad(),
                &[[0, 7, 0, 0]; 2],
                &[[0.5, 0.5, 0.0, 0.0]; 2],
                &bones,
                Mat4::IDENTITY,
                Mat4::IDENTITY,
            ),
            Err(SkinError::Joint {
                vertex: 0,
                joint: 7,
                joints: 1
            })
        );
        assert_eq!(
            skin_mesh(
                &quad(),
                &[[0; 4]],
                &[[1.0, 0.0, 0.0, 0.0]; 2],
                &bones,
                Mat4::IDENTITY,
                Mat4::IDENTITY
            ),
            Err(SkinError::Count {
                what: "joints",
                entries: 1,
                vertices: 2
            })
        );
    }
}
