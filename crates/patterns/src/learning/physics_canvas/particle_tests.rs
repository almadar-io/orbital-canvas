use super::*;
use orbital_canvas_base::packet::{DrawCommand, ParticleSource};
use serde_json::json;

fn word(x: u32, y: u32, palette: u32) -> [u8; 4] {
    (x | (y << 12) | (palette << 24)).to_le_bytes()
}

#[test]
fn packed_particles_draw_right_after_the_ground_in_scene_pixels() {
    use base64::Engine;
    let bytes: Vec<u8> = [word(10, 20, 0), word(30, 40, 1)].concat();
    let p = json!({"width": 600, "height": 400, "particles": {
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes), "stride": 4, "radius": 2, "palette": ["#ff0000", "#00ff00"]
    }});
    let scene = map(&p, Some((300.0, 200.0))).unwrap();
    match &scene.packet.draw_commands[..2] {
        [DrawCommand::Clear { .. }, DrawCommand::Particles {
            source: ParticleSource::Bytes { data },
            stride: 4,
            radius,
            palette,
            scale,
            ..
        }] => {
            assert_eq!(data, &bytes);
            assert_eq!(
                (*radius, *scale),
                (2.0, 0.5),
                "a 600-wide scene in a 300-wide view draws at half scale"
            );
            assert_eq!(palette.len(), 2);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_gpu_buffer_handle_is_the_source_and_control_both_or_neither_is_refused() {
    let gpu = json!({"particles": {"buffer": {"id": 7, "device": 3, "size": 64}, "stride": 16, "radius": 1, "palette": ["#fff"]}});
    let scene = map(&gpu, None).unwrap();
    assert!(scene.packet.draw_commands.iter().any(|c| matches!(
        c,
        DrawCommand::Particles {
            source: ParticleSource::Gpu { id: 7, device: 3 },
            ..
        }
    )));
    let both = json!({"particles": {"data": "", "buffer": {"id": 7, "device": 3}, "stride": 16, "radius": 1, "palette": ["#fff"]}});
    assert!(map(&both, None).is_err());
    let neither = json!({"particles": {"stride": 16, "radius": 1, "palette": ["#fff"]}});
    assert!(map(&neither, None).is_err());
}
