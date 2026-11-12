//! A scene canvas in a browser page: orbital-canvas maps and draws any canvas-style render node (the learning
//! instruments, diagrams, game `canvas` in 2D) into an HTML `<canvas>`, with `SceneHost` keeping the layout, clock,
//! taps, drags and pan/zoom exactly as native shells keep them.

use crate::web3d::open_canvas;
use crate::{AccessAction, KeyMods, SceneHost, SharedDeviceRenderer};
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

    /// Advances the scene clock to `now_ms` (`performance.now()`) with the page's `epoch_ms`
    /// (`performance.timeOrigin + now`); true while the host should keep drawing (an animated scene or assets
    /// still landing).
    pub fn frame(&mut self, now_ms: f64, epoch_ms: f64) -> bool {
        self.host.set_clock(now_ms);
        self.host.set_epoch_clock(epoch_ms);
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

    pub fn zoom(&mut self, factor: f32, x: f32, y: f32) -> bool {
        self.host.zoom(factor, x, y)
    }

    pub fn double_tap(&mut self, x: f32, y: f32) {
        self.host.double_tap(x, y);
    }

    /// The semantic tree as a JSON array in reading order, each node's rect in CSS pixels.
    pub fn semantic_tree(&self) -> String {
        let nodes: Vec<serde_json::Value> = self
            .host
            .semantic_tree()
            .iter()
            .map(|n| {
                let mut o = serde_json::Map::new();
                o.insert("id".into(), n.id.clone().into());
                o.insert("role".into(), n.role.clone().into());
                o.insert("name".into(), n.name.clone().into());
                if let Some(v) = &n.value {
                    o.insert("value".into(), v.clone().into());
                }
                if let Some(c) = n.checked {
                    o.insert("checked".into(), c.into());
                }
                o.insert("disabled".into(), n.disabled.into());
                o.insert("focused".into(), n.focused.into());
                if let Some(c) = n.caret {
                    o.insert("caret".into(), c.into());
                }
                if let Some(c) = n.selection_start {
                    o.insert("selectionStart".into(), c.into());
                }
                if let Some(c) = n.selection_end {
                    o.insert("selectionEnd".into(), c.into());
                }
                o.insert("multiline".into(), n.multiline.into());
                o.insert("order".into(), serde_json::json!(n.order));
                o.insert(
                    "rect".into(),
                    serde_json::json!({
                        "x": n.rect.x, "y": n.rect.y, "w": n.rect.w, "h": n.rect.h
                    }),
                );
                serde_json::Value::Object(o)
            })
            .collect();
        serde_json::Value::Array(nodes).to_string()
    }

    /// Text for the focused text field: committed (`composing` false) or the input method's preedit.
    pub fn text_input(&mut self, target: &str, text: &str, composing: bool) {
        self.host.text_input(target, text, composing);
    }

    /// An editing key (a DOM `KeyboardEvent.key`) for the focused text field.
    pub fn key_input(
        &mut self,
        target: &str,
        key: &str,
        shift: bool,
        alt: bool,
        ctrl: bool,
        meta: bool,
    ) {
        self.host
            .key_input(target, key, KeyMods { shift, alt, ctrl, meta });
    }

    /// An assistive technology acted on a node: `focus`, `activate`, `increment` or `decrement`.
    pub fn access_action(&mut self, target: &str, action: &str) -> Result<(), String> {
        let action = match action {
            "focus" => AccessAction::Focus,
            "activate" => AccessAction::Activate,
            "increment" => AccessAction::Increment,
            "decrement" => AccessAction::Decrement,
            other => return Err(format!("unknown access action `{other}`")),
        };
        self.host.access_action(target, action);
        Ok(())
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
