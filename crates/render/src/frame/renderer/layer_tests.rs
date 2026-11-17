use super::svg_sprite_tests::{px, renderer};
use super::*;
use orbital_canvas_base::packet::{SdfBackdrop, SdfKind, SdfMaterial};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const CLEAR: [f32; 4] = [0.0; 4];

fn quad(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> DrawCommand {
    DrawCommand::Quad {
        rect: Rect { x, y, w, h },
        color,
    }
}

fn sdf(rect: Rect, fill: [f32; 4], material: Option<SdfMaterial>) -> DrawCommand {
    DrawCommand::Sdf {
        kind: SdfKind::Rect,
        rect,
        corners: [0.0; 4],
        corner_kind: 0,
        arc: [0.0; 3],
        polygon: [0.0; 2],
        fill,
        stroke: None,
        shadow: None,
        rotation: 0.0,
        opacity: 1.0,
        material,
    }
}

fn glass(
    blur: f32,
    tint: Option<[f32; 4]>,
    saturate: f32,
    material: Option<SdfMaterial>,
) -> DrawCommand {
    DrawCommand::SdfBackdrop {
        shape: Box::new(sdf(
            Rect {
                x: 16.0,
                y: 16.0,
                w: 32.0,
                h: 32.0,
            },
            CLEAR,
            material,
        )),
        backdrop: SdfBackdrop {
            blur,
            tint,
            saturate,
        },
    }
}

fn frame(commands: Vec<DrawCommand>) -> GameRenderPacket {
    GameRenderPacket {
        pattern: "canvas-2d".into(),
        width: 64.0,
        height: 64.0,
        draw_commands: commands,
        ..Default::default()
    }
}

fn halves() -> Vec<DrawCommand> {
    vec![
        quad(0.0, 0.0, 32.0, 64.0, WHITE),
        quad(32.0, 0.0, 32.0, 64.0, BLACK),
    ]
}

/// Render into a physical `px` x `px` target and read it back; a frame may paint nothing, so no draw is asserted.
fn draw(r: &mut SharedDeviceRenderer, packet: &GameRenderPacket, px: u32) -> Option<Vec<u8>> {
    let target = r.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("layer test target"),
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
    r.render_packet_to_texture(packet, &target);
    let encoder = r
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("readback"),
        });
    copy_texture_to_rgba(r.device(), r.queue(), encoder, &target, px, px).ok()
}

fn at(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    px(rgba, 64, x, y)
}

fn material(wgsl: &str, targets: &[&str]) -> SdfMaterial {
    SdfMaterial {
        wgsl: wgsl.into(),
        params: vec![],
        live: false,
        since: 0.0,
        targets: targets.iter().map(|t| (*t).to_owned()).collect(),
    }
}

#[test]
fn a_backdrop_blurs_what_is_behind_it_inside_the_shape_only() {
    let Some(mut r) = renderer() else { return };
    let mut commands = halves();
    commands.push(glass(8.0, None, 1.0, None));
    let Some(blurred) = draw(&mut r, &frame(commands), 64) else {
        return;
    };
    let edge = at(&blurred, 31, 32);
    assert!(
        edge[0] > 60 && edge[0] < 200,
        "the hard edge is softened inside the glass: {edge:?}"
    );
    let inner = at(&blurred, 22, 32);
    assert!(
        inner[0] > edge[0] + 30,
        "further from the edge it stays whiter: {inner:?} vs {edge:?}"
    );
    assert_eq!(
        at(&blurred, 8, 32),
        [255, 255, 255, 255],
        "outside the shape is untouched"
    );
    assert_eq!(
        at(&blurred, 56, 32),
        [0, 0, 0, 255],
        "outside the shape is untouched"
    );
    assert_eq!(at(&blurred, 31, 10)[0], 255, "above the glass is untouched");
    let mut plain = halves();
    plain.push(sdf(
        Rect {
            x: 16.0,
            y: 16.0,
            w: 32.0,
            h: 32.0,
        },
        CLEAR,
        None,
    ));
    let Some(sharp) = draw(&mut r, &frame(plain), 64) else {
        return;
    };
    assert_eq!(
        at(&sharp, 31, 32),
        [255, 255, 255, 255],
        "control: a plain shape leaves the edge sharp"
    );
    assert_eq!(
        at(&sharp, 32, 32),
        [0, 0, 0, 255],
        "control: a plain shape leaves the edge sharp"
    );
}

#[test]
fn a_backdrop_only_blurs_what_was_painted_before_it() {
    let Some(mut r) = renderer() else { return };
    let mut commands = halves();
    commands.push(glass(8.0, None, 1.0, None));
    commands.push(quad(20.0, 20.0, 8.0, 8.0, RED));
    let Some(pixels) = draw(&mut r, &frame(commands), 64) else {
        return;
    };
    assert_eq!(
        at(&pixels, 24, 24),
        [255, 0, 0, 255],
        "a mark painted after the glass stays sharp on top"
    );
    let mut before = halves();
    before.push(quad(20.0, 20.0, 8.0, 8.0, RED));
    before.push(glass(8.0, None, 1.0, None));
    let Some(under) = draw(&mut r, &frame(before), 64) else {
        return;
    };
    let blurred = at(&under, 24, 24);
    assert!(
        blurred[0] > 100 && blurred[1] > 100,
        "control: a mark painted before the glass is blurred into the white behind it: {blurred:?}"
    );
}

