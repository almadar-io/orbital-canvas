//! The canvas a host embeds: its instance, surface, device and painters around the scene it shows.

use super::*;
use crate::packet::GameRenderPacket;
use crate::renderer::{copy_texture_to_rgba, DeviceResources, SharedDeviceRenderer};
use crate::{RendererError, Result};

/// Shared game renderer state.
pub struct Canvas {
    /// Global `wgpu` instance. Lives for the lifetime of the renderer.
    pub(crate) instance: wgpu::Instance,
    /// Optional render surface (present on desktop/mobile, absent in headless/tests).
    pub(crate) surface: Option<wgpu::Surface<'static>>,
    /// Lazily-initialized GPU device, queue, and pipeline.
    pub(crate) device_resources: Option<DeviceResources>,
    /// On-surface painter: the same sprite + quad pipelines the host-device shells use.
    pub(crate) shared: Option<renderer::SharedDeviceRenderer>,
    /// Painter for the readback path: always `Rgba8Unorm`, whatever the surface format is.
    pub(crate) rgba_shared: Option<renderer::SharedDeviceRenderer>,
    /// The scene, its layout memory and its input state ([`SceneHost`]).
    pub(crate) host: SceneHost,
}

impl Canvas {
    /// Create a headless renderer with no surface.
    ///
    /// Used on platforms where the surface is supplied later, or in tests.
    pub fn new_headless() -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
        });

        Ok(Self {
            instance,
            surface: None,
            device_resources: None,
            shared: None,
            rgba_shared: None,
            host: SceneHost::new(),
        })
    }

    /// Create a renderer bound to an already-created wgpu surface.
    ///
    /// The `instance` that was used to create `surface` is passed in so that
    /// both can live together in the renderer — the surface must not outlive the instance.
    // Called from platform::android and platform::ios (cfg-gated); appears unused on desktop.
    #[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
    pub(crate) fn new_with_surface(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
    ) -> Result<Self> {
        Ok(Self {
            instance,
            surface: Some(surface),
            device_resources: None,
            shared: None,
            rgba_shared: None,
            host: SceneHost::new(),
        })
    }

    /// Render one frame from the current packet.
    ///
    /// When a surface is present (Android/iOS), renders to the surface texture and presents.
    /// In headless mode, this is a no-op; call `render_packet_to_rgba` explicitly instead.
    /// Drains the pending input queue before rendering so callers can read it from the packet's
    /// tick_event path on the next submit cycle.
    pub fn render(&mut self) -> Result<()> {
        self.refresh_assets();
        // Drain queued input; the runtime picks these up via tick_event/draw_event callbacks.
        if !self.input_queue.is_empty() {
            tracing::trace!(count = self.input_queue.len(), "draining input queue");
            self.input_queue.clear();
        }
        if let Some(packet) = self.frame_packet() {
            tracing::trace!(pattern = %packet.pattern, "render packet");
            if self.surface.is_some() {
                self.render_to_surface(&packet)?;
            }
        }
        Ok(())
    }

    /// Resize the render surface and reconfigure it.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.set_viewport(width, height);
        if self.surface.is_some() {
            self.configure_surface(width, height)?;
        }
        Ok(())
    }
}

impl std::ops::Deref for Canvas {
    type Target = SceneHost;
    fn deref(&self) -> &SceneHost {
        &self.host
    }
}

impl std::ops::DerefMut for Canvas {
    fn deref_mut(&mut self) -> &mut SceneHost {
        &mut self.host
    }
}

impl Canvas {
    /// Ensure a `wgpu` adapter, device, queue, and pipeline are available.
    fn ensure_device_resources(&mut self) -> Result<&DeviceResources> {
        if self.device_resources.is_none() {
            let shared = crate::device::acquire(&self.instance, self.surface.as_ref())?;
            if self.surface.is_none() {
                if let Some(instance) = &shared.instance {
                    self.instance = instance.clone();
                }
            }
            let device = shared.device.clone();
            let queue = shared.queue.clone();

            // Use the surface's preferred format when on-screen, fall back to Rgba8Unorm headless.
            let surface_format = match (self.surface.as_ref(), shared.adapter.as_ref()) {
                (Some(s), Some(adapter)) => {
                    // Packet colors are already sRGB-encoded (CSS values): an sRGB target would
                    // encode them a second time and wash every color out.
                    let formats = s.get_capabilities(adapter).formats;
                    formats
                        .iter()
                        .copied()
                        .find(|f| !f.is_srgb())
                        .or_else(|| formats.first().copied())
                        .unwrap_or(wgpu::TextureFormat::Rgba8Unorm)
                }
                (Some(_), None) => {
                    return Err(RendererError::Surface(
                        "a surface needs the adapter that opened the shared device".into(),
                    ))
                }
                (None, _) => wgpu::TextureFormat::Rgba8Unorm,
            };

            self.device_resources = Some(DeviceResources {
                adapter: shared.adapter,
                device: device.clone(),
                queue: queue.clone(),
                surface_format,
            });
        }

        Ok(self.device_resources.as_ref().expect("just initialized"))
    }

    /// Configure (or reconfigure) the surface for a given size.
    ///
    /// Must be called before the first `render_to_surface` and on every resize.
    pub(crate) fn configure_surface(&mut self, width: u32, height: u32) -> Result<()> {
        self.set_viewport(width, height);
        let resources = self.ensure_device_resources()?;
        let format = resources.surface_format;
        let device = resources.device.clone();
        let surface = self
            .surface
            .as_ref()
            .ok_or_else(|| RendererError::Surface("no surface".into()))?;
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: width.max(1),
                height: height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            },
        );
        Ok(())
    }

    /// Render one frame from `packet` to the on-screen surface and present it.
    pub(crate) fn render_to_surface(
        &mut self,
        packet: &crate::packet::GameRenderPacket,
    ) -> Result<()> {
        let surface = self
            .surface
            .as_ref()
            .ok_or_else(|| RendererError::Surface("no surface".into()))?;

        let frame = surface
            .get_current_texture()
            .map_err(|e| RendererError::Surface(format!("get_current_texture: {e:?}")))?;

        let resources = self.ensure_device_resources()?;
        let device = resources.device.clone();
        let queue = resources.queue.clone();
        let format = resources.surface_format;
        self.shared
            .get_or_insert_with(|| SharedDeviceRenderer::new(device, queue, format))
            .render_packet_to_texture(packet, &frame.texture);
        frame.present();
        Ok(())
    }

    /// Render the supplied packet into an RGBA pixel buffer.
    ///
    /// The returned buffer has length `width * height * 4` and is in
    /// top-to-bottom row order matching image conventions.
    pub fn render_packet_to_rgba(&mut self, packet: &GameRenderPacket) -> Result<Vec<u8>> {
        let resources = self.ensure_device_resources()?;
        let device = resources.device.clone();
        let queue = resources.queue.clone();

        // The readback is the host surface in physical pixels once a viewport is known
        // (packets are in logical units), else the packet's own size.
        let (width, height) = self
            .viewport
            .unwrap_or((packet.width.max(1.0) as u32, packet.height.max(1.0) as u32));

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game render target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let shared = self.rgba_shared.get_or_insert_with(|| {
            SharedDeviceRenderer::new(
                device.clone(),
                queue.clone(),
                wgpu::TextureFormat::Rgba8Unorm,
            )
        });
        shared.render_packet_to_texture(packet, &texture);
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("game readback encoder"),
        });
        copy_texture_to_rgba(&device, &queue, encoder, &texture, width, height)
    }
}
