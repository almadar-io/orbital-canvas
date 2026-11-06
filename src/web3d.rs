//! A 3D canvas in a browser page: orbital-canvas renders a `canvas mode:"3d"` node into an HTML `<canvas>` on the
//! device slot it shares with the `gpu/*` answerer, so a scene and the program's GPU work see the same buffers. A
//! page without WebGPU gets a WebGL2 device of the canvas's own: scenes draw, GPU drawables are refused there.

use crate::mappers::canvas_scene3d::{scene_from_canvas_node_with_preview, SceneTheme};
use crate::scene3d::{Button, Pick, Scene3D, Scene3DView};
use std::sync::Arc;

pub(crate) struct Opened {
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) adapter: wgpu::Adapter,
    pub(crate) device: Arc<wgpu::Device>,
    pub(crate) queue: Arc<wgpu::Queue>,
    pub(crate) webgpu: bool,
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

/// A surface on `canvas`: on the WebGPU device the page's gpu/* work shares, else a WebGL2 device of its own.
pub(crate) async fn open_canvas(
    canvas: web_sys::HtmlCanvasElement,
) -> Result<(Opened, wgpu::SurfaceConfiguration), String> {
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
    Ok((opened, config))
}

pub struct WebCanvas3D {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    renderer: crate::SharedDeviceRenderer,
    view: Scene3DView,
    css_size: (f32, f32),
    /// False on the WebGL2 fallback.
    webgpu: bool,
    node: Option<(serde_json::Value, SceneTheme)>,
    props: Option<crate::mappers::canvas3d_props::CanvasProps3D>,
    /// The canvas clock: three's R3F clock starts at the first frame and runs from then on.
    origin_ms: Option<f64>,
    elapsed_ms: f64,
    last_ms: Option<f64>,
    /// The page's wall clock (`performance.timeOrigin + now`), which fx entries born at a time read.
    epoch_ms: Option<f64>,
    reduced_motion: bool,
    hover: Option<(f32, f32)>,
}

/// What a pointer release did, for the host to emit.
pub enum Released {
    Pick(Pick),
    Select(Option<String>),
    Move {
        id: String,
        x: i64,
        y: i64,
    },
    /// A click on a learning canvas's mesh: the pattern's `{ id, index }` payload.
    Shape(serde_json::Value),
}

impl WebCanvas3D {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, String> {
        let (opened, config) = open_canvas(canvas).await?;
        let (width, height) = (config.width, config.height);
        let mut view = Scene3DView::new();
        view.resize(width as f32, height as f32);
        Ok(Self {
            renderer: crate::SharedDeviceRenderer::new(
                opened.device.clone(),
                opened.queue.clone(),
                config.format,
            ),
            surface: opened.surface,
            config,
            device: opened.device,
            view,
            css_size: (width as f32, height as f32),
            webgpu: opened.webgpu,
            node: None,
            props: None,
            origin_ms: None,
            elapsed_ms: 0.0,
            last_ms: None,
            epoch_ms: None,
            reduced_motion: false,
            hover: None,
        })
    }

    /// Whether this canvas draws on the WebGPU device the page's `gpu/*` work uses.
    pub fn webgpu(&self) -> bool {
        self.webgpu
    }

    fn learning(&self) -> bool {
        self.node.as_ref().is_some_and(|(node, _)| {
            matches!(
                node.get("type").and_then(serde_json::Value::as_str),
                Some("physics-canvas" | "biology-canvas" | "chemistry-canvas")
            )
        })
    }

    /// Map `node` (a 3D render node) with the page's resolved `theme`; the camera keeps the user's orbit unless the
    /// node's framing changed.
    pub fn set_scene(
        &mut self,
        node: &serde_json::Value,
        theme: &SceneTheme,
    ) -> Result<(), String> {
        self.node = Some((node.clone(), theme.clone()));
        self.props = if self.learning() {
            None
        } else {
            Some(crate::mappers::canvas3d_props::canvas_props_3d(node).map_err(|e| e.to_string())?)
        };
        self.view
            .set_edit(self.props.as_ref().and_then(|p| p.edit.clone()).map(|e| {
                crate::scene3d::view::EditMode {
                    selected: e.selected,
                }
            }));
        self.remap()
    }

