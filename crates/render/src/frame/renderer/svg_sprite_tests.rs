use super::sprites::MAX_SVG_RASTERS_PER_URL;
use super::*;
use orbital_canvas_base::packet::{DrawCommand, GameRenderPacket, Rect};

/// Left half blue, right half transparent; 4x4 intrinsic.
const HALF_BLUE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="2" height="4" fill="#0000ff"/></svg>"##;

pub(super) fn renderer() -> Option<SharedDeviceRenderer> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: wgpu::BackendOptions::default(),
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: wgpu::Features::empty(),
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        label: Some("svg sprite test"),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    Some(SharedDeviceRenderer::new(
        Arc::new(device),
        Arc::new(queue),
        wgpu::TextureFormat::Rgba8Unorm,
    ))
}

pub(super) fn fixture(name: &str, contents: &[u8]) -> String {
    let dir =
        std::env::temp_dir().join(format!("orbital-canvas-svg-sprite-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    let url = format!("file://{}", path.display());
    for _ in 0..200 {
        if orbital_canvas_paint::asset::asset_bytes(&url).is_some() {
            return url;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("fixture must arrive within 2s");
}

fn sprite_packet(url: &str, logical: f32) -> GameRenderPacket {
    GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: logical,
        height: logical,
        draw_commands: vec![DrawCommand::Sprite {
            texture: url.to_string(),
            src: Rect {
                x: 0.0,
                y: 0.0,
                w: 4.0,
                h: 4.0,
            },
            dst: Rect {
                x: 0.0,
                y: 0.0,
                w: logical,
                h: logical,
            },
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        }],
        ..Default::default()
    }
}

/// Render `packet` into a physical `px` x `px` target and read it back.
pub(super) fn draw(
    r: &mut SharedDeviceRenderer,
    packet: &GameRenderPacket,
    px: u32,
) -> Option<Vec<u8>> {
    let target = r.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("svg sprite target"),
        size: wgpu::Extent3d {
            width: px,
            height: px,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    assert!(
        r.render_packet_to_texture(packet, &target),
        "sprite should draw"
    );
    let encoder = r
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("readback"),
        });
    copy_texture_to_rgba(r.device(), r.queue(), encoder, &target, px, px).ok()
}

pub(super) fn px(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn svg_sprite_draws_through_the_textured_pipeline() {
    let Some(mut r) = renderer() else { return };
    let url = fixture("half-blue.svg", HALF_BLUE.as_bytes());
    let Some(rgba) = draw(&mut r, &sprite_packet(&url, 8.0), 8) else {
        return;
    };
    let left = px(&rgba, 8, 1, 4);
    assert!(
        left[2] > 200 && left[0] < 60 && left[3] > 200,
        "left half is the svg's blue, got {left:?}"
    );
    assert_eq!(
        px(&rgba, 8, 6, 4)[3],
        0,
        "right half stays transparent (svg alpha preserved)"
    );
}

#[test]
fn svg_pattern_and_gradient_mask_reach_gpu_pixels() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
        <defs>
            <pattern id="tile" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="4" fill="#ff0000"/></pattern>
            <linearGradient id="fade" x1="0%" y1="0%" x2="100%" y2="0%"><stop offset="0%" stop-color="white" stop-opacity="1"/><stop offset="100%" stop-color="white" stop-opacity="0"/></linearGradient>
            <mask id="mask"><rect width="100%" height="100%" fill="url(#fade)"/></mask>
        </defs>
        <rect width="100%" height="100%" fill="url(#tile)" mask="url(#mask)"/>
    </svg>"##;
    let url = fixture("pattern-mask.svg", svg.as_bytes());
    let packet = GameRenderPacket {
        pattern: "geometric-pattern-scene".into(),
        width: 16.0,
        height: 16.0,
        draw_commands: vec![DrawCommand::Sprite {
            texture: url,
            src: Rect {
                x: 0.0,
                y: 0.0,
                w: 16.0,
                h: 16.0,
            },
            dst: Rect {
                x: 0.0,
                y: 0.0,
                w: 16.0,
                h: 16.0,
            },
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        }],
        ..Default::default()
    };
    let pixels = draw(&mut renderer, &packet, 16).expect("pattern mask GPU frame");
    let alphas = [1, 5, 9, 13].map(|x| px(&pixels, 16, x, 8)[3]);
    assert!(alphas[0] > 200, "visible edge: {alphas:?}");
    assert!(
        alphas[0] > alphas[1] && alphas[1] > alphas[2] && alphas[2] > alphas[3],
        "gradient fade: {alphas:?}"
    );
    assert!(alphas[3] < 80, "faded edge: {alphas:?}");
    for x in [2, 6, 10, 14] {
        assert!(
            px(&pixels, 16, x, 8)[3] < alphas[0] / 2,
            "tile gap at x={x}"
        );
    }
}

/// Density: a 3x target (24 physical for an 8-unit logical packet) gets a
/// ~3x denser raster; the 1x control rasterizes at logical size.
#[test]
fn svg_texture_is_rasterized_at_logical_size_times_density() {
    let Some(mut r1) = renderer() else { return };
    let url = fixture("density.svg", HALF_BLUE.as_bytes());
    let Some(_) = draw(&mut r1, &sprite_packet(&url, 8.0), 8) else {
        return;
    };
    assert_eq!(
        r1.svg_texture_sizes(),
        vec![(8, 8)],
        "1x: 8 logical px / 4 units = scale 2 -> 8px texture"
    );

    let mut r3 = renderer().expect("adapter present");
    let Some(rgba) = draw(&mut r3, &sprite_packet(&url, 8.0), 24) else {
        return;
    };
    let sizes = r3.svg_texture_sizes();
    assert_eq!(sizes.len(), 1);
    assert!(
        sizes[0].0 >= 24 && sizes[0].0 < 24 * 12 / 10 + 2,
        "3x: >= physical 24px, bucketed, got {sizes:?}"
    );
    // Bucketed texture is up to ~19% over the screen size, so the edge is
    // bilinear-soft within a texel or two; the interiors are exact.
    let inside = px(&rgba, 24, 9, 12);
    let outside = px(&rgba, 24, 15, 12);
    assert!(
        inside[2] > 240 && outside[3] < 8,
        "3x: svg edge near physical midpoint: {inside:?} | {outside:?}"
    );
}

#[test]
fn svg_raster_is_cached_per_size_and_reused_across_frames() {
    let Some(mut r) = renderer() else { return };
    let url = fixture("cache.svg", HALF_BLUE.as_bytes());
    let packet = sprite_packet(&url, 8.0);
    let Some(_) = draw(&mut r, &packet, 8) else {
        return;
    };
    let _ = draw(&mut r, &packet, 8);
    assert_eq!(
        r.svg_texture_sizes().len(),
        1,
        "repeat frame hits the cache"
    );
    assert_eq!(
        orbital_canvas_paint::svg::parse_count(&url),
        1,
        "tree parsed once"
    );
    // Same bucket (8.0 -> 8.2 stays under the next quarter-octave): still a hit.
    let _ = draw(&mut r, &sprite_packet(&url, 8.2), 8);
    assert_eq!(
        r.svg_texture_sizes().len(),
        1,
        "sub-bucket zoom reuses the texture"
    );
    // Control: a genuinely larger draw adds exactly one more texture.
    let _ = draw(&mut r, &sprite_packet(&url, 16.0), 16);
    assert_eq!(r.svg_texture_sizes().len(), 2);
    assert_eq!(
        orbital_canvas_paint::svg::parse_count(&url),
        1,
        "still one parse"
    );
}

#[test]
fn svg_rasters_per_url_are_bounded_and_evict_least_recently_used() {
    let Some(mut r) = renderer() else { return };
    let url = fixture("lru.svg", HALF_BLUE.as_bytes());
    let other = fixture("lru-other.svg", HALF_BLUE.as_bytes());
    // Logical sizes 8,16,32,64 -> four distinct textures (the bound).
    for logical in [8.0f32, 16.0, 32.0, 64.0] {
        let px = logical as u32;
        let _ = draw(&mut r, &sprite_packet(&url, logical), px);
    }
    assert_eq!(r.svg_texture_sizes().len(), MAX_SVG_RASTERS_PER_URL);
    // Another URL has its own budget.
    let _ = draw(&mut r, &sprite_packet(&other, 8.0), 8);
    assert_eq!(r.svg_texture_sizes().len(), MAX_SVG_RASTERS_PER_URL + 1);
    // Touch the oldest (8) so 16 becomes least recently used, then add a fifth size.
    let _ = draw(&mut r, &sprite_packet(&url, 8.0), 8);
    let _ = draw(&mut r, &sprite_packet(&url, 128.0), 128);
    let mut mine: Vec<_> = r
        .sprites
        .keys()
        .filter(|k| k.url == url)
        .filter_map(|k| k.raster)
        .collect();
    mine.sort_unstable();
    assert_eq!(
        mine,
        vec![(8, 8), (32, 32), (64, 64), (128, 128)],
        "16 was least recently used (8 was touched), so it is the one evicted"
    );
    assert_eq!(
        r.sprites.keys().filter(|k| k.url == other).count(),
        1,
        "other URL untouched by this URL's eviction"
    );
    // The evicted size re-rasterizes on demand (eviction is a cache miss, not a failure).
    let rgba = draw(&mut r, &sprite_packet(&url, 16.0), 16).expect("re-rasterize");
    assert!(px(&rgba, 16, 2, 8)[2] > 200, "evicted size draws again");
}

/// Control: a PNG keeps its single native-size texture (no SVG path).
#[test]
fn png_sprite_still_decodes_once_at_native_size() {
    let Some(mut r) = renderer() else { return };
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("encode png");
    let url = fixture("native.png", png.get_ref());
    let Some(rgba) = draw(&mut r, &sprite_packet(&url, 8.0), 24) else {
        return;
    };
    assert!(r.svg_texture_sizes().is_empty(), "no svg raster for a png");
    assert_eq!(r.sprites.len(), 1);
    let c = px(&rgba, 24, 12, 12);
    assert!(c[2] > 200 && c[0] < 60, "png drawn, got {c:?}");
    let _ = draw(&mut r, &sprite_packet(&url, 8.0), 24);
    assert_eq!(r.sprites.len(), 1, "png cache hit on repeat");
}
