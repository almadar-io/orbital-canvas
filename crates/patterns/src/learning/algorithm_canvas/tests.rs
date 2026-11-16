use super::*;
use crate::learning::shape::ShapeType;
use orbital_canvas_base::packet::DrawCommand;
use serde_json::json;

const VALUES: [f64; 8] = [5.0, 2.0, 8.0, 1.0, 9.0, 3.0, 7.0, 4.0];

fn near(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|a| (a - b).abs() < 0.01)
}

fn of(shapes: &[Shape], ty: ShapeType) -> Vec<&Shape> {
    shapes.iter().filter(|s| s.ty == ty).collect()
}

fn by_id<'a>(shapes: &'a [Shape], id: &str) -> &'a Shape {
    shapes
        .iter()
        .find(|s| s.id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no shape {id}"))
}

fn texts(shapes: &[Shape]) -> Vec<String> {
    shapes
        .iter()
        .filter(|s| s.ty == ShapeType::Text)
        .filter_map(|s| s.text.clone())
        .collect()
}

/// `std-algo-bubblesort` at its config defaults (values 5,2,8,1,9,3,7,4 on
/// 600x400) as `BubbleSortSim` renders it on INIT.
fn bubble_initial() -> Value {
    let bars: Vec<Value> = VALUES.iter().map(|v| json!({"value": v, "label": format!("{v}").trim_end_matches(".0"), "color": "series-1"})).collect();
    json!({
        "type": "algorithm-canvas", "width": 600, "height": 400, "backgroundColor": "",
        "keyMap": {"Space": "TOGGLE_RUN"},
        "bars": bars,
        "pointers": [{"index": 0, "label": "j"}, {"index": 1, "label": "j+1"}],
        "ranges": []
    })
}

/// Same behavior after the first pass: n=6, compare pair (2,3) highlighted,
/// the sorted suffix muted and covered by the `sorted` fill range.
fn bubble_mid() -> Value {
    let colors = [
        "series-1",
        "series-1",
        "highlight",
        "highlight",
        "series-1",
        "series-1",
        "muted",
        "muted",
    ];
    let bars: Vec<Value> = VALUES
        .iter()
        .zip(colors)
        .map(|(v, c)| json!({"value": v, "label": format!("{v}").trim_end_matches(".0"), "color": c}))
        .collect();
    json!({
        "type": "algorithm-canvas", "width": 600, "height": 400, "bars": bars,
        "pointers": [{"index": 2, "label": "j"}, {"index": 3, "label": "j+1"}],
        "ranges": [{"from": 6, "to": 7, "kind": "fill", "label": "sorted", "color": "fill"}]
    })
}

#[test]
fn bubble_initial_lays_eight_tinted_bars_on_a_334px_usable_height() {
    let shapes = derive_shapes(&bubble_initial(), 600.0, 400.0);
    let tallest = by_id(&shapes, "bar-4");
    assert!(
        near(tallest.height, 334.0) && near(tallest.y, 26.0),
        "value 9 = max fills baseline 360 up to TOP_PAD 26"
    );
    let first = by_id(&shapes, "bar-0");
    assert!(near(first.x, 7.5) && near(first.width, 60.0) && near(first.height, 5.0 / 9.0 * 334.0));
    assert_eq!(first.color.as_deref(), Some("series-1"));
    assert_eq!(first.fill.as_deref(), Some("series-1"));
    assert_eq!(first.fill_style.as_deref(), Some("tint"));
    assert_eq!(first.stroke.as_deref(), Some("normal"));
    assert_eq!(
        of(&shapes, ShapeType::Rect).len(),
        8,
        "no range fill when ranges is empty"
    );
}

#[test]
fn bar_labels_sit_8px_above_each_bar_in_the_label_tone() {
    let shapes = derive_shapes(&bubble_initial(), 600.0, 400.0);
    let label = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("9"))
        .expect("label for 9");
    assert!(near(label.x, 7.5 + 4.0 * 75.0 + 30.0) && near(label.y, 26.0 - 8.0));
    assert_eq!(
        (
            label.tone.as_deref(),
            label.text_size.as_deref(),
            label.align
        ),
        (Some("label"), Some("xs"), Some(Align::Center))
    );
}

