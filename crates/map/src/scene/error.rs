//! Why a scene could not be mapped.

/// Why a `canvas` render-ui node cannot become a 2D display-list packet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanvasSceneError {
    #[error("scene is not an object")]
    NotAnObject,
    #[error("canvas mode `{0}` has no native painter (only `2d`)")]
    UnsupportedMode(String),
    #[error("`{0}` has nothing the Rust renderer can draw")]
    NothingToDraw(String),
    #[error("3D canvas contract is unsupported: {0}")]
    Unsupported3DContract(String),
    #[error("`{field}`: {detail}")]
    InvalidProp { field: String, detail: String },
    #[error(transparent)]
    Fx(#[from] orbital_canvas_base::fx_projection::FxError),
    #[error(transparent)]
    Skin(#[from] orbital_canvas_base::skinning_2d::SkinError),
    #[error(transparent)]
    SkinnedMesh(crate::skinned_mesh_2d::SkinMeshError),
    #[error("`{pattern}` requires `{field}`")]
    MissingField { pattern: String, field: String },
    #[error("draw-sprite does not resolve: {0}")]
    UnresolvedSprite(String),
}
