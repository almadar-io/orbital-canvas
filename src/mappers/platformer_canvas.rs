//! Mapper for the `platformer-canvas` pattern.

use crate::packet::{DrawCommand, GameRenderPacket, Rect, Vec2};

/// Build a display-list packet from `platformer-canvas` props.
pub fn from_props(props: &serde_json::Value) -> GameRenderPacket {
    let canvas_w = get_f32(props, "canvasWidth", 800.0);
    let canvas_h = get_f32(props, "canvasHeight", 480.0);
    let world_w = get_f32(props, "worldWidth", canvas_w);

    let mut draw_commands = vec![DrawCommand::Clear {
        color: [0.34, 0.54, 0.89, 1.0], // sky blue
    }];

    // Ground plane.
    let ground_y = canvas_h * 0.75;
    draw_commands.push(DrawCommand::Quad {
        rect: Rect {
            x: 0.0,
            y: ground_y,
            w: canvas_w,
            h: canvas_h - ground_y,
        },
        color: [0.24, 0.52, 0.18, 1.0],
    });

    // Platforms from props array.
    if let Some(serde_json::Value::Array(plats)) = props.get("platforms") {
        for p in plats.iter().take(32) {
            let x = get_f32(p, "x", 0.0);
            let y = get_f32(p, "y", 0.0);
            let w = get_f32(p, "width", 80.0);
            let h = get_f32(p, "height", 16.0);
            draw_commands.push(DrawCommand::Quad {
                rect: Rect { x, y, w, h },
                color: [0.55, 0.35, 0.18, 1.0],
            });
        }
    }

    // Player from the `player` prop object.
    let player = props.get("player");
    let px = player
        .and_then(|p| p.get("x"))
        .and_then(|v| v.as_f64())
        .unwrap_or((canvas_w / 2.0) as f64) as f32;
    let py = player
        .and_then(|p| p.get("y"))
        .and_then(|v| v.as_f64())
        .unwrap_or((ground_y - 32.0) as f64) as f32;
    let pw = player
        .and_then(|p| p.get("width"))
        .and_then(|v| v.as_f64())
        .unwrap_or(24.0) as f32;
    let ph = player
        .and_then(|p| p.get("height"))
        .and_then(|v| v.as_f64())
        .unwrap_or(32.0) as f32;
    draw_commands.push(DrawCommand::Quad {
        rect: Rect {
            x: px,
            y: py,
            w: pw,
            h: ph,
        },
        color: [0.0, 0.71, 1.0, 1.0],
    });

    // World-width indicator line.
    if world_w > canvas_w {
        draw_commands.push(DrawCommand::Line {
            from: Vec2 {
                x: 0.0,
                y: canvas_h - 4.0,
            },
            to: Vec2 {
                x: canvas_w * (canvas_w / world_w),
                y: canvas_h - 4.0,
            },
            color: [1.0, 1.0, 0.0, 0.6],
            width: 3.0,
        });
        draw_commands.push(DrawCommand::Text {
            text: format!("viewport {:.0}/{:.0}px", canvas_w, world_w),
            pos: Vec2 {
                x: 8.0,
                y: canvas_h - 16.0,
            },
            size: 11.0,
            color: [1.0, 1.0, 0.8, 1.0],
            align: Default::default(),
            weight: 400,
            mono: false,
            halo: None,
        });
    }

    GameRenderPacket {
        pattern: "platformer-canvas".into(),
        width: canvas_w,
        height: canvas_h,
        background_image: None,
        asset_base_url: None,
        camera: None,
        draw_commands,
        tick_event: get_string(props, "tickEvent"),
        draw_event: get_string(props, "drawEvent"),
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

fn get_string(props: &serde_json::Value, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
