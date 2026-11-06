//! Error scopes and waiting, shared by every operation that touches the device.

use super::signal::Receiver;
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

/// Let the device run its callbacks until `done` settles. The browser event loop does that by itself.
///
/// A blocking `poll(wait)` notices finished work about 1.5 ms late on Metal, so this polls without blocking and sleeps
/// between polls, from 20 µs doubling to 500 µs: short work is seen within tens of microseconds, long work costs a few
/// polls per millisecond.
pub(super) fn drive<T>(device: &wgpu::Device, done: &Receiver<T>) -> Result<(), OsError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut pause = std::time::Duration::from_micros(20);
        loop {
            device
                .poll(wgpu::PollType::Poll)
                .map_err(|e| io_error(format!("device poll failed: {e}")))?;
            if done.settled() {
                return Ok(());
            }
            std::thread::sleep(pause);
            pause = (pause * 2).min(std::time::Duration::from_micros(500));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (device, done);
        Ok(())
    }
}
