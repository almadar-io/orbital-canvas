//! What each top-level drawable's walk produced, kept across mappings of one scene: a drawable that is unchanged in
//! an unchanged walk context is replayed from it instead of walked again. Its indexes into the command and semantic
//! lists are stored relative to where it began and shifted to where it lands now.

use super::*;
use std::collections::HashMap;
use std::sync::Arc;

/// A top-level drawable as the walk reads it: a plain value, or a shared one whose handle tells an unchanged drawable
/// from a changed one without comparing them.
pub trait SharedDrawable {
    fn value(&self) -> &Value;
    fn shared(&self) -> Option<&Arc<Value>>;
    fn to_shared(&self) -> Arc<Value>;
}

impl SharedDrawable for Value {
    fn value(&self) -> &Value {
        self
    }
    fn shared(&self) -> Option<&Arc<Value>> {
        None
    }
    fn to_shared(&self) -> Arc<Value> {
        Arc::new(self.clone())
    }
}

impl SharedDrawable for Arc<Value> {
    fn value(&self) -> &Value {
        self
    }
    fn shared(&self) -> Option<&Arc<Value>> {
        Some(self)
    }
    fn to_shared(&self) -> Arc<Value> {
        Arc::clone(self)
    }
}

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
    /// Sprite sizes and text layout read the assets and fonts loaded so far.
    pub asset_generation: u64,
    pub font_generation: u64,
}

/// One drawable's walk output, its indexes relative to the command and semantic counts it began at.
#[derive(Debug, Clone)]
struct Segment {
    node: Arc<Value>,
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
    /// The resolved offset of the origin it was walked at.
    offset: Option<(f32, f32)>,
    /// The clock it was walked at (infinite when the scene had settled), and the instant it stops moving with the
    /// clock (`None` when it never does): walked before that instant, its output is that clock's alone.
    walked_at: f64,
    moves_until: Option<f64>,
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
    pub(super) fn replay<D: SharedDrawable>(
        &mut self,
        node: &D,
        index: usize,
        offset: Option<(f32, f32)>,
        gathered: &mut Gathered,
        out: &mut Vec<DrawCommand>,
    ) -> bool {
        let Some(segment) = self.segments.get_mut(&key(node.value(), index)) else {
            return false;
        };
        let (out_start, semantic_start) = (out.len(), gathered.semantic.len());
        let identical = node.shared().is_some_and(|shared| Arc::ptr_eq(shared, &segment.node));
        let same = identical || *segment.node == *node.value();
        if same && !identical && node.shared().is_some() {
            // Hold the copy just shown: the next mapping of it matches by identity, not by a deep comparison.
            segment.node = node.to_shared();
        }
        let moved = segment.moves_until.is_some_and(|until| segment.walked_at < until);
        if !same || moved || segment.offset != offset || segment.paint_ordered_at.is_some_and(|at| at != semantic_start) {
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
    pub(super) fn walk<D: SharedDrawable>(
        &mut self,
        node: &D,
        index: usize,
        offset: Option<(f32, f32)>,
        projector: &Projector,
        to_screen: &dyn Fn(f32, f32) -> (f32, f32),
        zoom: f32,
        epoch_now_ms: f64,
        settled: bool,
        gathered: &mut Gathered,
        out: &mut Vec<DrawCommand>,
    ) -> Result<(), CanvasSceneError> {
        let walked_at = if settled { f64::INFINITY } else { epoch_now_ms };
        let moves_until = match super::animation::clocked_span(node.value()) {
            (true, until) => Some(until.unwrap_or(f64::INFINITY)),
            (false, _) => None,
        };
        let out_start = out.len();
        let (region_start, measured_start) = (gathered.regions.len(), gathered.measured.len());
        let (hit_start, semantic_start) = (gathered.text_hits.len(), gathered.semantic.len());
        let (pinned_start, animation_start) = (gathered.pinned.len(), gathered.animations.len());
        let (live_start, scene3d_before) = (gathered.live_scenes3d.len(), gathered.scene3d_count);
        let texts = std::mem::take(&mut gathered.texts);
        let labelled = std::mem::take(&mut gathered.labelled);
        let valued = std::mem::take(&mut gathered.valued);
        let walked = super::walk(node.value(), projector, to_screen, zoom, epoch_now_ms, gathered, out).map(|()| {
            if let Some(offset) = offset {
                super::origin::move_to_origin(offset, projector, to_screen, gathered, out, out_start, (region_start, measured_start, hit_start, semantic_start));
            }
        });
        let own_texts = std::mem::replace(&mut gathered.texts, texts);
        let own_labelled = std::mem::replace(&mut gathered.labelled, labelled);
        let own_valued = std::mem::replace(&mut gathered.valued, valued);
        gathered.texts.extend(own_texts.iter().map(|(k, v)| (k.clone(), v.clone())));
        gathered.labelled.extend(own_labelled.iter().cloned());
        gathered.valued.extend(own_valued.iter().cloned());
        walked?;
        let key = key(node.value(), index);
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
                node: node.to_shared(),
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
                offset,
                walked_at,
                moves_until,
            },
        );
        Ok(())
    }
}
