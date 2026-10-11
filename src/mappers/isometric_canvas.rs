//! Mapper for the `isometric-canvas` pattern.

use crate::packet::{DrawCommand, GameRenderPacket, Rect, Tile, Vec2};

/// Build a display-list packet from `isometric-canvas` props.
pub fn from_props(props: &serde_json::Value) -> GameRenderPacket {
    let width = get_f32(props, "width", 640.0);
    let height = get_f32(props, "height", 480.0);
    let tile_w = get_f32(props, "tileWidth", 64.0);
    let tile_h = get_f32(props, "tileHeight", 32.0);

    let mut draw_commands = vec![DrawCommand::Clear {
        color: [0.06, 0.09, 0.12, 1.0],
    }];

    // Collect tiles from props or synthesise a 6×10 default grid.
    let mut tile_list: Vec<Tile> = Vec::new();
    if let Some(serde_json::Value::Array(arr)) = props.get("tiles") {
        for t in arr.iter().take(128) {
            if let (Some(r), Some(c)) = (
                t.get("row").and_then(|v| v.as_u64()),
                t.get("col").and_then(|v| v.as_u64()),
            ) {
                tile_list.push(Tile {
                    row: r as u32,
                    col: c as u32,
                    kind: t
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("grass")
                        .to_string(),
                });
            }
        }
    }
    let (rows, cols) = if tile_list.is_empty() {
        for r in 0u32..6 {
            for c in 0u32..10 {
                tile_list.push(Tile {
                    row: r,
                    col: c,
                    kind: "grass".to_string(),
                });
            }
        }
        (6u32, 10u32)
    } else {
        let rows = tile_list.iter().map(|t| t.row).max().unwrap_or(0) + 1;
        let cols = tile_list.iter().map(|t| t.col).max().unwrap_or(0) + 1;
        (rows, cols)
    };

    // Isometric projection: draw each tile as a coloured quad.
    for tile in &tile_list {
        let iso_x = (tile.col as f32 - tile.row as f32) * tile_w / 2.0 + width / 2.0;
        let iso_y = (tile.col as f32 + tile.row as f32) * tile_h / 2.0;
        let color = match tile.kind.as_str() {
            "water" => [0.18, 0.38, 0.70, 1.0],
            "sand" => [0.80, 0.72, 0.44, 1.0],
            "rock" => [0.48, 0.44, 0.40, 1.0],
            _ => [0.22, 0.56, 0.22, 1.0],
        };
        draw_commands.push(DrawCommand::Quad {
            rect: Rect {
                x: iso_x,
                y: iso_y,
                w: tile_w - 2.0,
                h: tile_h - 2.0,
            },
            color,
        });
    }

    // Grid size label.
    draw_commands.push(DrawCommand::Text {
        text: format!("{}×{} tiles", cols, rows),
        pos: Vec2 { x: 8.0, y: 8.0 },
        size: 12.0,
        color: [0.8, 0.8, 0.8, 1.0],
        align: Default::default(),
        weight: 400,
        mono: false,
        halo: None,
    });

    // Units as yellow dots.
    if let Some(serde_json::Value::Array(units)) = props.get("units") {
        for u in units.iter().take(32) {
            let ux = get_f32(u, "x", 0.0);
            let uy = get_f32(u, "y", 0.0);
            draw_commands.push(DrawCommand::Quad {
                rect: Rect {
                    x: ux - 4.0,
                    y: uy - 4.0,
                    w: 8.0,
                    h: 8.0,
                },
                color: [1.0, 0.85, 0.0, 1.0],
            });
        }
    }

    // Loading overlay.
    if props
        .get("isLoading")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        draw_commands.push(DrawCommand::Quad {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: width,
                h: height,
            },
            color: [0.0, 0.0, 0.0, 0.5],
        });
        draw_commands.push(DrawCommand::Text {
            text: "Loading\u{2026}".to_string(),
            pos: Vec2 {
                x: width / 2.0 - 30.0,
                y: height / 2.0,
            },
            size: 18.0,
            color: [1.0, 1.0, 1.0, 1.0],
            align: Default::default(),
            weight: 400,
            mono: false,
            halo: None,
        });
    }

    GameRenderPacket {
        pattern: "isometric-canvas".into(),
        width,
        height,
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