#[test]
fn pointers_draw_an_arrow_under_the_bar_and_a_label_in_the_pointer_band() {
    let shapes = derive_shapes(&bubble_initial(), 600.0, 400.0);
    let arrows = of(&shapes, ShapeType::Arrow);
    assert_eq!(arrows.len(), 2);
    assert!(near(arrows[0].x1, 37.5) && near(arrows[0].y1, 375.0) && near(arrows[0].y2, 363.0));
    assert_eq!(arrows[0].color.as_deref(), Some("highlight"));
    let j1 = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("j+1"))
        .expect("j+1 label");
    assert!(near(j1.x, 112.5) && near(j1.y, 390.0));
    assert_eq!(
        (j1.color.as_deref(), j1.text_size.as_deref()),
        (Some("highlight"), Some("sm"))
    );
    let mut none = bubble_initial();
    none["pointers"] = json!([]);
    assert!(
        of(&derive_shapes(&none, 600.0, 400.0), ShapeType::Arrow).is_empty(),
        "control: no pointers, no arrows"
    );
}

#[test]
fn a_fill_range_is_a_translucent_solid_band_with_a_left_label() {
    let shapes = derive_shapes(&bubble_mid(), 600.0, 400.0);
    let band = of(&shapes, ShapeType::Rect)
        .into_iter()
        .find(|s| s.id.is_none())
        .expect("range band");
    assert!(
        near(band.x, 450.0)
            && near(band.width, 150.0)
            && near(band.height, 400.0)
            && near(band.y, 0.0)
    );
    assert_eq!(
        (
            band.fill_style.as_deref(),
            band.stroke.as_deref(),
            band.color.as_deref()
        ),
        (Some("solid"), Some("thin"), Some("fill"))
    );
    assert!(near(band.opacity, 0.15));
    let label = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("sorted"))
        .expect("range label");
    assert!(near(label.x, 454.0) && near(label.y, 10.0) && label.align == Some(Align::Left));
    let mut no_range = bubble_mid();
    no_range["ranges"] = json!([]);
    assert_eq!(
        of(&derive_shapes(&no_range, 600.0, 400.0), ShapeType::Rect).len(),
        8,
        "control: no band without the range"
    );
}

#[test]
fn bar_state_colors_come_from_the_bar_color_not_the_series_default() {
    let shapes = derive_shapes(&bubble_mid(), 600.0, 400.0);
    assert_eq!(by_id(&shapes, "bar-2").color.as_deref(), Some("highlight"));
    assert_eq!(by_id(&shapes, "bar-7").fill.as_deref(), Some("muted"));
    let mut bare = bubble_mid();
    bare["bars"][2].as_object_mut().map(|o| o.remove("color"));
    assert_eq!(
        by_id(&derive_shapes(&bare, 600.0, 400.0), "bar-2")
            .color
            .as_deref(),
        Some("series-1")
    );
}

#[test]
fn two_pointers_on_one_index_merge_their_labels_and_keep_the_first_color() {
    let mut props = bubble_initial();
    props["pointers"] = json!([{"index": 3, "label": "lo", "color": "success"}, {"index": 3, "label": "mid", "color": "error"}]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert_eq!(of(&shapes, ShapeType::Arrow).len(), 1);
    let label = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("lo · mid"))
        .expect("merged label");
    assert_eq!(label.color.as_deref(), Some("success"));
    props["pointers"] =
        json!([{"index": 3, "label": "lo", "color": "success"}, {"index": 4, "label": "mid"}]);
    assert_eq!(
        of(&derive_shapes(&props, 600.0, 400.0), ShapeType::Arrow).len(),
        2,
        "control: distinct indices stay separate"
    );
}

