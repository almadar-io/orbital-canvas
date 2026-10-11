//! `AvlEffect`'s glyph table (`components/avl/atoms/AvlEffect.tsx`): an effect operator's own
//! glyph, else its category's, else the dashed unknown mark, drawn in the category color over
//! the category tint (`AvlEffectChip` with `showBackground`).

use super::draw::{mix, tint, with_alpha, Ink, Pen};
use crate::mappers::learning::theme::Rgba;
use crate::packet::TextAlign;

pub(crate) fn effect_category(head: &str) -> Option<&'static str> {
    Some(match head {
        "render-ui" | "navigate" | "navigate-back" => "ui",
        "set" | "persist" | "fetch" | "fetch-stream" | "ref" | "deref" | "swap" | "watch" => "data",
        "emit"
        | "notify"
        | "send-server"
        | "call-service"
        | "cancel-call"
        | "integration/http"
        | "integration/github-get-repo"
        | "integration/github-create-issue" => "communication",
        "spawn" | "despawn" => "lifecycle",
        "do" | "if" | "when" | "let" | "log" | "atomic" => "control",
        "wait" | "async/delay" | "async/debounce" | "async/throttle" | "async/interval"
        | "async/race" | "async/all" | "async/sequence" | "async/timeout" | "async/retry" => {
            "async"
        }
        "forward"
        | "train"
        | "evaluate"
        | "checkpoint/save"
        | "checkpoint/load"
        | "nn/setWeights"
        | "train/loop"
        | "train/step"
        | "train/clipGradients"
        | "train/clipWeights"
        | "train/sgd"
        | "train/adam"
        | "prob/seed"
        | "prob/condition" => "compute",
        "workspace/write-orbital"
        | "workspace/write-file"
        | "workspace/write-schema"
        | "workspace/write-plan"
        | "workspace/archive-orbital"
        | "behavior/catalog"
        | "behavior/describe"
        | "behavior/source"
        | "program/read"
        | "program/print"
        | "program/eval"
        | "program/compose" => "system",
        other => match other.split_once('/').map(|(p, _)| p) {
            Some("agent" | "os" | "browser" | "llm" | "session" | "application") => "system",
            Some("trace") => "control",
            Some("memory") => "data",
            _ => return None,
        },
    })
}

pub(crate) fn category_color(pen: &Pen, category: &str) -> Rgba {
    match category {
        "ui" => pen.tone("accent"),
        "data" => pen.tone("info"),
        "communication" => pen.tone("warning"),
        "lifecycle" => pen.tone("success"),
        "async" => mix(pen.tone("info"), 50.0, pen.tone("accent")),
        "compute" => pen.tone("primary"),
        "system" => mix(pen.tone("label"), 60.0, pen.tone("primary")),
        _ => pen.tone("label"),
    }
}

/// A chip `size` wide centered on (`x`, `y`): tinted disc, faint ring, then the glyph.
pub fn chip(pen: &mut Pen, op: &str, x: f32, y: f32, size: f32) {
    let s = size / 2.0 / 1.2 - 0.5;
    let category = effect_category(op);
    let c = category.map_or_else(|| pen.tone("label"), |cat| category_color(pen, cat));
    if category.is_some() {
        pen.circle(x, y, s * 1.2, Some(tint(c, 10.0)), None);
        pen.circle(
            x,
            y,
            s * 1.2,
            None,
            Some(&Ink::new(with_alpha(c, 0.3), 0.5)),
        );
    }
    glyph(pen, op, x, y, s, c);
}

/// The glyph alone, `s` its half-size.
pub fn glyph(pen: &mut Pen, op: &str, x: f32, y: f32, s: f32, c: Rgba) {
    if !own_glyph(pen, op, x, y, s, c) {
        match effect_category(op) {
            Some("ui") => own(pen, "render-ui", x, y, s, c),
            Some("data") => own(pen, "set", x, y, s, c),
            Some("communication") => own(pen, "emit", x, y, s, c),
            Some("lifecycle") => own(pen, "spawn", x, y, s, c),
            Some("control") => own(pen, "if", x, y, s, c),
            Some("async") => clock(pen, x, y, s, c),
            Some("compute") => neuron(pen, x, y, s, c),
            Some("system") => gear(pen, x, y, s, c),
            _ => unknown(pen, x, y, s, c),
        }
    }
}

