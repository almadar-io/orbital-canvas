//! A 3D canvas in a browser page: orbital-canvas renders a `canvas mode:"3d"` node into an HTML `<canvas>` on the
//! device slot it shares with the `gpu/*` answerer, so a scene and the program's GPU work see the same buffers. A
//! page without WebGPU gets a WebGL2 device of the canvas's own: scenes draw, GPU drawables are refused there.

use crate::mappers::canvas_scene3d::{
    scene_from_canvas_node_at, scene_from_canvas_node_themed, SceneTheme,
};
use crate::scene3d::{Button, Pick, Renderer3D, Scene3D, Scene3DView};
use std::sync::Arc;

pub struct WebCanvas3D {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    renderer: Renderer3D,
    view: Scene3DView,
    css_size: (f32, f32),
    /// False on the WebGL2 fallback.
    webgpu: bool,
    node: Option<(serde_json::Value, SceneTheme)>,
    /// The canvas clock: three's R3F clock starts at the first frame and runs from then on.
    origin_ms: Option<f64>,
    elapsed_ms: f64,
    reduced_motion: bool,
}

struct Opened {
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    webgpu: bool,
}

async fn open_webgpu(
    canvas: web_sys::HtmlCanvasElement,
) -> Result<Opened, (String, web_sys::HtmlCanvasElement)> {
    let shared = match crate::device::shared_device_async().await {
        Ok(shared) => shared,
        Err(error) => return Err((error.to_string(), canvas)),
    };
    let (Some(instance), Some(adapter)) = (shared.instance.clone(), shared.adapter.clone()) else {
        return Err((
            "the shared device was registered without its instance and adapter".into(),
            canvas,
        ));
    };
    let surface = match instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone())) {
        Ok(surface) => surface,
        Err(error) => return Err((error.to_string(), canvas)),
    };
    Ok(Opened {
        surface,
        adapter,
        device: shared.device.clone(),
        queue: shared.queue.clone(),
        webgpu: true,
    })
}

async fn open_webgl2(
    canvas: web_sys::HtmlCanvasElement,
    webgpu_error: &str,
) -> Result<Opened, String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        ..Default::default()
    });
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| format!("no WebGPU ({webgpu_error}) and no WebGL2 surface: {e}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .map_err(|e| format!("no WebGPU ({webgpu_error}) and no WebGL2 adapter: {e}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("orbital-canvas webgl2"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(Opened {
        surface,
        adapter,
        device: Arc::new(device),
        queue: Arc::new(queue),
        webgpu: false,
    })
}

impl WebCanvas3D {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, String> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let opened = match open_webgpu(canvas).await {
            Ok(opened) => opened,
            Err((error, canvas)) => open_webgl2(canvas, &error).await?,
        };
        let caps = opened.surface.get_capabilities(&opened.adapter);
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
        opened.surface.configure(&opened.device, &config);
        let mut view = Scene3DView::new();
        view.resize(width as f32, height as f32);
        Ok(Self {
            surface: opened.surface,
            config,
            device: opened.device.clone(),
            renderer: Renderer3D::new(opened.device, opened.queue),
            view,
            css_size: (width as f32, height as f32),
            webgpu: opened.webgpu,
            node: None,
            origin_ms: None,
            elapsed_ms: 0.0,
            reduced_motion: false,
        })
    }

    /// Whether this canvas draws on the WebGPU device the page's `gpu/*` work uses.
    pub fn webgpu(&self) -> bool {
        self.webgpu
    }

    /// Map `node` (a `canvas` render-ui node) with the page's resolved `theme`; the camera keeps the user's orbit
    /// unless the node's framing changed.
    pub fn set_scene(
        &mut self,
        node: &serde_json::Value,
        theme: &SceneTheme,
    ) -> Result<(), String> {
        self.node = Some((node.clone(), theme.clone()));
        self.remap()
    }

    /// Animations play on the canvas clock once it runs, and rest before the first frame or under reduced motion.
    fn remap(&mut self) -> Result<(), String> {
        let Some((node, theme)) = &self.node else {
            return Ok(());
        };
        let (width, height) = self.css_size;
        let scene = match self.origin_ms {
            Some(_) if !self.reduced_motion => {
                scene_from_canvas_node_at(node, width, height, theme, self.elapsed_ms)
            }
            _ => scene_from_canvas_node_themed(node, width, height, theme),
        }
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
        self.reduced_motion = reduced;
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

    /// Advances the canvas clock to `now_ms` (any monotonic clock) and damping; true when the frame changed.
    pub fn frame(&mut self, now_ms: f64) -> Result<bool, String> {
        let origin = *self.origin_ms.get_or_insert(now_ms);
        self.elapsed_ms = (now_ms - origin).max(0.0);
        let animated =
            self.view.scene().is_some_and(|scene| scene.animated) && !self.reduced_motion;
        if animated {
            self.remap()?;
        }
        Ok(self.view.frame() || animated)
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
