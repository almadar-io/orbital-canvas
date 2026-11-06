//! Error scopes and waiting, shared by every operation that touches the device.

use super::{invalid, io_error};
use orbital_core_base::os::OsError;

/// wgpu 26 error scopes belong to the device, and every program shares the one slot device: without this, one
/// program's scope pops another's error.
static SCOPE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` with validation and out-of-memory scopes open: a wgpu error becomes the answer instead of the
/// device's panicking uncaptured-error handler.
pub(super) async fn scoped<T>(device: &wgpu::Device, f: impl FnOnce() -> T) -> Result<T, OsError> {
    let _scope = SCOPE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let out = f();
    let validation = device.pop_error_scope().await;
    let memory = device.pop_error_scope().await;
    if let Some(error) = validation {
        return Err(invalid(error.to_string()));
    }
    if let Some(error) = memory {
        return Err(io_error(error.to_string()));
    }
    Ok(out)
}

/// Let the device run its callbacks. The browser event loop does that by itself.
pub(super) fn drive(device: &wgpu::Device) -> Result<(), OsError> {
    #[cfg(not(target_arch = "wasm32"))]
    device
        .poll(wgpu::PollType::wait())
        .map_err(|e| io_error(format!("device poll failed: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let _ = device;
    Ok(())
}