fn own(pen: &mut Pen, op: &str, x: f32, y: f32, s: f32, c: Rgba) {
    own_glyph(pen, op, x, y, s, c);
}

/// `headIcon`: the operators that carry their own glyph; false for every other operator.
fn own_glyph(pen: &mut Pen, op: &str, x: f32, y: f32, s: f32, c: Rgba) -> bool {
    let w15 = Ink::new(c, 1.5);
    let w1 = Ink::new(c, 1.0);
    match op {
        "render-ui" => {
            pen.rrect(x - s, y - s, s * 2.0, s * 2.0, 1.0, None, Some(&w15));
            pen.line((x, y - s), (x, y + s), &w1);
            pen.line((x - s, y), (x + s, y), &w1);
        }
        "set" => pen.polyline(
            &[
                (x - s * 0.6, y + s),
                (x - s, y + s * 0.4),
                (x + s * 0.4, y - s),
                (x + s, y - s * 0.4),
            ],
            true,
            None,
            Some(&w15),
        ),
        "persist" => {
            pen.ellipse(x, y - s * 0.6, s, s * 0.4, 0.0, None, Some(&w15));
            pen.line((x - s, y - s * 0.6), (x - s, y + s * 0.4), &w15);
            pen.line((x + s, y - s * 0.6), (x + s, y + s * 0.4), &w15);
            pen.ellipse(x, y + s * 0.4, s, s * 0.4, 0.0, None, Some(&w15));
        }
        "fetch" => {
            pen.line((x, y - s), (x, y + s * 0.6), &w15);
            pen.polyline(
                &[
                    (x - s * 0.5, y + s * 0.1),
                    (x, y + s),
                    (x + s * 0.5, y + s * 0.1),
                ],
                false,
                None,
                Some(&w15),
            );
        }
        "emit" => {
            pen.circle(x, y, s * 0.3, Some(c), None);
            pen.svg_path(
                &format!(
                    "M{},{} A{s},{s} 0 0,1 {},{}",
                    x - s * 0.7,
                    y - s * 0.7,
                    x + s * 0.7,
                    y - s * 0.7
                ),
                None,
                Some(&w15),
            );
            let r = s * 1.4;
            pen.svg_path(
                &format!("M{},{} A{r},{r} 0 0,1 {},{}", x - s, y - s, x + s, y - s),
                None,
                Some(&Ink::new(with_alpha(c, 0.6), 1.0)),
            );
        }
        "navigate" => {
            pen.line((x - s, y), (x + s * 0.6, y), &w15);
            pen.polyline(
                &[
                    (x + s * 0.1, y - s * 0.5),
                    (x + s, y),
                    (x + s * 0.1, y + s * 0.5),
                ],
                false,
                None,
                Some(&w15),
            );
        }
        "call-service" => {
            pen.line((x - s, y - s * 0.3), (x + s, y - s * 0.3), &w15);
            pen.polyline(
                &[
                    (x + s * 0.5, y - s * 0.7),
                    (x + s, y - s * 0.3),
                    (x + s * 0.5, y + s * 0.1),
                ],
                false,
                None,
                Some(&w15),
            );
            pen.line((x + s, y + s * 0.3), (x - s, y + s * 0.3), &w15);
            pen.polyline(
                &[
                    (x - s * 0.5, y - s * 0.1),
                    (x - s, y + s * 0.3),
                    (x - s * 0.5, y + s * 0.7),
                ],
                false,
                None,
                Some(&w15),
            );
        }
        "spawn" => {
            pen.circle(x, y, s, None, Some(&w15));
            pen.line((x - s * 0.5, y), (x + s * 0.5, y), &w15);
            pen.line((x, y - s * 0.5), (x, y + s * 0.5), &w15);
        }
        "despawn" => {
            pen.circle(x, y, s, None, Some(&w15));
            pen.line((x - s * 0.5, y), (x + s * 0.5, y), &w15);
        }
        "do" => {
            pen.circle(x, y, s, None, Some(&w15));
            pen.polyline(
                &[
                    (x - s * 0.3, y - s * 0.5),
                    (x + s * 0.5, y),
                    (x - s * 0.3, y + s * 0.5),
                ],
                true,
                Some(c),
                None,
            );
        }
        "if" => pen.polyline(
            &[(x, y - s), (x + s * 0.7, y), (x, y + s), (x - s * 0.7, y)],
            true,
            None,
            Some(&w15),
        ),
        "log" => pen.text_middle(x, y, "\u{b6}", s * 2.2, c, TextAlign::Center, 400),
        _ => return false,
    }
    true
}