#[test]
fn pointers_outside_the_bars_are_dropped() {
    let mut props = bubble_initial();
    props["pointers"] = json!([{"index": 8, "label": "x"}, {"index": -1, "label": "y"}]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert!(of(&shapes, ShapeType::Arrow).is_empty() && !texts(&shapes).contains(&"x".to_string()));
}

#[test]
fn labels_default_to_the_value_up_to_24_bars_and_an_empty_label_hides_it() {
    let bars: Vec<Value> = (0..24).map(|i| json!({"value": i})).collect();
    assert_eq!(
        texts(&derive_shapes(&json!({"bars": bars}), 600.0, 400.0)).len(),
        24
    );
    let bars: Vec<Value> = (0..25).map(|i| json!({"value": i})).collect();
    assert!(
        texts(&derive_shapes(&json!({"bars": bars}), 600.0, 400.0)).is_empty(),
        "25 bars: no auto labels"
    );
    let shapes = derive_shapes(
        &json!({"bars": [{"value": 3, "label": ""}, {"value": 4.5}]}),
        600.0,
        400.0,
    );
    assert_eq!(texts(&shapes), vec!["4.5".to_string()]);
}

/// `std-algo-mergesort` mid-merge: bars + aux strip + a bracket over the
/// active span + a tinted fill range, sharing the single bars panel.
#[test]
fn mergesort_splits_the_bars_panel_into_primary_and_aux_with_a_bracket_headroom() {
    let bars: Vec<Value> = VALUES
        .iter()
        .map(|v| json!({"value": v, "label": "", "color": "series-1"}))
        .collect();
    let aux: Vec<Value> = VALUES
        .iter()
        .map(|_| json!({"value": 0, "label": "", "color": "fill"}))
        .collect();
    let props = json!({
        "bars": bars, "auxBars": aux,
        "ranges": [{"from": 0, "to": 3, "kind": "bracket", "label": "merge", "color": "accent"}, {"from": 0, "to": 1, "kind": "fill", "color": "series-2"}]
    });
    let shapes = derive_shapes(&props, 600.0, 400.0);
    // primaryH 240, baseline 200, headroom 16+14=30, usable 200-(26+30)=144
    assert!(near(by_id(&shapes, "bar-4").height, 144.0) && near(by_id(&shapes, "bar-4").y, 56.0));
    let aux0 = by_id(&shapes, "auxbar-0");
    assert!(
        near(aux0.y.zip(aux0.height).map(|(y, h)| y + h), 392.0),
        "aux baseline = 240+160-8"
    );
    let lines = of(&shapes, ShapeType::Line);
    assert_eq!(lines.len(), 3);
    assert!(near(lines[0].x1, 7.5) && near(lines[0].x2, 300.0 - 7.5) && near(lines[0].y1, 16.0));
    assert_eq!(
        (
            lines[1].y2.zip(lines[1].y1).map(|(b, a)| b - a),
            lines[0].color.as_deref()
        ),
        (Some(6.0), Some("accent"))
    );
    let merge = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("merge"))
        .expect("bracket label");
    assert!(near(merge.y, 10.0) && merge.align == Some(Align::Center));
    let band = of(&shapes, ShapeType::Rect)
        .into_iter()
        .find(|s| s.id.is_none())
        .expect("fill range");
    assert!(near(band.height, 240.0) && band.color.as_deref() == Some("series-2"));

    let mut plain = props.clone();
    plain["auxBars"] = json!([]);
    plain["ranges"] = json!([]);
    let p = derive_shapes(&plain, 600.0, 400.0);
    assert!(
        near(by_id(&p, "bar-4").height, 334.0),
        "control: no aux/bracket restores full height"
    );
}

/// `std-algo-hashing` after inserting 12, 25, 37, 41 (mod 8): the table is
/// 16 pre-allocated buckets, keys 25 and 41 collide into bucket 1.
fn hashing() -> Value {
    let mut buckets: Vec<Value> = (0..16)
        .map(|i| json!({"index": i, "entries": []}))
        .collect();
    buckets[4] = json!({"index": 4, "entries": [{"label": "k12", "state": "default"}]});
    buckets[1] = json!({"index": 1, "entries": [{"label": "k25", "state": "default"}, {"label": "k41", "state": "highlight"}]});
    buckets[5] = json!({"index": 5, "entries": [{"label": "k37", "state": "probing"}]});
    json!({"type": "algorithm-canvas", "width": 600, "height": 400, "buckets": buckets})
}