#[test]
fn saturate_and_tint_shape_the_blurred_backdrop() {
    let Some(mut r) = renderer() else { return };
    let red = || vec![quad(0.0, 0.0, 64.0, 64.0, RED)];
    let with = |blur: f32, tint, saturate| {
        let mut commands = red();
        commands.push(glass(blur, tint, saturate, None));
        commands
    };
    let Some(gray) = draw(&mut r, &frame(with(0.0, None, 0.0)), 64) else {
        return;
    };
    let g = at(&gray, 32, 32);
    assert!(
        g[0].abs_diff(g[1]) < 6 && g[1].abs_diff(g[2]) < 6,
        "saturate 0 is gray: {g:?}"
    );
    assert!(g[0] > 40 && g[0] < 70, "at the red's luminance: {g:?}");
    let Some(kept) = draw(&mut r, &frame(with(0.0, None, 1.0)), 64) else {
        return;
    };
    assert_eq!(
        at(&kept, 32, 32),
        [255, 0, 0, 255],
        "control: saturate 1 keeps the red"
    );
    let Some(tinted) = draw(
        &mut r,
        &frame(with(0.0, Some([0.0, 0.0, 1.0, 1.0]), 1.0)),
        64,
    ) else {
        return;
    };
    assert_eq!(
        at(&tinted, 32, 32),
        [0, 0, 255, 255],
        "an opaque tint covers the backdrop"
    );
    let Some(half) = draw(
        &mut r,
        &frame(with(0.0, Some([0.0, 0.0, 1.0, 0.5]), 1.0)),
        64,
    ) else {
        return;
    };
    let h = at(&half, 32, 32);
    assert!(
        h[0] > 100 && h[2] > 100 && h[1] < 10,
        "a half tint mixes: {h:?}"
    );
    assert_eq!(
        at(&half, 4, 4),
        [255, 0, 0, 255],
        "outside the glass is untouched"
    );
}

#[test]
fn a_material_reads_behind_and_it_is_zero_without_a_backdrop() {
    let Some(mut r) = renderer() else { return };
    let wgsl = "fn shade(px: Pixel) -> vec4<f32> { return vec4<f32>(px.behind.rgb, 1.0); }";
    let mut over = vec![quad(0.0, 0.0, 64.0, 64.0, RED)];
    over.push(glass(0.0, None, 1.0, Some(material(wgsl, &[]))));
    let Some(with) = draw(&mut r, &frame(over), 64) else {
        return;
    };
    assert_eq!(
        at(&with, 32, 32),
        [255, 0, 0, 255],
        "the material paints what is behind"
    );
    let mut without = vec![quad(0.0, 0.0, 64.0, 64.0, RED)];
    without.push(sdf(
        Rect {
            x: 16.0,
            y: 16.0,
            w: 32.0,
            h: 32.0,
        },
        CLEAR,
        Some(material(wgsl, &[])),
    ));
    let Some(zero) = draw(&mut r, &frame(without), 64) else {
        return;
    };
    assert_eq!(
        at(&zero, 32, 32),
        [0, 0, 0, 255],
        "control: no backdrop, behind is zero"
    );
}

fn target(id: &str, color: [f32; 4]) -> DrawCommand {
    DrawCommand::Target {
        id: id.into(),
        width: 16,
        height: 16,
        live: false,
        commands: vec![quad(0.0, 0.0, 16.0, 16.0, color)],
    }
}

fn reader(id: &str) -> DrawCommand {
    DrawCommand::Sprite {
        texture: format!("target:{id}"),
        src: Rect::default(),
        dst: Rect {
            x: 8.0,
            y: 8.0,
            w: 16.0,
            h: 16.0,
        },
        rotation: 0.0,
        opacity: 1.0,
        flip_x: false,
        flip_y: false,
        tint: None,
    }
}

#[test]
fn a_target_paints_nothing_itself_and_a_sprite_paints_it() {
    let Some(mut r) = renderer() else { return };
    let Some(alone) = draw(&mut r, &frame(vec![target("t", GREEN)]), 64) else {
        return;
    };
    assert_eq!(
        at(&alone, 16, 16)[3],
        0,
        "control: a target alone paints nothing in the frame"
    );
    let Some(shown) = draw(&mut r, &frame(vec![target("t", GREEN), reader("t")]), 64) else {
        return;
    };
    assert_eq!(
        at(&shown, 16, 16),
        [0, 255, 0, 255],
        "the sprite paints the target"
    );
    assert_eq!(at(&shown, 4, 4)[3], 0, "outside the sprite is empty");
    assert_eq!(at(&shown, 30, 30)[3], 0, "outside the sprite is empty");
}

