//! Frame origins: a top-level drawable that names an `origin` is placed in that origin's space, and moves with it as a
//! `draw-group` at the origin's offset would, so a moved origin re-walks only its own drawables.

use super::*;
use std::collections::HashMap;

/// The frame's `origins` (`{ <id>: { x, y, origin? } }`), each resolved to its offset from the page through its
/// `origin` chain. A chain that names a missing origin or returns to itself is refused.
pub(super) fn resolve_origins(props: &Value) -> Result<HashMap<String, (f32, f32)>, CanvasSceneError> {
    let Some(table) = props.get("origins").filter(|o| !o.is_null()) else {
        return Ok(HashMap::new());
    };
    let table = table
        .as_object()
        .ok_or_else(|| invalid("origins", "origins is an object of { x, y, origin? } by id"))?;
    let mut resolved = HashMap::with_capacity(table.len());
    for id in table.keys() {
        let mut at = (0.0f32, 0.0f32);
        let mut seen: Vec<&str> = Vec::new();
        let mut current = id.as_str();
        loop {
            if seen.contains(&current) {
                return Err(invalid("origins", format!("`{id}` reaches `{current}` again through its origins")));
            }
            seen.push(current);
            let entry = table
                .get(current)
                .ok_or_else(|| invalid("origins", format!("`{}` names origin `{current}`, which the frame does not list", seen[seen.len().saturating_sub(2)])))?;
            at.0 += get_f32(entry, "x", 0.0);
            at.1 += get_f32(entry, "y", 0.0);
            match entry.get("origin").and_then(Value::as_str).filter(|o| !o.is_empty()) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        resolved.insert(id.clone(), at);
    }
    Ok(resolved)
}

/// The resolved offset of the origin `node` names, `None` for a drawable on the page.
pub(super) fn offset_of(node: &Value, origins: &HashMap<String, (f32, f32)>) -> Result<Option<(f32, f32)>, CanvasSceneError> {
    match node.get("origin").and_then(Value::as_str).filter(|o| !o.is_empty()) {
        None => Ok(None),
        Some(id) => origins
            .get(id)
            .copied()
            .map(Some)
            .ok_or_else(|| invalid("origin", format!("names `{id}`, which the frame's origins do not list"))),
    }
}

/// Move what one top-level drawable walked (from `out_start` and `starts`: regions, measured, text hits, semantic) to
/// its origin's `offset`, as a `draw-group` at that position moves its items, and mark its regions with the offset.
pub(super) fn move_to_origin(
    offset: (f32, f32),
    projector: &Projector,
    to_screen: &dyn Fn(f32, f32) -> (f32, f32),
    gathered: &mut Gathered,
    out: &mut [DrawCommand],
    out_start: usize,
    starts: (usize, usize, usize, usize),
) {
    let (region_start, measured_start, hits_start, semantic_start) = starts;
    let (gx, gy) = projector.project(ScenePos { x: offset.0, y: offset.1 });
    let (zero_x, zero_y) = to_screen(0.0, 0.0);
    let (x, y) = to_screen(gx, gy);
    group::transform_walked(
        gathered,
        out,
        group::Walked {
            commands: out_start,
            regions: region_start,
            measured: measured_start,
            hits: hits_start,
            semantic: semantic_start,
        },
        Vec2 { x: zero_x, y: zero_y },
        Vec2 { x: x - zero_x, y: y - zero_y },
        1.0,
        0.0,
        1.0,
    );
    for region in &mut gathered.regions[region_start..] {
        region.origin.get_or_insert(offset);
    }
}
