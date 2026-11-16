//! `algorithm-canvas` — port of `AlgorithmCanvas.tsx`: bars, slots, cells,
//! buckets and frames laid out as equal vertical panels (pointers, ranges,
//! auxBars, row/col labels decorate them) and derived into `LearningShape`s in
//! canvas pixels, drawn by [`canvas`].

use super::canvas::{self, CanvasSpec};
use super::shape::{parse_shapes, Align, Shape};
use crate::scene::MappedScene;
use orbital_canvas_map::error::CanvasSceneError;
use orbital_canvas_map::game_canvas_2d::get_f32;
use serde_json::Value;

const DEFAULT_BAR_COLOR: &str = "series-1";
const DEFAULT_CELL_COLOR: &str = "fill";
const DEFAULT_POINTER_COLOR: &str = "highlight";
const POINTER_BAND: f32 = 40.0;
const TOP_PAD: f32 = 26.0;

const RANGE_COLOR_DEFAULT: &str = "highlight";
const RANGE_FILL_OPACITY: f32 = 0.15;
const BRACKET_TOP_OFFSET: f32 = 16.0;
const BRACKET_ROW_H: f32 = 14.0;
const BRACKET_TICK_H: f32 = 6.0;
const BRACKET_LABEL_OFFSET: f32 = 6.0;

const FRAME_TWO_LINE_MIN_H: f32 = 22.0;
const FRAME_STRIP_H: f32 = 44.0;
const FRAME_GAP: f32 = 6.0;
const FRAME_BOTTOM_PAD: f32 = 8.0;
const FRAME_MIN_H: f32 = 12.0;

const BUCKET_ENTRY_MIN_W: f32 = 24.0;
const BUCKET_ENTRY_MAX_W: f32 = 64.0;

const CORNER_MIN_CELL: f32 = 28.0;
const CORNER_INSET_X: f32 = 3.0;
const CORNER_INSET_Y: f32 = 6.0;

const AUX_PRIMARY_RATIO: f32 = 0.6;
const AUX_LABEL_BAND: f32 = 18.0;
const AUX_BASELINE_PAD: f32 = 8.0;