#[test]
fn a_reader_before_its_writer_reads_the_previous_frame() {
    let Some(mut r) = renderer() else { return };
    let packet = frame(vec![reader("t"), target("t", GREEN)]);
    let Some(first) = draw(&mut r, &packet, 64) else {
        return;
    };
    assert_eq!(
        at(&first, 16, 16)[3],
        0,
        "the first frame has no earlier texture to read"
    );
    let Some(second) = draw(&mut r, &packet, 64) else {
        return;
    };
    assert_eq!(
        at(&second, 16, 16),
        [0, 255, 0, 255],
        "the next frame reads what the writer left"
    );
    let Some(ordered) = draw(&mut r, &frame(vec![target("u", GREEN), reader("u")]), 64) else {
        return;
    };
    assert_eq!(
        at(&ordered, 16, 16),
        [0, 255, 0, 255],
        "control: a writer before its reader is read at once"
    );
}

#[test]
fn a_reader_before_its_writer_sees_the_old_content_while_the_writer_repaints() {
    let Some(mut r) = renderer() else { return };
    let Some(_) = draw(&mut r, &frame(vec![target("t", GREEN)]), 64) else {
        return;
    };
    let Some(late) = draw(&mut r, &frame(vec![reader("t"), target("t", RED)]), 64) else {
        return;
    };
    assert_eq!(
        at(&late, 16, 16),
        [0, 255, 0, 255],
        "the reader before the writer shows last frame's green"
    );
    let Some(now) = draw(&mut r, &frame(vec![target("t", RED), reader("t")]), 64) else {
        return;
    };
    assert_eq!(
        at(&now, 16, 16),
        [255, 0, 0, 255],
        "control: after the writer it shows the new red"
    );
}

#[test]
fn a_target_repaints_when_its_children_change_and_keeps_its_texture_when_they_do_not() {
    let Some(mut r) = renderer() else { return };
    let Some(green) = draw(&mut r, &frame(vec![target("t", GREEN), reader("t")]), 64) else {
        return;
    };
    assert_eq!(at(&green, 16, 16), [0, 255, 0, 255]);
    let Some(again) = draw(&mut r, &frame(vec![target("t", GREEN), reader("t")]), 64) else {
        return;
    };
    assert_eq!(
        at(&again, 16, 16),
        [0, 255, 0, 255],
        "unchanged children keep the texture"
    );
    let Some(red) = draw(&mut r, &frame(vec![target("t", RED), reader("t")]), 64) else {
        return;
    };
    assert_eq!(
        at(&red, 16, 16),
        [255, 0, 0, 255],
        "changed children repaint it"
    );
}

#[test]
fn a_material_samples_its_target_as_target_a() {
    let Some(mut r) = renderer() else { return };
    let wgsl = "fn shade(px: Pixel) -> vec4<f32> { return textureSampleLevel(target_a, target_sampler, px.uv, 0.0); }";
    let left = Rect {
        x: 0.0,
        y: 32.0,
        w: 16.0,
        h: 16.0,
    };
    let right = Rect {
        x: 32.0,
        y: 32.0,
        w: 16.0,
        h: 16.0,
    };
    let both = frame(vec![
        target("g", GREEN),
        target("r", RED),
        sdf(left, WHITE, Some(material(wgsl, &["g"]))),
        sdf(right, WHITE, Some(material(wgsl, &["r"]))),
    ]);
    let Some(pixels) = draw(&mut r, &both, 64) else {
        return;
    };
    assert_eq!(
        at(&pixels, 8, 40),
        [0, 255, 0, 255],
        "the first shape samples the green target"
    );
    assert_eq!(
        at(&pixels, 40, 40),
        [255, 0, 0, 255],
        "the next shape, same WGSL, samples the red one"
    );
    let none = frame(vec![
        target("g", GREEN),
        sdf(left, WHITE, Some(material(wgsl, &[]))),
        sdf(right, WHITE, Some(material(wgsl, &["nope"]))),
    ]);
    let Some(empty) = draw(&mut r, &none, 64) else {
        return;
    };
    assert_eq!(
        at(&empty, 8, 40)[3],
        0,
        "control: no targets, nothing sampled"
    );
    assert_eq!(
        at(&empty, 40, 40)[3],
        0,
        "control: an unknown target, nothing sampled"
    );
}

#[test]
fn a_target_is_allocated_at_the_density_of_the_pass_that_paints_it() {
    let Some(mut r) = renderer() else { return };
    let Some(_) = draw(&mut r, &frame(vec![target("t", GREEN), reader("t")]), 128) else {
        return;
    };
    let dense = r.targets.read("t").expect("the target was painted");
    assert_eq!((dense.width, dense.height), (32, 32), "a 16-point target on a 2x pass holds 32 device pixels");
    let Some(shown) = draw(&mut r, &frame(vec![target("t", GREEN), reader("t")]), 128) else {
        return;
    };
    assert_eq!(px(&shown, 128, 32, 32), [0, 255, 0, 255], "the sprite still paints the whole target at logical size");
    let Some(_) = draw(&mut r, &frame(vec![target("u", GREEN), reader("u")]), 64) else {
        return;
    };
    let plain = r.targets.read("u").expect("the target was painted");
    assert_eq!((plain.width, plain.height), (16, 16), "control: at 1x the target holds its logical size");
}