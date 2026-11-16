//! What can go wrong opening a device or drawing a frame.

use thiserror::Error;

/// Errors that can occur when creating or using the renderer.
#[derive(Debug, Error)]
pub enum RendererError {
    #[error("no suitable graphics adapter found")]
    NoAdapter,
    #[error("failed to request device: {0}")]
    DeviceRequestFailed(String),
    #[error("surface error: {0}")]
    Surface(String),
    #[error("invalid packet: {0}")]
    InvalidPacket(String),
}

/// Result type alias for renderer operations.
pub type Result<T> = std::result::Result<T, RendererError>;
