//! What each top-level drawable's walk produced, kept across mappings of one scene: a drawable that is unchanged in
//! an unchanged walk context is replayed from it instead of walked again. Its indexes into the command and semantic
//! lists are stored relative to where it began and shifted to where it lands now.

use super::*;
use std::collections::HashMap;

/// Everything a walk's output depends on besides the drawable itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WalkContext {
    pub width: f32,
    pub height: f32,
    pub zoom: f32,
    pub camera: (f32, f32),
    pub projector_scale: f32,
    pub base_offset_x: f32,
    pub projection: Projection,
    /// The clock the walk reads, `None` once the scene has settled and no drawable moves with it.
    pub clock: Option<f64>,
    /// Sprite sizes and text layout read the assets and fonts loaded so far.
    pub asset_generation: u64,
    pub font_generation: u64,
}

/// One drawable's walk output, its indexes relative to the command and semantic counts it began at.
#[derive(Debug, Clone)]
struct Segment {
    node: Value,
    commands: Vec<DrawCommand>,
    regions: Vec<Region>,
    measured: Vec<Measured>,
    text_hits: Vec<crate::scene::TextHit>,
    semantic: Vec<crate::scene::SemanticNode>,
    texts: HashMap<String, String>,
    labelled: Vec<(usize, String)>,
    valued: Vec<(usize, String)>,
    pinned: Vec<(usize, usize)>,
    animations: Vec<orbital_canvas_base::packet::Animation>,
    live_scenes3d: Vec<LiveScene3d>,
    /// The semantic count it began at, when a node it made takes its order from its paint position.
    paint_ordered_at: Option<usize>,
}

/// The drawables of the scene last mapped, by `id` (or index when a drawable has none).
#[derive(Debug, Clone, Default)]
pub struct DrawableCache {
    context: Option<WalkContext>,
    segments: HashMap<String, Segment>,
    /// How many drawables the last mapping replayed instead of walking.
    pub reused: usize,
}

fn key(node: &Value, index: usize) -> String {
    match node.get("id").and_then(Value::as_str) {
        Some(id) => format!("id:{id}"),
        None => format!("#{index}"),
    }
}

fn shift_animation(animation: &mut orbital_canvas_base::packet::Animation, by: isize) {
    let shift = |n: &mut usize| *n = (*n as isize + by) as usize;
    match animation.path.first_mut() {
        Some(top) => shift(top),
        None => {
            shift(&mut animation.first);
            shift(&mut animation.end);
        }
    }
}

impl DrawableCache {
    /// Start a mapping in `context`: a new context makes every kept output stale.
    pub(super) fn begin(&mut self, context: WalkContext) {
        if self.context != Some(context) {
            self.segments.clear();
            self.context = Some(context);
        }
        self.reused = 0;
    }

    /// Append the kept output of `node` at `index` when it is reusable here, saying whether it was.
    pub(super) fn replay(&mut self, node: &Value, index: usize, gathered: &mut Gathered, out: &mut Vec<DrawCommand>) -> bool {
        let Some(segment) = self.segments.get(&key(node, index)) else {
            return false;
        };
        let (out_start, semantic_start) = (out.len(), gathered.semantic.len());
        if segment.node != *node || segment.paint_ordered_at.is_some_and(|at| at != semantic_start) {
            return false;
        }
        out.extend(segment.commands.iter().cloned());
        gathered.regions.extend(segment.regions.iter().cloned());
        gathered.measured.extend(segment.measured.iter().cloned());
        gathered.text_hits.extend(segment.text_hits.iter().cloned());
        gathered.semantic.extend(segment.semantic.iter().cloned());
        gathered.texts.extend(segment.texts.iter().map(|(k, v)| (k.clone(), v.clone())));
        gathered.labelled.extend(segment.labelled.iter().map(|(i, t)| (i + semantic_start, t.clone())));
        gathered.valued.extend(segment.valued.iter().map(|(i, t)| (i + semantic_start, t.clone())));
        gathered.pinned.extend(segment.pinned.iter().map(|(a, b)| (a + out_start, b + out_start)));
        gathered.animations.extend(segment.animations.iter().cloned().map(|mut a| {
            shift_animation(&mut a, out_start as isize);
            a
        }));
        gathered.live_scenes3d.extend(segment.live_scenes3d.iter().cloned());
        self.reused += 1;
        true
    }

    /// Walk `node` at `index` and keep what it produced for the next mapping.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn walk(
        &mut self,
        node: &Value,
        index: usize,
        projector: &Projector,
        to_screen: &dyn Fn(f32, f32) -> (f32, f32),
        zoom: f32,
        epoch_now_ms: f64,
        gathered: &mut Gathered,
        out: &mut Vec<DrawCommand>,
    ) -> Result<(), CanvasSceneError> {
        let out_start = out.len();
        let (region_start, measured_start) = (gathered.regions.len(), gathered.measured.len());
        let (hit_start, semantic_start) = (gathered.text_hits.len(), gathered.semantic.len());
        let (pinned_start, animation_start) = (gathered.pinned.len(), gathered.animations.len());
        let (live_start, scene3d_before) = (gathered.live_scenes3d.len(), gathered.scene3d_count);
        let texts = std::mem::take(&mut gathered.texts);
        let labelled = std::mem::take(&mut gathered.labelled);
        let valued = std::mem::take(&mut gathered.valued);
        let walked = super::walk(node, projector, to_screen, zoom, epoch_now_ms, gathered, out);
        let own_texts = std::mem::replace(&mut gathered.texts, texts);
        let own_labelled = std::mem::replace(&mut gathered.labelled, labelled);
        let own_valued = std::mem::replace(&mut gathered.valued, valued);
        gathered.texts.extend(own_texts.iter().map(|(k, v)| (k.clone(), v.clone())));
        gathered.labelled.extend(own_labelled.iter().cloned());
        gathered.valued.extend(own_valued.iter().cloned());
        walked?;
        let key = key(node, index);
        // An unnamed 3D scene is named by how many came before it, which a replay elsewhere would not keep.
        if gathered.scene3d_count != scene3d_before || gathered.pinned.len() < pinned_start {
            self.segments.remove(&key);
            return Ok(());
        }
        let semantic = gathered.semantic[semantic_start..].to_vec();
        let paint_ordered = semantic
            .iter()
            .enumerate()
            .any(|(i, n)| n.order == (semantic_start + i) as f32);
        self.segments.insert(
            key,
            Segment {
                node: node.clone(),
                commands: out[out_start..].to_vec(),
                regions: gathered.regions[region_start..].to_vec(),
                measured: gathered.measured[measured_start..].to_vec(),
                text_hits: gathered.text_hits[hit_start..].to_vec(),
                semantic,
                texts: own_texts,
                labelled: own_labelled.into_iter().map(|(i, t)| (i - semantic_start, t)).collect(),
                valued: own_valued.into_iter().map(|(i, t)| (i - semantic_start, t)).collect(),
                pinned: gathered.pinned[pinned_start..].iter().map(|(a, b)| (a - out_start, b - out_start)).collect(),
                animations: gathered.animations[animation_start..]
                    .iter()
                    .cloned()
                    .map(|mut a| {
                        shift_animation(&mut a, -(out_start as isize));
                        a
                    })
                    .collect(),
                live_scenes3d: gathered.live_scenes3d[live_start..].to_vec(),
                paint_ordered_at: paint_ordered.then_some(semantic_start),
            },
        );
        Ok(())
    }
}
