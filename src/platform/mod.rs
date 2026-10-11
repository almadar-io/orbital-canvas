//! Platform-specific surface initialization glue.

#[cfg(feature = "desktop")]
pub mod desktop;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "ios")]
pub mod ios;