#[test]
fn hashing_draws_an_index_chip_per_bucket_and_chained_entries_by_state() {
    let shapes = derive_shapes(&hashing(), 600.0, 400.0);
    let chip = by_id(&shapes, "bucket-index-1");
    assert!(
        near(chip.x, 2.0)
            && near(chip.y, 25.0 + 2.0)
            && near(chip.width, 36.0)
            && near(chip.height, 21.0)
    );
    assert_eq!(
        (
            chip.color.as_deref(),
            chip.fill.as_deref(),
            chip.stroke.as_deref()
        ),
        (Some("ink"), Some("fill"), Some("thin"))
    );
    let e0 = by_id(&shapes, "bucket-1-0");
    let e1 = by_id(&shapes, "bucket-1-1");
    assert!(
        near(e0.x, 44.0) && near(e1.x, 108.0) && near(e0.width, 62.0),
        "entryW 64 from index col 40 + 4"
    );
    assert_eq!(e0.fill.as_deref(), Some("series-1"));
    assert_eq!(e1.fill.as_deref(), Some("highlight"));
    assert_eq!(by_id(&shapes, "bucket-5-0").fill.as_deref(), Some("accent"));
    let k41 = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("k41"))
        .expect("entry label");
    assert!(
        near(k41.x, 108.0 + 31.0) && near(k41.y, 25.0 + 12.5) && k41.tone.as_deref() == Some("ink")
    );
}

#[test]
fn a_chain_longer_than_the_row_collapses_into_a_plus_n_overflow_chip() {
    let entries: Vec<Value> = (0..12).map(|i| json!({"label": format!("k{i}")})).collect();
    let props = json!({"buckets": [{"index": 0, "entries": entries}, {"index": 1, "entries": []}]});
    let shapes = derive_shapes(&props, 600.0, 400.0);
    // entryW = max(24, (600-40-8)/12=46) = 46; maxVisible = floor(556/46) = 12 -> no overflow
    assert!(shapes
        .iter()
        .all(|s| s.id.as_deref() != Some("bucket-0-overflow")));
    let entries: Vec<Value> = (0..30).map(|i| json!({"label": format!("k{i}")})).collect();
    let props = json!({"buckets": [{"index": 0, "entries": entries}]});
    let shapes = derive_shapes(&props, 600.0, 400.0);
    // entryW = 24; maxVisible = floor(556/24) = 23 -> 22 shown + "+8"
    assert!(shapes
        .iter()
        .any(|s| s.id.as_deref() == Some("bucket-0-21")));
    assert!(shapes
        .iter()
        .all(|s| s.id.as_deref() != Some("bucket-0-22")));
    assert!(by_id(&shapes, "bucket-0-overflow").fill.as_deref() == Some("series-1"));
    assert!(texts(&shapes).contains(&"+8".to_string()));
}

/// `std-algo-stack` after three pushes: 10 vertical slots, three filled from
/// the bottom, a `top` pointer on slot 2.
fn stack() -> Value {
    let mut slots: Vec<Value> = (0..10).map(|_| json!({"state": "empty"})).collect();
    for (i, v) in ["5", "12", "8"].iter().enumerate() {
        slots[i] = json!({"value": v, "state": "filled"});
    }
    json!({
        "type": "algorithm-canvas", "width": 600, "height": 400, "slots": slots, "slotOrientation": "vertical",
        "pointers": [{"index": 2, "label": "top", "color": "series-3"}]
    })
}

#[test]
fn stack_slots_stack_bottom_up_and_empty_slots_draw_no_value() {
    let shapes = derive_shapes(&stack(), 600.0, 400.0);
    let s0 = by_id(&shapes, "slot-0");
    assert!(
        near(s0.x, 240.0) && near(s0.y, 360.0) && near(s0.width, 120.0) && near(s0.height, 40.0)
    );
    assert!(near(by_id(&shapes, "slot-2").y, 280.0));
    assert_eq!(
        (
            s0.color.as_deref(),
            s0.fill.as_deref(),
            s0.fill_style.as_deref()
        ),
        (Some("ink"), Some("series-1"), Some("tint"))
    );
    let empty = by_id(&shapes, "slot-5");
    assert_eq!(
        (empty.color.as_deref(), empty.fill.as_deref()),
        (Some("guide"), Some("fill"))
    );
    assert_eq!(
        texts(&shapes)
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .len(),
        4,
        "three values plus the pointer label"
    );
    let v = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("12"))
        .expect("value");
    assert!(
        near(v.x, 300.0)
            && near(v.y, 340.0)
            && v.tone.as_deref() == Some("ink")
            && v.text_size.as_deref() == Some("sm")
    );
}

