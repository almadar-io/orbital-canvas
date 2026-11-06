//! A scene canvas in a browser page: orbital-canvas maps and draws any canvas-style render node (the learning
//! instruments, diagrams, game `canvas` in 2D) into an HTML `<canvas>`, with `SceneHost` keeping the layout, clock,
//! taps, drags and pan/zoom exactly as native shells keep them.

use crate::web3d::open_canvas;
use crate::{SceneHost, SharedDeviceRenderer};
use std::sync::Arc;

pub struct WebSceneCanvas {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    renderer: SharedDeviceRenderer,
    host: SceneHost,
    webgpu: bool,
}

impl WebSceneCanvas {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, String> {
        let (opened, config) = open_canvas(canvas).await?;
        let mut host = SceneHost::new();
        host.set_viewport(config.width, config.height);
        Ok(Self {
            renderer: SharedDeviceRenderer::new(
                opened.device.clone(),
                opened.queue.clone(),
                config.format,
            ),
            surface: opened.surface,
            config,
            device: opened.device,
            host,
            webgpu: opened.webgpu,
        })
    }

    pub fn webgpu(&self) -> bool {
        self.webgpu
    }

    pub fn set_scene(&mut self, node: serde_json::Value) -> Result<(), String> {
        self.host.set_scene(node).map_err(|e| e.to_string())
    }

    /// The element's CSS size and the device pixel ratio: the scene lays out in CSS pixels on a surface of
    /// their product.
    pub fn resize(
        &mut self,
        css_width: f32,
        css_height: f32,
        pixel_ratio: f32,
    ) -> Result<(), String> {
        self.config.width = (css_width.max(1.0) * pixel_ratio).round().max(1.0) as u32;
        self.config.height = (css_height.max(1.0) * pixel_ratio).round().max(1.0) as u32;
        self.surface.configure(&self.device, &self.config);
        self.host
            .set_viewport(self.config.width, self.config.height);
        self.host.set_scale(pixel_ratio)
    }

    pub fn set_motion(&mut self, enabled: bool) {
        self.host.set_motion(enabled);
    }

    /// Advances the scene clock to `now_ms`; true while the host should keep drawing (an animated scene or
    /// assets still landing).
    pub fn frame(&mut self, now_ms: f64) -> bool {
        self.host.set_clock(now_ms);
        self.host.refresh_assets();
        self.host.wants_frames()
    }

    pub fn render(&mut self) -> Result<(), String> {
        let Some(packet) = self.host.frame_packet() else {
            return Ok(());
        };
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|e| e.to_string())?;
        self.renderer
            .render_packet_to_texture(&packet, &frame.texture);
        frame.present();
        Ok(())
    }

    /// A pointer event in CSS pixels (`touch-down`, `touch-move`, `touch-up`, `touch-cancel`, `wheel`, …) as
    /// `SceneHost::input` reads it.
    pub fn input(&mut self, kind: &str, x: f32, y: f32, button: i32) {
        self.host.input(kind, x, y, button, None);
    }

    pub fn zoom(&mut self, factor: f32, x: f32, y: f32) {
        self.host.zoom(factor, x, y);
    }

    pub fn double_tap(&mut self, x: f32, y: f32) {
        self.host.double_tap(x, y);
    }

    /// The scene events taps and drags queued, each `{ event, payload }`.
    pub fn take_events(&mut self) -> serde_json::Value {
        serde_json::Value::Array(
            self.host
                .take_scene_events()
                .into_iter()
                .map(|emit| serde_json::json!({ "event": emit.event, "payload": emit.delivered() }))
                .collect(),
        )
    }
}
