//! `positioned-canvas` — the Rust twin of `PositionedCanvas.tsx`: a floor plan of status-bordered
//! items (round / square / rectangle) placed at their own `x`,`y`. When `editable`, a tap toggles
//! selection and a drag moves the item (reported through `moveEvent` / `onMove`); a tap on the
//! plan itself clears the selection.

use super::draw::{frame, tint, Ink, Pen};
use super::{props_f32, props_str};
use crate::mappers::game_canvas_2d::CanvasSceneError;
use crate::mappers::learning::canvas::theme_for;
use crate::mappers::scene::{Drag, Emit, Hit, MappedScene};
use crate::packet::{Rect, TextAlign};
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Empty,
    Seated,
    Ordered,
    AwaitingBill,
    Cleaning,
}

impl Status {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("seated") => Self::Seated,
            Some("ordered") => Self::Ordered,
            Some("awaiting-bill") => Self::AwaitingBill,
            Some("cleaning") => Self::Cleaning,
            _ => Self::Empty,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::Seated => "Seated",
            Self::Ordered => "Ordered",
            Self::AwaitingBill => "Awaiting bill",
            Self::Cleaning => "Cleaning",
        }
    }

    fn tone(self) -> &'static str {
        match self {
            Self::Empty | Self::Cleaning => "grid",
            Self::Seated => "success",
            Self::Ordered => "warning",
            Self::AwaitingBill => "info",
        }
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Round,
    Square,
    Rectangle,
}

impl Shape {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("square") => Self::Square,
            Some("rectangle") => Self::Rectangle,
            _ => Self::Round,
        }
    }

    fn size(self) -> (f32, f32) {
        match self {
            Self::Round | Self::Square => (96.0, 96.0),
            Self::Rectangle => (144.0, 80.0),
        }
    }
}

const HEAVY: f32 = 2.0;
const ICON: f32 = 16.0;

fn icon(pen: &mut Pen, status: Status, x: f32, y: f32, color: [f32; 4]) {
    let k = ICON / 24.0;
    let p = |px: f32, py: f32| (x + px * k, y + py * k);
    let ink = Ink::new(color, 2.0 * k);
    match status {
        Status::Seated => {
            let (c, r) = (p(9.0, 7.0), 4.0 * k);
            pen.circle(c.0, c.1, r, None, Some(&ink));
            let body = [
                (2.0, 21.0),
                (2.0, 17.0),
                (4.0, 15.0),
                (14.0, 15.0),
                (16.0, 17.0),
                (16.0, 21.0),
            ];
            pen.polyline(&body.map(|(a, b)| p(a, b)), false, None, Some(&ink));
        }
        Status::Ordered => {
            let cup = [
                (4.0, 8.0),
                (4.0, 17.0),
                (8.0, 21.0),
                (14.0, 21.0),
                (18.0, 17.0),
                (18.0, 8.0),
                (4.0, 8.0),
            ];
            pen.polyline(&cup.map(|(a, b)| p(a, b)), false, None, Some(&ink));
            let handle = [(18.0, 10.0), (21.0, 10.0), (21.0, 14.0), (18.0, 14.0)];
            pen.polyline(&handle.map(|(a, b)| p(a, b)), false, None, Some(&ink));
            for sx in [8.0, 12.0] {
                pen.line(p(sx, 2.0), p(sx, 5.0), &ink);
            }
        }
        Status::AwaitingBill => {
            let c = p(12.0, 12.0);
            pen.circle(c.0, c.1, 10.0 * k, None, Some(&ink));
            pen.line(p(12.0, 8.0), p(12.0, 12.0), &ink);
            pen.line(p(12.0, 16.0), p(12.01, 16.0), &ink);
        }
        _ => {}
    }
}

struct Item {
    id: String,
    label: String,
    x: f32,
    y: f32,
    capacity: f32,
    party: Option<f32>,
    server: Option<String>,
    status: Status,
    shape: Shape,
}