fn list<'a>(props: &'a Value, key: &str) -> &'a [Value] {
    props
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn num(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

/// A present string field (JS `??` keeps an empty string).
fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// JS `String(n)` for a finite number.
fn js_num(n: f32) -> String {
    let n = f64::from(n);
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// `Number.isFinite(v) ? v : 0`.
fn finite_value(bar: &Value) -> f32 {
    num(bar, "value").filter(|n| n.is_finite()).unwrap_or(0.0)
}

fn max_value(bars: &[Value]) -> f32 {
    bars.iter().map(finite_value).fold(1.0, f32::max)
}

fn t(x: f32, y: f32, s: impl Into<String>, size: &str, align: Align) -> Shape {
    Shape::text(x, y, s).text_size(size).align(align)
}

fn tone_text(x: f32, y: f32, s: impl Into<String>, tone: &str, size: &str, align: Align) -> Shape {
    t(x, y, s, size, align).tone(tone)
}

fn color_text(
    x: f32,
    y: f32,
    s: impl Into<String>,
    color: &str,
    size: &str,
    align: Align,
) -> Shape {
    t(x, y, s, size, align).color(color)
}

fn tinted(x: f32, y: f32, w: f32, h: f32, id: String, color: &str) -> Shape {
    Shape::rect(x, y, w, h)
        .id(id)
        .color(color)
        .fill(color)
        .fill_style("tint")
        .stroke_weight("normal")
}

fn bar_strip(
    bars: &[Value],
    id_prefix: &str,
    baseline: f32,
    usable_h: f32,
    width: f32,
    out: &mut Vec<Shape>,
) {
    let slot = width / bars.len() as f32;
    let max_v = max_value(bars);
    for (i, bar) in bars.iter().enumerate() {
        let v = finite_value(bar);
        let bh = ((v / max_v) * usable_h).max(0.0);
        let x = i as f32 * slot + slot * 0.1;
        let w = slot * 0.8;
        let color = text(bar, "color").unwrap_or_else(|| DEFAULT_BAR_COLOR.into());
        out.push(tinted(
            x,
            baseline - bh,
            w,
            bh,
            format!("{id_prefix}-{i}"),
            &color,
        ));
        let label = text(bar, "label").or_else(|| (bars.len() <= 24).then(|| js_num(v)));
        if let Some(label) = label.filter(|l| !l.is_empty()) {
            out.push(tone_text(
                x + w / 2.0,
                baseline - bh - 8.0,
                label,
                "label",
                "xs",
                Align::Center,
            ));
        }
    }
}

struct Pointer {
    index: f32,
    label: Option<String>,
    color: Option<String>,
}

fn merged_pointers(pointers: &[Value]) -> Vec<Pointer> {
    let mut out: Vec<Pointer> = Vec::new();
    for p in pointers {
        let index = num(p, "index").unwrap_or(f32::NAN);
        let label = text(p, "label");
        match out.iter_mut().find(|q| q.index == index) {
            Some(prev) => {
                let joined: Vec<String> = [prev.label.clone(), label]
                    .into_iter()
                    .flatten()
                    .filter(|l| !l.is_empty())
                    .collect();
                prev.label = (!joined.is_empty()).then(|| joined.join(" · "));
            }
            None => out.push(Pointer {
                index,
                label: label.filter(|l| !l.is_empty()),
                color: text(p, "color"),
            }),
        }
    }
    out
}

/// The shapes `AlgorithmCanvas` derives, in its paint order.
pub fn derive_shapes(props: &Value, width: f32, height: f32) -> Vec<Shape> {
    let bars = list(props, "bars");
    let cells = list(props, "cells");
    let pointers = list(props, "pointers");
    let ranges = list(props, "ranges");
    let slots = list(props, "slots");
    let frames = list(props, "frames");
    let buckets = list(props, "buckets");
    let aux_bars = list(props, "auxBars");
    let row_labels = list(props, "rowLabels");
    let col_labels = list(props, "colLabels");
    let vertical = props.get("slotOrientation").and_then(Value::as_str) == Some("vertical");

    let mut out: Vec<Shape> = Vec::new();

    let presence = [
        !bars.is_empty(),
        !slots.is_empty(),
        !cells.is_empty(),
        !buckets.is_empty(),
        !frames.is_empty(),
    ];
    let panel_count = presence.iter().filter(|p| **p).count();
    let panel_height = height / panel_count.max(1) as f32;
    let mut panel_y = [0.0f32; 5];
    let mut compact = 0.0;
    for (i, present) in presence.iter().enumerate() {
        if *present {
            panel_y[i] = compact * panel_height;
            compact += 1.0;
        }
    }
    let [panel_y_bars, panel_y_slots, panel_y_cells, panel_y_buckets, panel_y_frames] = panel_y;

    if !bars.is_empty() {
        let slot = width / bars.len() as f32;

        let bracket_ranges: Vec<&Value> = ranges
            .iter()
            .filter(|r| r.get("kind").and_then(Value::as_str) == Some("bracket"))
            .collect();
        let bracket_headroom = if bracket_ranges.is_empty() {
            0.0
        } else {
            BRACKET_TOP_OFFSET + bracket_ranges.len() as f32 * BRACKET_ROW_H
        };

        let has_aux = !aux_bars.is_empty();
        let primary_h = if has_aux {
            panel_height * AUX_PRIMARY_RATIO
        } else {
            panel_height
        };

        let baseline = panel_y_bars + primary_h - POINTER_BAND;
        let usable_h = baseline - (panel_y_bars + TOP_PAD + bracket_headroom);
        bar_strip(bars, "bar", baseline, usable_h, width, &mut out);

        for r in ranges {
            if r.get("kind").and_then(Value::as_str).unwrap_or("fill") != "fill" {
                continue;
            }
            let (from, to) = (
                num(r, "from").unwrap_or(f32::NAN),
                num(r, "to").unwrap_or(f32::NAN),
            );
            let color = text(r, "color").unwrap_or_else(|| RANGE_COLOR_DEFAULT.into());
            out.push(
                Shape::rect(
                    from * slot,
                    panel_y_bars,
                    (to - from + 1.0) * slot,
                    primary_h,
                )
                .color(color.clone())
                .fill(color)
                .fill_style("solid")
                .stroke_weight("thin")
                .opacity(RANGE_FILL_OPACITY),
            );
            if let Some(label) = text(r, "label").filter(|l| !l.is_empty()) {
                out.push(tone_text(
                    from * slot + 4.0,
                    panel_y_bars + 10.0 + bracket_headroom,
                    label,
                    "label",
                    "xs",
                    Align::Left,
                ));
            }
        }

        for (i, r) in bracket_ranges.iter().enumerate() {
            let (from, to) = (
                num(r, "from").unwrap_or(f32::NAN),
                num(r, "to").unwrap_or(f32::NAN),
            );
            let bracket_y = panel_y_bars + BRACKET_TOP_OFFSET + i as f32 * BRACKET_ROW_H;
            let x1 = from * slot + slot * 0.1;
            let x2 = (to + 1.0) * slot - slot * 0.1;
            let color = text(r, "color").unwrap_or_else(|| RANGE_COLOR_DEFAULT.into());
            out.push(
                Shape::line(x1, bracket_y, x2, bracket_y)
                    .color(color.clone())
                    .stroke_weight("normal"),
            );
            out.push(
                Shape::line(x1, bracket_y, x1, bracket_y + BRACKET_TICK_H)
                    .color(color.clone())
                    .stroke_weight("normal"),
            );
            out.push(
                Shape::line(x2, bracket_y, x2, bracket_y + BRACKET_TICK_H)
                    .color(color)
                    .stroke_weight("normal"),
            );
            if let Some(label) = text(r, "label").filter(|l| !l.is_empty()) {
                out.push(tone_text(
                    (x1 + x2) / 2.0,
                    bracket_y - BRACKET_LABEL_OFFSET,
                    label,
                    "label",
                    "xs",
                    Align::Center,
                ));
            }
        }

        if has_aux {
            let aux_h = panel_height - primary_h;
            let aux_baseline = panel_y_bars + primary_h + aux_h - AUX_BASELINE_PAD;
            let aux_usable_h = aux_baseline - (panel_y_bars + primary_h + AUX_LABEL_BAND);
            bar_strip(
                aux_bars,
                "auxbar",
                aux_baseline,
                aux_usable_h,
                width,
                &mut out,
            );
        }

        for p in merged_pointers(pointers) {
            if p.index < 0.0 || p.index >= bars.len() as f32 || p.index.is_nan() {
                continue;
            }
            let cx = p.index * slot + slot / 2.0;
            let color = p.color.unwrap_or_else(|| DEFAULT_POINTER_COLOR.into());
            out.push(
                Shape::arrow(cx, baseline + 15.0, cx, baseline + 3.0)
                    .color(color.clone())
                    .stroke_weight("normal"),
            );
            if let Some(label) = p.label {
                out.push(color_text(
                    cx,
                    panel_y_bars + primary_h - 10.0,
                    label,
                    &color,
                    "sm",
                    Align::Center,
                ));
            }
        }
    }

    if !slots.is_empty() {
        let n = slots.len() as f32;
        let v_box_h = panel_height / n;
        let v_box_w = (width * 0.5).min(120.0);
        let v_box_x = (width - v_box_w) / 2.0;
        let h_cell_w = width / n;
        let h_box_w = h_cell_w * 0.82;
        let h_box_h = (panel_height * 0.6).min(48.0);
        let h_box_y = panel_y_slots + (panel_height - h_box_h) / 2.0;

        let slot_box = |i: usize| -> (f32, f32, f32, f32) {
            if vertical {
                (
                    v_box_x,
                    panel_y_slots + panel_height - (i as f32 + 1.0) * v_box_h,
                    v_box_w,
                    v_box_h,
                )
            } else {
                (
                    i as f32 * h_cell_w + (h_cell_w - h_box_w) / 2.0,
                    h_box_y,
                    h_box_w,
                    h_box_h,
                )
            }
        };

        for (i, s) in slots.iter().enumerate() {
            let (bx, by, bw, bh) = slot_box(i);
            let state = s.get("state").and_then(Value::as_str).unwrap_or("filled");
            let color = text(s, "color");
            let fill = match state {
                "empty" => "fill".to_string(),
                "highlight" => color.unwrap_or_else(|| "highlight".into()),
                _ => color.unwrap_or_else(|| DEFAULT_BAR_COLOR.into()),
            };
            let stroke = if state == "empty" { "guide" } else { "ink" };
            out.push(
                Shape::rect(bx, by, bw, bh)
                    .id(format!("slot-{i}"))
                    .color(stroke)
                    .fill(fill)
                    .fill_style("tint")
                    .stroke_weight("normal"),
            );
            let value = match s.get("value") {
                Some(Value::String(v)) => Some(v.clone()),
                Some(Value::Number(v)) => v.as_f64().map(|n| js_num(n as f32)),
                _ => None,
            };
            if let Some(value) = value.filter(|_| state != "empty") {
                out.push(tone_text(
                    bx + bw / 2.0,
                    by + bh / 2.0,
                    value,
                    "ink",
                    "sm",
                    Align::Center,
                ));
            }
        }

        if bars.is_empty() {
            for p in pointers {
                let index = num(p, "index").unwrap_or(f32::NAN);
                if index < 0.0 || index >= slots.len() as f32 || index.is_nan() {
                    continue;
                }
                let (bx, by, bw, bh) = slot_box(index as usize);
                let color = text(p, "color").unwrap_or_else(|| DEFAULT_POINTER_COLOR.into());
                let label = text(p, "label").filter(|l| !l.is_empty());
                if vertical {
                    let cy = by + bh / 2.0;
                    out.push(
                        Shape::arrow(bx + bw + 34.0, cy, bx + bw + 4.0, cy)
                            .color(color.clone())
                            .stroke_weight("normal"),
                    );
                    if let Some(label) = label {
                        out.push(color_text(
                            bx + bw + 38.0,
                            cy,
                            label,
                            &color,
                            "sm",
                            Align::Left,
                        ));
                    }
                } else {
                    let cx = bx + bw / 2.0;
                    out.push(
                        Shape::arrow(cx, panel_y_slots + panel_height - 18.0, cx, by + bh + 4.0)
                            .color(color.clone())
                            .stroke_weight("normal"),
                    );
                    if let Some(label) = label {
                        out.push(color_text(
                            cx,
                            panel_y_slots + panel_height - 8.0,
                            label,
                            &color,
                            "sm",
                            Align::Center,
                        ));
                    }
                }
            }
        }
    }

    if !cells.is_empty() {
        let max_col = cells
            .iter()
            .map(|c| num(c, "col").unwrap_or(0.0))
            .fold(0.0, f32::max)
            + 1.0;
        let max_row = cells
            .iter()
            .map(|c| num(c, "row").unwrap_or(0.0))
            .fold(0.0, f32::max)
            + 1.0;
        let col_label_h = if col_labels.is_empty() { 0.0 } else { 16.0 };
        let row_label_w = if row_labels.is_empty() { 0.0 } else { 20.0 };
        let grid_x0 = row_label_w;
        let grid_y0 = panel_y_cells + col_label_h;
        let cw = (width - row_label_w) / max_col;
        let ch = (panel_height - col_label_h) / max_row;

        for (i, c) in cells.iter().enumerate() {
            let x = grid_x0 + num(c, "col").unwrap_or(0.0) * cw;
            let y = grid_y0 + num(c, "row").unwrap_or(0.0) * ch;
            let color = text(c, "color").unwrap_or_else(|| DEFAULT_CELL_COLOR.into());
            out.push(
                Shape::rect(x + 1.0, y + 1.0, cw - 2.0, ch - 2.0)
                    .id(format!("cell-{i}"))
                    .color("ink")
                    .fill(color)
                    .stroke_weight("thin"),
            );
            let label = text(c, "label").or_else(|| num(c, "value").map(js_num));
            if let Some(label) = label.filter(|l| !l.is_empty() && cw >= 18.0 && ch >= 14.0) {
                out.push(tone_text(
                    x + cw / 2.0,
                    y + ch / 2.0,
                    label,
                    "ink",
                    "sm",
                    Align::Center,
                ));
            }
            if let Some(corner) = c
                .get("corner")
                .filter(|_| cw >= CORNER_MIN_CELL && ch >= CORNER_MIN_CELL)
            {
                let spots = [
                    ("tl", x + CORNER_INSET_X, y + CORNER_INSET_Y, Align::Left),
                    (
                        "tr",
                        x + cw - CORNER_INSET_X,
                        y + CORNER_INSET_Y,
                        Align::Right,
                    ),
                    (
                        "bl",
                        x + CORNER_INSET_X,
                        y + ch - CORNER_INSET_Y,
                        Align::Left,
                    ),
                    (
                        "br",
                        x + cw - CORNER_INSET_X,
                        y + ch - CORNER_INSET_Y,
                        Align::Right,
                    ),
                ];
                for (key, sx, sy, align) in spots {
                    if let Some(s) = text(corner, key).filter(|s| !s.is_empty()) {
                        out.push(tone_text(sx, sy, s, "ink", "xs", align));
                    }
                }
            }
        }

        for l in col_labels {
            let color = text(l, "color").unwrap_or_else(|| "label".into());
            let index = num(l, "index").unwrap_or(0.0);
            let s = text(l, "text").unwrap_or_default();
            out.push(color_text(
                grid_x0 + index * cw + cw / 2.0,
                panel_y_cells + col_label_h / 2.0,
                s,
                &color,
                "xs",
                Align::Center,
            ));
        }
        for l in row_labels {
            let color = text(l, "color").unwrap_or_else(|| "label".into());
            let index = num(l, "index").unwrap_or(0.0);
            let s = text(l, "text").unwrap_or_default();
            out.push(color_text(
                row_label_w - 6.0,
                grid_y0 + index * ch + ch / 2.0,
                s,
                &color,
                "xs",
                Align::Right,
            ));
        }
    }

    if !buckets.is_empty() {
        let index_of = |b: &Value| num(b, "index").unwrap_or(0.0);
        let bucket_count = buckets.iter().map(index_of).fold(0.0, f32::max) + 1.0;
        let row_h = panel_height / bucket_count;
        let index_col_w = (width * 0.12).min(40.0);
        let max_chain = buckets
            .iter()
            .map(|b| list(b, "entries").len() as f32)
            .fold(1.0, f32::max);
        let entry_w =
            BUCKET_ENTRY_MAX_W.min(BUCKET_ENTRY_MIN_W.max((width - index_col_w - 8.0) / max_chain));
        let max_visible = ((width - index_col_w - 4.0) / entry_w).floor();

        for b in buckets {
            let index = index_of(b);
            let entries = list(b, "entries");
            let row_y = panel_y_buckets + index * row_h;
            out.push(
                Shape::rect(2.0, row_y + 2.0, index_col_w - 4.0, row_h - 4.0)
                    .id(format!("bucket-index-{}", js_num(index)))
                    .color("ink")
                    .fill("fill")
                    .stroke_weight("thin"),
            );
            out.push(tone_text(
                2.0 + (index_col_w - 4.0) / 2.0,
                row_y + row_h / 2.0,
                js_num(index),
                "ink",
                "xs",
                Align::Center,
            ));

            let overflow = entries.len() as f32 > max_visible;
            let visible = if overflow {
                (max_visible - 1.0).max(0.0) as usize
            } else {
                entries.len()
            };
            for (j, entry) in entries.iter().take(visible).enumerate() {
                let ex = index_col_w + 4.0 + j as f32 * entry_w;
                let state = entry
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("default");
                let fallback = match state {
                    "highlight" => "highlight",
                    "probing" => "accent",
                    _ => "series-1",
                };
                let fill = text(entry, "color").unwrap_or_else(|| fallback.into());
                out.push(tinted(
                    ex,
                    row_y + 2.0,
                    entry_w - 2.0,
                    row_h - 4.0,
                    format!("bucket-{}-{j}", js_num(index)),
                    &fill,
                ));
                if entry_w >= 20.0 && row_h >= 16.0 {
                    let label = text(entry, "label").unwrap_or_default();
                    out.push(tone_text(
                        ex + (entry_w - 2.0) / 2.0,
                        row_y + row_h / 2.0,
                        label,
                        "ink",
                        "xs",
                        Align::Center,
                    ));
                }
            }
            if overflow {
                let ex = index_col_w + 4.0 + visible as f32 * entry_w;
                out.push(tinted(
                    ex,
                    row_y + 2.0,
                    entry_w - 2.0,
                    row_h - 4.0,
                    format!("bucket-{}-overflow", js_num(index)),
                    DEFAULT_BAR_COLOR,
                ));
                out.push(tone_text(
                    ex + (entry_w - 2.0) / 2.0,
                    row_y + row_h / 2.0,
                    format!("+{}", entries.len() - visible),
                    "ink",
                    "xs",
                    Align::Center,
                ));
            }
        }
    }

    if !frames.is_empty() {
        let n = frames.len() as f32;
        let max_stride = (panel_height - FRAME_BOTTOM_PAD - TOP_PAD) / n;
        let stride = (FRAME_STRIP_H + FRAME_GAP).min(max_stride);
        let frame_h = FRAME_MIN_H.max(stride - FRAME_GAP);
        let x = 8.0;
        let w = width - 16.0;
        let bottom = panel_y_frames + panel_height - FRAME_BOTTOM_PAD;

        for (i, f) in frames.iter().enumerate() {
            let y = bottom - i as f32 * stride - frame_h;
            let fallback = match f.get("state").and_then(Value::as_str).unwrap_or("active") {
                "returning" => "highlight",
                "done" => "muted",
                _ => "primary",
            };
            let fill = text(f, "color").unwrap_or_else(|| fallback.into());
            out.push(tinted(x, y, w, frame_h, format!("frame-{i}"), &fill));
            let label = text(f, "label").unwrap_or_default();
            if frame_h >= FRAME_TWO_LINE_MIN_H {
                out.push(tone_text(
                    16.0,
                    y + frame_h * 0.35,
                    label,
                    "ink",
                    "sm",
                    Align::Left,
                ));
                if let Some(detail) = text(f, "detail").filter(|d| !d.is_empty()) {
                    out.push(tone_text(
                        16.0,
                        y + frame_h * 0.72,
                        detail,
                        "label",
                        "xs",
                        Align::Left,
                    ));
                }
            } else {
                out.push(tone_text(
                    16.0,
                    y + frame_h / 2.0,
                    label,
                    "ink",
                    "xs",
                    Align::Left,
                ));
            }
        }
    }

    out.extend(parse_shapes(props.get("shapes")));
    out
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let width = get_f32(props, "width", 600.0);
    let height = get_f32(props, "height", 400.0);
    let shapes = derive_shapes(props, width, height);
    let spec = CanvasSpec::from_props("algorithm-canvas", props, 600.0, 400.0, shapes);
    Ok(canvas::render(&spec, &canvas::theme_for(props), viewport))
}

#[cfg(test)]
mod tests;
