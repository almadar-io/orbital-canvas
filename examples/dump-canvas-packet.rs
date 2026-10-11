//! Dump a `.orb` fixture's `canvas`/`canvas-2d` pattern to a PNG — headless
//! visual proof of the `orbital-canvas` display list (the same packet
//! the dioxus-native paint source composites through vello). Deterministic:
//! no window, no compositor, no timing.
//!
//! Usage: `cargo run -p orbital-canvas --example dump-canvas-packet -- <fixture.orb> [out.png]`

use std::sync::Arc;
use std::time::{Duration, Instant};

use orbital_canvas::mappers::game_canvas_2d;
use orbital_canvas::packet::DrawCommand;
use orbital_canvas::renderer::{copy_texture_to_rgba, SharedDeviceRenderer};

/// Depth-first search for the first `canvas` / `canvas-2d` pattern node.
fn find_canvas(n: &serde_json::Value) -> Option<serde_json::Value> {
    if let Some(obj) = n.as_object() {
        if matches!(
            obj.get("type").and_then(|v| v.as_str()),
            Some("canvas") | Some("canvas-2d")
        ) {
            return Some(n.clone());
        }
        for v in obj.values() {
            if let Some(r) = find_canvas(v) {
                return Some(r);
            }
        }
    } else if let Some(arr) = n.as_array() {
        for v in arr {
            if let Some(r) = find_canvas(v) {
                return Some(r);
            }
        }
    }
    None
}

fn sprite_count(packet: &orbital_canvas::packet::GameRenderPacket) -> usize {
    packet
        .draw_commands
        .iter()
        .filter(|c| matches!(c, DrawCommand::Sprite { .. }))
        .count()
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("almadar::game_renderer=info")),
        )
        .init();
    let fixture = std::env::args().nth(1).expect("fixture .orb path");
    let out = std::env::args().nth(2).unwrap_or_else(|| "/tmp/canvas-packet.png".into());
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&fixture).expect("fixture readable"),
    )
    .expect("fixture is valid JSON");
    let props = find_canvas(&schema).expect("fixture contains a canvas/canvas-2d pattern");

    // First map schedules asset fetches (atlas JSON, sprite PNGs); rebuild
    // until the sprite count stabilizes — the same in-flight → resolved
    // convergence the dioxus paint source gets from the asset generation.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut packet = game_canvas_2d::from_props(&props);
    let mut last_sprites = sprite_count(&packet);
    loop {
        std::thread::sleep(Duration::from_millis(60));
        packet = game_canvas_2d::from_props(&props);
        let sprites = sprite_count(&packet);
        if (sprites == last_sprites && sprites > 0) || Instant::now() > deadline {
            break;
        }
        last_sprites = sprites;
    }
    println!(
        "packet: {}x{}, {} commands, {} sprites",
        packet.width,
        packet.height,
        packet.draw_commands.len(),
        sprite_count(&packet)
    );
    for cmd in &packet.draw_commands {
        if let DrawCommand::Sprite {
            texture,
            src,
            dst,
            opacity,
            ..
        } = cmd
        {
            println!(
                "sprite {} src=({},{},{}x{}) dst=({},{},{}x{}) opacity={}",
                texture.rsplit('/').next().unwrap_or(texture),
                src.x, src.y, src.w, src.h,
                dst.x, dst.y, dst.w, dst.h,
                opacity
            );
        }
    }
    if let Some(DrawCommand::Poly { points, .. }) = packet
        .draw_commands
        .iter()
        .find(|c| matches!(c, DrawCommand::Poly { .. }))
    {
        println!("poly first-point=({},{}) count={}", points[0].x, points[0].y, points.len());
    }

    // Pre-warm every sprite texture: the mapper only ensures atlas JSON +
    // whole-image dims, so a sheet PNG referenced purely by an atlas sprite
    // starts fetching at upload time. Wait for the bytes so this single
    // render pass has every texture (the windowed shells get this for free
    // from their vsync repaint loop).
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let pending = packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Sprite { texture, .. } => Some(texture.as_str()),
                _ => None,
            })
            .filter(|url| orbital_canvas::asset::asset_bytes(url).is_none())
            .count();
        if pending == 0 || Instant::now() > deadline {
            if pending > 0 {
                println!("WARNING: {pending} sprite texture(s) never arrived");
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

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
        label: Some("dump-canvas-packet"),
        ..Default::default()
    }))
    .expect("wgpu device");

    let mut renderer = SharedDeviceRenderer::new(
        Arc::new(device),
        Arc::new(queue),
        wgpu::TextureFormat::Rgba8Unorm,
    );

    let width = packet.width.max(1.0) as u32;
    let height = packet.height.max(1.0) as u32;
    let target = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("dump target"),
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

    let drew = renderer.render_packet_to_texture(&packet, &target);
    println!("render_packet_to_texture: drew={drew}");

    let encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
    let rgba = copy_texture_to_rgba(
        renderer.device(),
        renderer.queue(),
        encoder,
        &target,
        width,
        height,
    )
    .expect("readback");

    image::save_buffer(
        &out,
        &rgba,
        width,
        height,
        image::ColorType::Rgba8,
    )
    .expect("png written");
    println!("wrote {out} ({width}x{height})");
}