#[test]
fn a_vertical_slot_pointer_points_left_from_beside_the_box() {
    let shapes = derive_shapes(&stack(), 600.0, 400.0);
    let arrow = of(&shapes, ShapeType::Arrow)[0];
    assert!(
        near(arrow.x1, 394.0)
            && near(arrow.x2, 364.0)
            && near(arrow.y1, 300.0)
            && near(arrow.y2, 300.0)
    );
    assert_eq!(arrow.color.as_deref(), Some("series-3"));
    let top = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("top"))
        .expect("label");
    assert!(near(top.x, 398.0) && near(top.y, 300.0) && top.align == Some(Align::Left));
    let mut horizontal = stack();
    horizontal["slotOrientation"] = json!("horizontal");
    let h = derive_shapes(&horizontal, 600.0, 400.0);
    let a = of(&h, ShapeType::Arrow)[0];
    assert!(
        a.x1 == a.x2 && near(a.y1, 382.0),
        "control: horizontal pointer rises from the panel bottom"
    );
    assert!(near(by_id(&h, "slot-0").height, 48.0));
}

#[test]
fn slot_pointers_are_ignored_when_bars_are_present() {
    let mut props = stack();
    props["bars"] = json!([{"value": 1}]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert_eq!(
        of(&shapes, ShapeType::Arrow).len(),
        0,
        "pointer 2 is out of the single bar's range and slots defer to bars"
    );
}

#[test]
fn a_highlight_slot_takes_the_highlight_tone_unless_colored() {
    let props = json!({"slots": [{"value": 1, "state": "highlight"}, {"value": 2, "state": "highlight", "color": "error"}, {"value": 3, "color": "success"}]});
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert_eq!(by_id(&shapes, "slot-0").fill.as_deref(), Some("highlight"));
    assert_eq!(by_id(&shapes, "slot-1").fill.as_deref(), Some("error"));
    assert_eq!(by_id(&shapes, "slot-2").fill.as_deref(), Some("success"));
}

/// `std-algo-dp` (4x5 knapsack table at defaults) one row filled, with the
/// r0.. / c0.. header labels.
fn dp() -> Value {
    let mut cells = Vec::new();
    for r in 0..4 {
        for c in 0..5 {
            let filled = r == 0;
            cells.push(json!({"id": r * 5 + c, "row": r, "col": c, "value": if filled { c } else { 0 }, "color": if filled { "success" } else { "fill" }, "label": if filled { format!("{c}") } else { String::new() }}));
        }
    }
    let rows: Vec<Value> = (0..4)
        .map(|i| json!({"index": i, "text": format!("r{i}")}))
        .collect();
    let cols: Vec<Value> = (0..5)
        .map(|i| json!({"index": i, "text": format!("c{i}")}))
        .collect();
    json!({"type": "algorithm-canvas", "width": 600, "height": 400, "cells": cells, "rowLabels": rows, "colLabels": cols})
}

#[test]
fn dp_cells_form_a_lattice_inset_by_the_label_gutters() {
    let shapes = derive_shapes(&dp(), 600.0, 400.0);
    let c = by_id(&shapes, "cell-6");
    assert!(
        near(c.x, 20.0 + 116.0 + 1.0)
            && near(c.y, 16.0 + 96.0 + 1.0)
            && near(c.width, 114.0)
            && near(c.height, 94.0),
        "cw=(600-20)/5, ch=(400-16)/4"
    );
    assert_eq!(
        (
            c.color.as_deref(),
            c.fill.as_deref(),
            c.stroke.as_deref(),
            c.fill_style.as_deref()
        ),
        (Some("ink"), Some("fill"), Some("thin"), None)
    );
    assert_eq!(by_id(&shapes, "cell-0").fill.as_deref(), Some("success"));
    let t3 = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("3"))
        .expect("cell label");
    assert!(
        near(t3.x, 20.0 + 3.0 * 116.0 + 58.0)
            && near(t3.y, 16.0 + 48.0)
            && t3.tone.as_deref() == Some("ink")
    );
    assert_eq!(
        texts(&shapes)
            .iter()
            .filter(|t| t.len() == 1 && t.chars().all(|c| c.is_ascii_digit()))
            .count(),
        5,
        "empty-label cells draw no text"
    );
}