    /// Animations play on the canvas clock once it runs, and rest before the first frame or under reduced motion.
    fn remap(&mut self) -> Result<(), String> {
        let Some((node, theme)) = &self.node else {
            return Ok(());
        };
        let (width, height) = self.css_size;
        let scene = if self.learning() {
            crate::mappers::learning_scene3d::learning_scene3d(node, width, height, theme)
        } else {
            let elapsed =
                (self.origin_ms.is_some() && !self.reduced_motion).then_some(self.elapsed_ms);
            scene_from_canvas_node_with_preview(
                node,
                width,
                height,
                theme,
                elapsed,
                self.epoch_ms,
                self.view.preview(),
            )
        }
        .map_err(|e| e.to_string())?;
        let interaction = &scene.interaction;
        self.view.set_follow(
            interaction
                .follow_from
                .map(|start_eye| crate::scene3d::view::Follow { start_eye }),
        );
        let picks_ground =
            interaction.editable || !interaction.cells.is_empty() || self.ground_events();
        self.view
            .set_ground_extent(picks_ground.then_some(crate::scene3d::view::GroundExtent {
                min_x: interaction.min_x,
                max_x: interaction.max_x,
                min_z: interaction.min_z,
                max_z: interaction.max_z,
            }));
        self.view.set_scene(scene);
        Ok(())
    }