fn clock(pen: &mut Pen, x: f32, y: f32, s: f32, c: Rgba) {
    let w = Ink::new(c, 1.5);
    pen.circle(x, y, s, None, Some(&w));
    pen.polyline(
        &[(x, y - s * 0.6), (x, y), (x + s * 0.5, y + s * 0.3)],
        false,
        None,
        Some(&w),
    );
}

fn neuron(pen: &mut Pen, x: f32, y: f32, s: f32, c: Rgba) {
    let w = Ink::new(c, 1.0);
    pen.line((x - s, y - s * 0.6), (x + s * 0.8, y), &w);
    pen.line((x - s, y + s * 0.6), (x + s * 0.8, y), &w);
    pen.circle(x - s, y - s * 0.6, s * 0.3, Some(c), None);
    pen.circle(x - s, y + s * 0.6, s * 0.3, Some(c), None);
    pen.circle(x + s * 0.8, y, s * 0.35, Some(c), None);
}

fn gear(pen: &mut Pen, x: f32, y: f32, s: f32, c: Rgba) {
    pen.circle(
        x,
        y,
        s * 0.75,
        None,
        Some(&Ink::new(c, 1.5).dashed(&[s * 0.5, s * 0.3])),
    );
    pen.circle(x, y, s * 0.3, Some(c), None);
}

fn unknown(pen: &mut Pen, x: f32, y: f32, s: f32, c: Rgba) {
    pen.circle(x, y, s, None, Some(&Ink::new(c, 1.2).dashed(&[2.0, 2.0])));
    pen.text_middle(x, y, "?", s * 1.4, c, TextAlign::Center, 400);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mappers::learning::theme::DiagramTheme;
    use crate::mappers::learning::Xf;
    use crate::packet::DrawCommand;

    fn draw(op: &str) -> Vec<DrawCommand> {
        let theme = DiagramTheme::heritage(false);
        let mut pen = Pen::new(&theme, Xf::identity());
        glyph(&mut pen, op, 10.0, 10.0, 6.0, theme.tone("info").unwrap());
        pen.cmds
    }

    #[test]
    fn operators_with_their_own_glyph_draw_distinct_marks() {
        let ops = [
            "render-ui",
            "set",
            "persist",
            "fetch",
            "emit",
            "navigate",
            "call-service",
            "spawn",
            "despawn",
            "do",
            "if",
            "log",
        ];
        let drawn: Vec<String> = ops.iter().map(|op| format!("{:?}", draw(op))).collect();
        for (i, a) in drawn.iter().enumerate() {
            assert!(!a.is_empty() && a != "[]", "{} draws nothing", ops[i]);
            for (j, b) in drawn.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "{} and {} share a glyph", ops[i], ops[j]);
            }
        }
    }

    #[test]
    fn an_operator_without_its_own_glyph_takes_its_categorys() {
        assert_eq!(
            format!("{:?}", draw("notify")),
            format!("{:?}", draw("emit")),
            "communication"
        );
        assert_eq!(
            format!("{:?}", draw("deref")),
            format!("{:?}", draw("set")),
            "data"
        );
        assert_ne!(
            format!("{:?}", draw("async/delay")),
            format!("{:?}", draw("if")),
            "async is the clock"
        );
    }

    #[test]
    fn control_an_unknown_operator_draws_the_dashed_question_mark() {
        let cmds = draw("no-such-op");
        assert!(
            cmds.iter()
                .any(|c| matches!(c, DrawCommand::Text { text, .. } if text == "?")),
            "{cmds:?}"
        );
    }

    #[test]
    fn the_emit_arcs_are_curves() {
        let cmds = draw("emit");
        let curves = cmds
            .iter()
            .filter(|c| matches!(c, DrawCommand::Path { segments, .. } if segments.iter().any(|s| matches!(s, crate::packet::PathSegment::CubicTo { .. }))))
            .count();
        assert_eq!(curves, 2, "{cmds:?}");
    }
}
