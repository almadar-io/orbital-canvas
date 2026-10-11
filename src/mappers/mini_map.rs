//! Mapper for the `mini-map` pattern.

use crate::packet::{DrawCommand, GameRenderPacket, Rect};

/// Build a display-list packet from `mini-map` props.
pub fn from_props(props: &serde_json::Value) -> GameRenderPacket {
    let size = get_f32(props, "size", 120.0);

    let mut draw_commands = vec![DrawCommand::Clear {
        color: [0.078, 0.157, 0.117, 1.0],
    }];

    // Viewport marker (small hollow rectangle in the center).
    let marker_size = size * 0.4;
    let marker_color = [0.3, 0.5, 0.4, 1.0];
    draw_commands.push(DrawCommand::Quad {
        rect: Rect {
            x: (size - marker_size) / 2.0,
            y: (size - marker_size) / 2.0,
            w: marker_size,
            h: 2.0,
        },
        color: marker_color,
    });
    draw_commands.push(DrawCommand::Quad {
        rect: Rect {
            x: (size - marker_size) / 2.0,
            y: (size + marker_size) / 2.0 - 2.0,
            w: marker_size,
            h: 2.0,
        },
        color: marker_color,
    });
    draw_commands.push(DrawCommand::Quad {
        rect: Rect {
            x: (size - marker_size) / 2.0,
            y: (size - marker_size) / 2.0,
            w: 2.0,
            h: marker_size,
        },
        color: marker_color,
    });
    draw_commands.push(DrawCommand::Quad {
        rect: Rect {
            x: (size + marker_size) / 2.0 - 2.0,
            y: (size - marker_size) / 2.0,
            w: 2.0,
            h: marker_size,
        },
        color: marker_color,
    });

    // Player / hero dot in the center.
    let dot_size = 8.0;
    draw_commands.push(DrawCommand::Quad {
        rect: Rect {
            x: (size - dot_size) / 2.0,
            y: (size - dot_size) / 2.0,
            w: dot_size,
            h: dot_size,
        },
        color: [1.0, 1.0, 0.0, 1.0],
    });

    GameRenderPacket {
        pattern: "mini-map".into(),
        width: size,
        height: size,
        background_image: None,
        asset_base_url: None,
        camera: None,
        draw_commands,
        tick_event: None,
        draw_event: None,
        fps: None,
        animations: Vec::new(),
    }
}

fn get_f32(props: &serde_json::Value, key: &str, default: f32) -> f32 {
    props
        .get(key)
        .and_then(|v| v.as_f64())
        .map(|f| f as f32)
        .unwrap_or(default)
}