    /// three mounts its ground plane only when the canvas declares a tile or unit click (or edits).
    fn ground_events(&self) -> bool {
        self.node.as_ref().is_some_and(|(node, _)| {
            let f = node.get("props").unwrap_or(node);
            f.get("tileClickEvent").is_some() || f.get("unitClickEvent").is_some()
        })
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

    /// The overlays three's host draws over the scene (minimap, coordinates, tile info) and a learning canvas's
    /// 2D layer (readouts, meters, traces, equation, shapes).
    fn overlays(
        &self,
        scene: &crate::scene3d::Scene3D,
    ) -> Result<Vec<crate::packet::DrawCommand>, String> {
        let mut out = Vec::new();
        let Some((node, theme)) = &self.node else {
            return Ok(out);
        };
        if self.learning() {
            out.extend(
                crate::mappers::learning_overlay3d::learning_overlay_2d(node, self.css_size)
                    .map_err(|e| e.to_string())?,
            );
            return Ok(out);
        }
        let Some(props) = &self.props else {
            return Ok(out);
        };
        if !(props.show_minimap || props.show_coordinates || props.show_tile_info) {
            return Ok(out);
        }
        let colors = crate::mappers::canvas3d_props::OverlayColors::from_theme(theme)
            .map_err(|e| e.to_string())?;
        let drawn = &scene.interaction.drawn;
        if props.show_coordinates {
            out.extend(crate::mappers::canvas3d_props::coordinate_labels(
                scene,
                self.css_size,
                drawn,
                &colors,
            ));
        }
        if props.show_minimap {
            out.extend(crate::mappers::canvas3d_props::minimap(
                drawn,
                self.css_size,
                &colors,
            ));
        }
        if let (true, Some(pointer)) = (props.show_tile_info, self.hover) {
            if let Some(cell) = self.view.ground_cell(pointer.0, pointer.1) {
                out.extend(crate::mappers::canvas3d_props::tile_info(
                    &scene.interaction,
                    cell,
                    pointer,
                    &colors,
                ));
            }
        }
        Ok(out)
    }

    pub fn render(&mut self) -> Result<(), String> {
        let (Some(scene), Some(camera)) = (self.view.scene(), self.view.camera()) else {
            return Ok(());
        };
        let framed = Scene3D {
            camera,
            ..scene.clone()
        };
        let mut draw_commands = vec![crate::packet::DrawCommand::Scene3D {
            scene: Box::new(framed.clone()),
        }];
        draw_commands.extend(self.overlays(&framed)?);
        let packet = crate::packet::GameRenderPacket {
            pattern: "canvas".into(),
            width: self.css_size.0,
            height: self.css_size.1,
            draw_commands,
            ..Default::default()
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

    /// Advances the canvas clock to `now_ms` (`performance.now()`) with the page's `epoch_ms`
    /// (`performance.timeOrigin + now`), eases a following camera and the orbit damping; true when the frame changed.
    pub fn frame(&mut self, now_ms: f64, epoch_ms: f64) -> Result<bool, String> {
        let origin = *self.origin_ms.get_or_insert(now_ms);
        let dt = self
            .last_ms
            .map_or(0.0, |last| ((now_ms - last) / 1000.0).max(0.0)) as f32;
        self.last_ms = Some(now_ms);
        self.elapsed_ms = (now_ms - origin).max(0.0);
        self.epoch_ms = Some(epoch_ms);
        let animated =
            self.view.scene().is_some_and(|scene| scene.animated) && !self.reduced_motion;
        if animated {
            self.remap()?;
        }
        Ok(self.view.advance(dt) || animated)
    }

    pub fn pointer_down(&mut self, id: i32, x: f32, y: f32, button: Button, touch: bool) {
        self.view.pointer_down(id, x, y, button, touch);
    }

    /// True when the frame changed (the camera moved, or a drag moved its preview).
    pub fn pointer_move(&mut self, id: i32, x: f32, y: f32) -> Result<bool, String> {
        self.hover = Some((x, y));
        let before = self.view.preview().cloned();
        let moved = self.view.pointer_move(id, x, y);
        if self.view.preview().cloned() != before {
            self.remap()?;
            return Ok(true);
        }
        Ok(moved || self.props.as_ref().is_some_and(|p| p.show_tile_info))
    }

    pub fn pointer_up(
        &mut self,
        id: i32,
        x: f32,
        y: f32,
        unit_events: bool,
        tile_events: bool,
    ) -> Result<Option<Released>, String> {
        let released = self
            .view
            .pointer_release(id, x, y, unit_events, tile_events);
        if self.view.preview().is_none() {
            self.remap()?;
        }
        Ok(match released {
            None => None,
            Some(crate::scene3d::view::Release::Pick(Pick::Unit { id, x, z }))
                if self.learning() =>
            {
                let (node, _) = self.node.as_ref().ok_or("no scene")?;
                let _ = (x, z);
                Some(Released::Shape(
                    crate::mappers::learning_overlay3d::learning_click_payload(node, &id)
                        .map_err(|e| e.to_string())?,
                ))
            }
            Some(crate::scene3d::view::Release::Pick(pick)) => Some(Released::Pick(pick)),
            Some(crate::scene3d::view::Release::Select { id }) => Some(Released::Select(id)),
            Some(crate::scene3d::view::Release::Move { id, x, y }) => {
                Some(Released::Move { id, x, y })
            }
        })
    }

    pub fn pointer_leave(&mut self) {
        self.hover = None;
    }

    pub fn wheel(&mut self, delta_y: f32) -> bool {
        self.view.wheel(delta_y)
    }

    pub fn camera(&self) -> Option<crate::scene3d::Camera> {
        self.view.camera()
    }

    pub fn set_camera_position(&mut self, position: [f32; 3]) -> bool {
        self.view.set_camera_position(position)
    }

    pub fn look_at(&mut self, target: [f32; 3]) -> bool {
        self.view.look_at(target)
    }

    pub fn reset_camera(&mut self) -> bool {
        self.view.reset_camera()
    }
}