#[test]
fn dp_row_and_col_labels_sit_in_the_gutters_and_vanish_without_labels() {
    let shapes = derive_shapes(&dp(), 600.0, 400.0);
    let c2 = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("c2"))
        .expect("col label");
    assert!(
        near(c2.x, 20.0 + 2.0 * 116.0 + 58.0)
            && near(c2.y, 8.0)
            && c2.color.as_deref() == Some("label")
            && c2.align == Some(Align::Center)
    );
    let r1 = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("r1"))
        .expect("row label");
    assert!(near(r1.x, 14.0) && near(r1.y, 16.0 + 96.0 + 48.0) && r1.align == Some(Align::Right));
    let mut bare = dp();
    bare["rowLabels"] = json!([]);
    bare["colLabels"] = json!([]);
    let b = derive_shapes(&bare, 600.0, 400.0);
    assert!(
        near(by_id(&b, "cell-0").x, 1.0) && near(by_id(&b, "cell-0").y, 1.0),
        "control: no gutters"
    );
    assert!(!texts(&b).contains(&"c2".to_string()));
}

#[test]
fn corner_annotations_need_a_28px_cell() {
    let cells = json!([{"row": 0, "col": 0, "value": 7, "corner": {"tl": "a", "tr": "b", "bl": "c", "br": "d"}}]);
    let shapes = derive_shapes(&json!({"cells": cells}), 600.0, 400.0);
    let tr = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("b"))
        .expect("tr");
    assert!(
        near(tr.x, 597.0)
            && near(tr.y, 6.0)
            && tr.align == Some(Align::Right)
            && tr.text_size.as_deref() == Some("xs")
    );
    let br = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("d"))
        .expect("br");
    assert!(near(br.y, 394.0));
    let small = derive_shapes(&json!({"cells": cells}), 16.0, 10.0);
    assert!(
        !texts(&small).contains(&"a".to_string()),
        "control: a small cell drops the corners"
    );
    assert!(
        !texts(&small).contains(&"7".to_string()),
        "and a cell under 18x14 drops its value"
    );
}

/// `std-algo-recursion` (factorial 5) after three nested calls: a bottom-up
/// stack of fixed 44px slabs.
#[test]
fn recursion_frames_stack_bottom_up_in_fixed_slabs_colored_by_state() {
    let frames = json!([
        {"label": "fact(5)", "detail": "root call", "state": "done"},
        {"label": "fact(4)", "detail": "computing", "state": "returning"},
        {"label": "fact(3)", "detail": "computing", "state": "active"}
    ]);
    let shapes = derive_shapes(&json!({"frames": frames}), 600.0, 400.0);
    let f0 = by_id(&shapes, "frame-0");
    assert!(
        near(f0.x, 8.0)
            && near(f0.width, 584.0)
            && near(f0.height, 44.0)
            && near(f0.y, 392.0 - 44.0)
    );
    assert!(
        near(by_id(&shapes, "frame-2").y, 392.0 - 100.0 - 44.0),
        "stride 50"
    );
    assert_eq!(f0.fill.as_deref(), Some("muted"));
    assert_eq!(by_id(&shapes, "frame-1").fill.as_deref(), Some("highlight"));
    assert_eq!(by_id(&shapes, "frame-2").fill.as_deref(), Some("primary"));
    let label = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("fact(3)"))
        .expect("label");
    assert!(
        near(label.x, 16.0)
            && near(label.y, 248.0 + 44.0 * 0.35)
            && label.text_size.as_deref() == Some("sm")
    );
    let detail = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("root call"))
        .expect("detail");
    assert!(
        near(detail.y, 348.0 + 44.0 * 0.72)
            && detail.tone.as_deref() == Some("label")
            && detail.text_size.as_deref() == Some("xs")
    );
}

