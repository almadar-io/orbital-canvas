//! Render a learning-canvas scene (props JSON with a `type`) to a PNG through
//! the same mapper and renderer the native shells use.
//!
//! Usage: `cargo run -p orbital-canvas --example dump-learning-scene -- <scene.json> [out.png] [width] [height] [density] [light|dark]`

use std::sync::Arc;

use orbital_canvas::mappers::scene::map_scene;
use orbital_canvas::renderer::{copy_texture_to_rgba, SharedDeviceRenderer};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("scene .json path");
    let out = args.next().unwrap_or_else(|| "/tmp/learning-scene.png".into());
    let vw: f32 = args.next().map_or(390.0, |v| v.parse().expect("width"));
    let vh: f32 = args.next().map_or(300.0, |v| v.parse().expect("height"));
    let density: f32 = args.next().map_or(3.0, |v| v.parse().expect("density"));
    let scheme = args.next().unwrap_or_else(|| "light".into());
    let mut node: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("readable")).expect("valid JSON");
    node["colorScheme"] = serde_json::Value::String(scheme);

    let scene = map_scene(&node, Some((vw, vh))).expect("scene maps");
    println!(
        "packet {}x{}: {} commands, {} hit regions",
        scene.packet.width,
        scene.packet.height,
        scene.packet.draw_commands.len(),
        scene.hits.len()
    );

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("wgpu adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("dump-learning-scene"),
        ..Default::default()
    }))
    .expect("wgpu device");
    let mut renderer = SharedDeviceRenderer::new(Arc::new(device), Arc::new(queue), wgpu::TextureFormat::Rgba8Unorm);

    let (w, h) = ((vw * density) as u32, (vh * density) as u32);
    let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("dump target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    renderer.render_packet_to_texture(&scene.packet, &target);
    let encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
    let rgba = copy_texture_to_rgba(renderer.device(), renderer.queue(), encoder, &target, w, h).expect("readback");
    image::save_buffer(&out, &rgba, w, h, image::ColorType::Rgba8).expect("png written");
    println!("wrote {out} ({w}x{h})");
}