fn parse_items(props: &Value) -> Vec<Item> {
    props
        .get("items")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    Some(Item {
                        id: r.get("id").and_then(Value::as_str)?.to_string(),
                        label: r
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        x: r.get("x").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                        y: r.get("y").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                        capacity: r.get("capacity").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                        party: r.get("partySize").and_then(Value::as_f64).map(|v| v as f32),
                        server: r
                            .get("serverName")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                        status: Status::parse(r.get("status").and_then(Value::as_str)),
                        shape: Shape::parse(r.get("shape").and_then(Value::as_str)),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn count_text(n: f32) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

pub fn map(props: &Value, viewport: Option<(f32, f32)>) -> Result<MappedScene, CanvasSceneError> {
    let theme = theme_for(props);
    let (w, h) = (
        props_f32(props, "width", 800.0).max(1.0),
        props_f32(props, "height", 600.0).max(1.0),
    );
    let (xf, pw, ph) = frame(w, h, viewport);
    let mut pen = Pen::new(&theme, xf);
    pen.clear(theme.ground);
    let (ground, grid, ink, quiet) = (
        pen.ground(),
        pen.tone("grid"),
        pen.tone("ink"),
        pen.tone("label"),
    );
    pen.rrect(
        0.5,
        0.5,
        w - 1.0,
        h - 1.0,
        theme.corner,
        Some(ground),
        Some(&Ink::new(grid, 1.0)),
    );

    let editable = props
        .get("editable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let selected = props_str(props, "selectedId").filter(|s| !s.is_empty());
    let tap = |name: &str| {
        props_str(props, name)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let (select_event, on_select) = (tap("selectEvent"), tap("onSelect"));
    let (move_event, on_move) = (tap("moveEvent"), tap("onMove"));
    let select_emits = |id: Option<&str>| -> Vec<Emit> {
        let payload = id.map_or(Value::Null, |i| json!(i));
        let mut out = Vec::new();
        if let Some(ev) = &on_select {
            out.push(Emit {
                event: ev.clone(),
                arg: "id".into(),
                payload: payload.clone(),
            });
        }
        if let Some(ev) = &select_event {
            out.push(Emit {
                event: ev.clone(),
                arg: String::new(),
                payload: json!({ "id": payload }),
            });
        }
        out
    };
    if !select_emits(None).is_empty() {
        pen.hits.push(Hit {
            rect: Rect {
                x: pen.xf.ox,
                y: pen.xf.oy,
                w: w * pen.xf.s,
                h: h * pen.xf.s,
            },
            emits: select_emits(None),
        });
    }

    let items = parse_items(props);
    let mut drags: Vec<Drag> = Vec::new();
    for it in &items {
        let (iw, ih) = it.shape.size();
        let is_selected = selected == Some(it.id.as_str());
        let border = pen.tone(it.status.tone());
        let fill = if it.status == Status::Cleaning {
            mix_wash(&pen)
        } else {
            ground
        };
        let corner = theme.corner;
        let item_x = it.x;
        let item_y = it.y;
        if is_selected {
            let primary = pen.tone("primary");
            match it.shape {
                Shape::Round => pen.circle(
                    item_x + iw / 2.0,
                    item_y + ih / 2.0,
                    iw / 2.0 + 2.0 + 1.0,
                    None,
                    Some(&Ink::new(primary, 2.0)),
                ),
                _ => pen.rrect(
                    item_x - 3.0,
                    item_y - 3.0,
                    iw + 6.0,
                    ih + 6.0,
                    corner + 3.0,
                    None,
                    Some(&Ink::new(primary, 2.0)),
                ),
            }
        }
        let ink_border = Ink::new(border, HEAVY);
        match it.shape {
            Shape::Round => pen.circle(
                item_x + iw / 2.0,
                item_y + ih / 2.0,
                iw / 2.0 - HEAVY / 2.0,
                Some(fill),
                Some(&ink_border),
            ),
            _ => pen.rrect(
                item_x + HEAVY / 2.0,
                item_y + HEAVY / 2.0,
                iw - HEAVY,
                ih - HEAVY,
                corner,
                Some(fill),
                Some(&ink_border),
            ),
        }

        let text_color = if it.status == Status::Cleaning {
            quiet
        } else {
            ink
        };
        let caption = match (it.status, it.party) {
            (Status::Seated, Some(p)) => format!("{}/{}", count_text(p), count_text(it.capacity)),
            _ => format!("Cap {}", count_text(it.capacity)),
        };
        let server = (it.status == Status::Seated)
            .then(|| it.server.clone())
            .flatten();
        let rows = 2.0 + if server.is_some() { 1.0 } else { 0.0 };
        let content_h = 20.0 + 16.0 * (rows - 1.0) + 4.0 * (rows - 1.0);
        let top = item_y + (ih - content_h) / 2.0;
        let has_icon = matches!(
            it.status,
            Status::Seated | Status::Ordered | Status::AwaitingBill
        );
        let label_w = crate::vector::text_width(&it.label, 14.0);
        let row_w = label_w + if has_icon { ICON + 4.0 } else { 0.0 };
        let mut lx = item_x + (iw - row_w) / 2.0;
        if has_icon {
            icon(&mut pen, it.status, lx, top + 2.0, text_color);
            lx += ICON + 4.0;
        }
        pen.text_middle(
            lx,
            top + 10.0,
            &it.label,
            14.0,
            text_color,
            TextAlign::Left,
            600,
        );
        pen.text_middle(
            item_x + iw / 2.0,
            top + 20.0 + 4.0 + 8.0,
            &caption,
            12.0,
            quiet,
            TextAlign::Center,
            400,
        );
        if let Some(name) = server {
            pen.text_middle(
                item_x + iw / 2.0,
                top + 20.0 + 4.0 + 16.0 + 4.0 + 8.0,
                &name,
                12.0,
                quiet,
                TextAlign::Center,
                400,
            );
        }
        if is_selected {
            badge(&mut pen, it.status, item_x + iw + 8.0, item_y - 8.0);
        }

        let rect = Rect {
            x: pen.xf.ox + item_x * pen.xf.s,
            y: pen.xf.oy + item_y * pen.xf.s,
            w: iw * pen.xf.s,
            h: ih * pen.xf.s,
        };
        let emits = if editable {
            select_emits((!is_selected).then_some(it.id.as_str()))
        } else {
            Vec::new()
        };
        pen.hits.push(Hit { rect, emits });
        if editable && (move_event.is_some() || on_move.is_some()) {
            drags.push(Drag {
                rect,
                scene_scale: pen.xf.s,
                scene_origin: (pen.xf.ox, pen.xf.oy),
                max: ((w - iw).max(0.0), (h - ih).max(0.0)),
                id: it.id.clone(),
                moves: [on_move.clone(), move_event.clone()]
                    .into_iter()
                    .flatten()
                    .collect(),
                pin: false,
            });
        }
    }
    let mut scene = pen.finish("positioned-canvas", pw, ph);
    scene.drags = drags;
    Ok(scene)
}

fn mix_wash(pen: &Pen) -> [f32; 4] {
    super::draw::mix(pen.tone("ink"), 6.0, pen.ground())
}

/// The status badge pinned to a selected item's top-right (`Badge` size `sm`).
fn badge(pen: &mut Pen, status: Status, right: f32, top: f32) {
    let text = status.label();
    let tw = crate::vector::text_width(text, 12.0);
    let (bw, bh) = (tw + 16.0, 20.0);
    let x = right - bw;
    let (tone, ink, ground) = (pen.tone(status.tone()), pen.tone("ink"), pen.ground());
    let base = if status == Status::Empty || status == Status::Cleaning {
        mix_wash(pen)
    } else {
        tint(tone, 10.0)
    };
    pen.rrect(x, top, bw, bh, bh / 2.0, Some(ground), None);
    pen.rrect(
        x,
        top,
        bw,
        bh,
        bh / 2.0,
        Some(base),
        Some(&Ink::new(tone, 1.0)),
    );
    pen.text_middle(
        x + bw / 2.0,
        top + bh / 2.0,
        text,
        12.0,
        ink,
        TextAlign::Center,
        500,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::DrawCommand;

    /// `std-restaurant-floor`-shaped rows: a seated round table with a party and server, an empty
    /// square, a rectangle waiting on the bill, and a table being cleaned.
    fn plan() -> Value {
        json!({
            "type": "positioned-canvas", "width": 600, "height": 400, "editable": true, "selectedId": "t1",
            "selectEvent": "SELECT_TABLE", "moveEvent": "MOVE_TABLE",
            "items": [
                {"id": "t1", "label": "T1", "x": 40, "y": 40, "capacity": 4, "partySize": 3, "serverName": "Sam", "status": "seated", "shape": "round"},
                {"id": "t2", "label": "T2", "x": 200, "y": 40, "capacity": 2, "status": "empty", "shape": "square"},
                {"id": "t3", "label": "T3", "x": 360, "y": 40, "capacity": 6, "status": "awaiting-bill", "shape": "rectangle"},
                {"id": "t4", "label": "T4", "x": 40, "y": 200, "capacity": 8, "status": "cleaning", "shape": "round"}
            ]
        })
    }

    fn texts(s: &MappedScene) -> Vec<String> {
        s.packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn items_draw_their_label_capacity_and_party_text() {
        let t = texts(&map(&plan(), None).unwrap());
        for want in [
            "T1", "3/4", "Sam", "T2", "Cap 2", "T3", "Cap 6", "T4", "Cap 8",
        ] {
            assert!(t.iter().any(|x| x == want), "{want} missing from {t:?}");
        }
        assert!(
            !t.iter().any(|x| x == "Cap 4"),
            "a seated table with a party shows party/capacity instead"
        );
    }

    #[test]
    fn only_the_selected_item_wears_the_status_badge() {
        assert!(texts(&map(&plan(), None).unwrap())
            .iter()
            .any(|x| x == "Seated"));
        let mut none = plan();
        none["selectedId"] = json!("");
        assert!(
            !texts(&map(&none, None).unwrap())
                .iter()
                .any(|x| x == "Seated"),
            "control: nothing selected, no badge"
        );
    }

    #[test]
    fn tapping_an_item_toggles_its_selection_and_the_plan_clears_it() {
        let scene = map(&plan(), None).unwrap();
        let on_t1 = scene.emits_at(88.0, 88.0);
        assert_eq!(on_t1[0].event, "SELECT_TABLE");
        assert_eq!(
            on_t1[0].delivered(),
            json!({"id": null}),
            "the selected item toggles off"
        );
        let on_t2 = scene.emits_at(248.0, 88.0);
        assert_eq!(
            on_t2[0].delivered(),
            json!({"id": "t2"}),
            "an unselected item selects"
        );
        let blank = scene.emits_at(500.0, 350.0);
        assert_eq!(
            blank[0].delivered(),
            json!({"id": null}),
            "the plan itself clears the selection"
        );
    }

    #[test]
    fn a_read_only_plan_swallows_item_taps_but_the_plan_still_clears() {
        let mut p = plan();
        p["editable"] = json!(false);
        let scene = map(&p, None).unwrap();
        assert!(
            scene.emits_at(248.0, 88.0).is_empty(),
            "items do nothing when not editable"
        );
        assert_eq!(
            scene.emits_at(500.0, 350.0)[0].delivered(),
            json!({"id": null})
        );
        assert!(scene.drags.is_empty());
        let mut inert = plan();
        inert.as_object_mut().unwrap().remove("selectEvent");
        inert.as_object_mut().unwrap().remove("moveEvent");
        let silent = map(&inert, None).unwrap();
        assert!(
            silent.emits_at(500.0, 350.0).is_empty(),
            "control: no select wiring, no plan tap"
        );
        assert!(silent.drags.is_empty());
    }

    #[test]
    fn editable_items_are_draggable_within_the_plan_bounds() {
        let scene = map(&plan(), None).unwrap();
        assert_eq!(scene.drags.len(), 4);
        let t3 = scene.drags.iter().find(|d| d.id == "t3").unwrap();
        assert_eq!(
            t3.max,
            (600.0 - 144.0, 400.0 - 80.0),
            "a rectangle is 144 x 80"
        );
        assert_eq!(t3.moves, vec!["MOVE_TABLE".to_string()]);
        let t1 = scene.drags.iter().find(|d| d.id == "t1").unwrap();
        assert_eq!(t1.max, (600.0 - 96.0, 400.0 - 96.0));
    }

    #[test]
    fn status_picks_the_border_tone_and_a_cleaning_table_is_washed() {
        let theme = theme_for(&plan());
        let scene = map(&plan(), None).unwrap();
        let strokes: Vec<[f32; 4]> = scene
            .packet
            .draw_commands
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Path {
                    stroke: Some(s),
                    fill: Some(_),
                    ..
                } => Some(s.color),
                _ => None,
            })
            .collect();
        assert!(
            strokes.contains(&theme.tone("success").unwrap()),
            "seated borders in success"
        );
        assert!(
            strokes.contains(&theme.tone("info").unwrap()),
            "awaiting-bill borders in info"
        );
        assert!(
            !strokes.contains(&theme.tone("warning").unwrap()),
            "control: nothing is ordered"
        );
    }

    #[test]
    fn rows_without_an_id_are_skipped_and_the_scene_has_the_authored_size() {
        let mut p = plan();
        p["items"] = json!([{"label": "no id", "x": 1, "y": 1, "capacity": 2}]);
        let scene = map(&p, None).unwrap();
        assert!(!texts(&scene).iter().any(|x| x == "no id"));
        assert_eq!((scene.packet.width, scene.packet.height), (600.0, 400.0));
    }
}