#[test]
fn a_deep_stack_compresses_and_single_lines_the_labels() {
    let frames: Vec<Value> = (0..30)
        .map(|i| json!({"label": format!("f{i}"), "detail": "d"}))
        .collect();
    let shapes = derive_shapes(&json!({"frames": frames}), 600.0, 400.0);
    // maxStride = (400-8-26)/30 = 12.2 -> frameH = max(12, 6.2) = 12
    assert!(near(by_id(&shapes, "frame-0").height, 12.0));
    let l = shapes
        .iter()
        .find(|s| s.text.as_deref() == Some("f0"))
        .expect("label");
    assert!(near(l.y, 392.0 - 6.0) && l.text_size.as_deref() == Some("xs"));
    assert!(
        !texts(&shapes).contains(&"d".to_string()),
        "no detail line below 22px"
    );
}

#[test]
fn families_share_the_height_as_equal_panels_in_the_fixed_order() {
    let props = json!({
        "frames": [{"label": "a"}], "bars": [{"value": 2}], "cells": [{"row": 0, "col": 0}],
        "slots": [{"value": 1}], "buckets": [{"index": 0, "entries": []}]
    });
    let shapes = derive_shapes(&props, 600.0, 500.0);
    // panels of 100: bars, slots, cells, buckets, frames
    assert!(near(
        by_id(&shapes, "bar-0")
            .y
            .zip(by_id(&shapes, "bar-0").height)
            .map(|(y, h)| y + h),
        100.0 - 40.0
    ));
    assert!(
        near(by_id(&shapes, "slot-0").y, 100.0 + 26.0),
        "(100 - 48)/2 centered"
    );
    assert!(near(by_id(&shapes, "cell-0").y, 200.0 + 1.0));
    assert!(near(by_id(&shapes, "bucket-index-0").y, 300.0 + 2.0));
    assert!(near(
        by_id(&shapes, "frame-0")
            .y
            .zip(by_id(&shapes, "frame-0").height)
            .map(|(y, h)| y + h),
        500.0 - 8.0
    ));
}

#[test]
fn no_families_derive_nothing_and_user_shapes_append_last() {
    assert!(derive_shapes(&json!({}), 600.0, 400.0).is_empty());
    let mut props = bubble_initial();
    props["shapes"] = json!([{"type": "line", "x1": 0, "y1": 0, "x2": 5, "y2": 5, "id": "mine"}]);
    let shapes = derive_shapes(&props, 600.0, 400.0);
    assert_eq!(shapes.last().and_then(|s| s.id.as_deref()), Some("mine"));
}

#[test]
fn the_scene_paints_labels_and_reports_taps_with_derived_indices() {
    let mut props = bubble_initial();
    props["interactive"] = json!(true);
    props["onShapeClick"] = json!("TAP_SHAPE");
    let scene = map(&props, None).expect("maps");
    let drawn: Vec<&str> = scene
        .packet
        .draw_commands
        .iter()
        .filter_map(|c| match c {
            DrawCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(drawn.contains(&"j+1") && drawn.contains(&"9"), "{drawn:?}");
    let hit = scene.emits_at(37.5, 300.0);
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].event, "TAP_SHAPE");
    assert_eq!(
        hit[0].payload,
        json!({"id": "bar-0", "type": "rect", "index": 0})
    );
    let second = scene.emits_at(112.5, 300.0);
    assert_eq!(second[0].payload["id"], "bar-1");
    props["interactive"] = json!(false);
    assert!(
        map(&props, None)
            .expect("maps")
            .emits_at(37.5, 300.0)
            .is_empty(),
        "control: not interactive, no hits"
    );
}

#[test]
fn readouts_and_the_series_override_ride_the_atom() {
    let mut props = bubble_initial();
    let base = map(&props, None).expect("maps").packet.draw_commands.len();
    props["readouts"] = json!([{"label": "swaps", "value": 4}]);
    assert!(map(&props, None).expect("maps").packet.draw_commands.len() > base);
}
