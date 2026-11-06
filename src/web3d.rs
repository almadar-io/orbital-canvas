//! A 3D canvas in a browser page: orbital-canvas renders a `canvas mode:"3d"` node into an HTML `<canvas>` on the
//! device slot it shares with the `gpu/*` answerer, so a scene and the program's GPU work see the same buffers.

use crate::mappers::canvas_scene3d::{scene_from_canvas_node_themed, SceneTheme};
use crate::scene3d::{Button, Pick, Renderer3D, Scene3D, Scene3DView};
use std::sync::Arc;

pub struct WebCanvas3D {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    renderer: Renderer3D,
    view: Scene3DView,
    css_size: (f32, f32),
}

impl WebCanvas3D {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, String> {
        let shared = crate::device::shared_device_async()
            .await
            .map_err(|e| e.to_string())?;
        let instance = shared.instance.clone().ok_or(
            "the shared device was registered without its instance, so no surface can be made",
        )?;
        let adapter = shared
            .adapter
            .as_ref()
            .ok_or("the shared device has no adapter")?;
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| e.to_string())?;
        let caps = surface.get_capabilities(adapter);
        let format = *caps
            .formats
            .first()
            .ok_or("the canvas offers no surface format")?;
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            caps.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&shared.device, &config);
        let mut view = Scene3DView::new();
        view.resize(width as f32, height as f32);
        Ok(Self {
            surface,
            config,
            device: shared.device.clone(),
            renderer: Renderer3D::new(shared.device.clone(), shared.queue.clone()),
            view,
            css_size: (width as f32, height as f32),
        })
    }

    /// Map `node` (a `canvas` render-ui node) with the page's resolved `theme`; the camera keeps the user's orbit
    /// unless the node's framing changed.
    pub fn set_scene(
        &mut self,
        node: &serde_json::Value,
        theme: &SceneTheme,
    ) -> Result<(), String> {
        let scene = scene_from_canvas_node_themed(node, self.css_size.0, self.css_size.1, theme)
            .map_err(|e| e.to_string())?;
        self.view.set_scene(scene);
        Ok(())
    }

    /// The element's CSS size and the device pixel ratio; the drawing buffer is the product.
    pub fn resize(&mut self, css_width: f32, css_height: f32, pixel_ratio: f32) {
        self.css_size = (css_width.max(1.0), css_height.max(1.0));
        self.view.resize(self.css_size.0, self.css_size.1);
        self.config.width = (self.css_size.0 * pixel_ratio).round().max(1.0) as u32;
        self.config.height = (self.css_size.1 * pixel_ratio).round().max(1.0) as u32;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.view.set_reduced_motion(reduced);
    }

    pub fn render(&mut self) -> Result<(), String> {
        let (Some(scene), Some(camera)) = (self.view.scene(), self.view.camera()) else {
            return Ok(());
        };
        let framed = Scene3D {
            camera,
            ..scene.clone()
        };
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|e| e.to_string())?;
        self.renderer
            .render(&framed, &frame.texture, 4)
            .map_err(|e| e.to_string())?;
        frame.present();
        Ok(())
    }

    /// Advances damping; true while the camera still moves (the host keeps animating).
    pub fn frame(&mut self) -> bool {
        self.view.frame()
    }

    pub fn pointer_down(&mut self, id: i32, x: f32, y: f32, button: Button, touch: bool) {
        self.view.pointer_down(id, x, y, button, touch);
    }

    pub fn pointer_move(&mut self, id: i32, x: f32, y: f32) -> bool {
        self.view.pointer_move(id, x, y)
    }

    pub fn pointer_up(
        &mut self,
        id: i32,
        x: f32,
        y: f32,
        unit_events: bool,
        tile_events: bool,
    ) -> Option<Pick> {
        self.view.pointer_up(id, x, y, unit_events, tile_events)
    }

    pub fn wheel(&mut self, delta_y: f32) -> bool {
        self.view.wheel(delta_y)
    }

    pub fn camera(&self) -> Option<crate::scene3d::Camera> {
        self.view.camera()
    }
}
