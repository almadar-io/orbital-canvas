//! Render targets (rasm floor contract 14.7): a [`DrawCommand::Target`] paints its children into an offscreen
//! texture, a `Sprite` with `texture: "target:<id>"` paints it, and an SDF material samples it as `target_a`.
//!
//! A target is painted where it stands in the command list. A reader resolved before its writer reads the texture
//! the writer left in the previous frame (the store keeps two textures per target and flips on each repaint); a
//! reader after it reads this frame's. A cycle of readers is refused when the scene is mapped.

use crate::packet::DrawCommand;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

/// The prefix of a sprite `texture` that names a render target.
pub(crate) const TARGET_PREFIX: &str = "target:";

/// Visit `commands` and every command nested in them; `into_targets` also descends into a `Target`'s children.
pub(crate) fn visit(commands: &[DrawCommand], into_targets: bool, f: &mut dyn FnMut(&DrawCommand)) {
    for command in commands {
        f(command);
        match command {
            DrawCommand::SdfBackdrop { shape, .. } => {
                visit(std::slice::from_ref(shape), into_targets, f)
            }
            DrawCommand::SdfBlend { first, second, .. } => {
                visit(std::slice::from_ref(first), into_targets, f);
                visit(std::slice::from_ref(second), into_targets, f);
            }
            DrawCommand::Blurred { command, .. } => {
                visit(std::slice::from_ref(command), into_targets, f)
            }
            DrawCommand::Shadow { commands, .. }
            | DrawCommand::AdditiveGroup { commands }
            | DrawCommand::ClipGroup { commands, .. } => visit(commands, into_targets, f),
            DrawCommand::Target { commands, .. } if into_targets => {
                visit(commands, into_targets, f)
            }
            _ => {}
        }
    }
}

/// The target ids `commands` read (sprites and materials), not looking inside nested targets.
pub(crate) fn reads(commands: &[DrawCommand]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    visit(commands, false, &mut |command| match command {
        DrawCommand::Sprite { texture, .. } => {
            if let Some(id) = texture.strip_prefix(TARGET_PREFIX) {
                ids.insert(id.to_owned());
            }
        }
        DrawCommand::Sdf {
            material: Some(material),
            ..
        } => ids.extend(material.targets.iter().cloned()),
        _ => {}
    });
    ids
}

/// Whether anything in `commands` reads the clock through a live material.
pub(crate) fn has_live_material(commands: &[DrawCommand]) -> bool {
    let mut live = false;
    visit(commands, false, &mut |command| {
        if let DrawCommand::Sdf {
            material: Some(material),
            ..
        } = command
        {
            live |= material.live;
        }
    });
    live
}

/// The id of a target that (through the targets it reads) reads itself, if any; each target is a node, each read
/// an edge.
pub(crate) fn find_cycle(commands: &[DrawCommand]) -> Option<String> {
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    visit(commands, true, &mut |command| {
        if let DrawCommand::Target { id, commands, .. } = command {
            graph.entry(id.clone()).or_default().extend(reads(commands));
        }
    });
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Done,
    }
    fn dfs(
        node: &str,
        graph: &BTreeMap<String, BTreeSet<String>>,
        marks: &mut HashMap<String, Mark>,
    ) -> Option<String> {
        match marks.get(node) {
            Some(Mark::Open) => return Some(node.to_owned()),
            Some(Mark::Done) => return None,
            None => {}
        }
        marks.insert(node.to_owned(), Mark::Open);
        if let Some(edges) = graph.get(node) {
            for next in edges {
                if let Some(found) = dfs(next, graph, marks) {
                    return Some(found);
                }
            }
        }
        marks.insert(node.to_owned(), Mark::Done);
        None
    }
    let mut marks = HashMap::new();
    graph.keys().find_map(|id| dfs(id, &graph, &mut marks))
}

/// A hash of what a target paints: its size, its commands, the clock when it is live or holds a live material, the
/// font and asset generations, and the version of every target its children read.
pub(crate) fn content_hash(
    width: u32,
    height: u32,
    live: bool,
    commands: &[DrawCommand],
    time_ms: f64,
    version_of: &dyn Fn(&str) -> u64,
) -> Option<u64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (width, height).hash(&mut hasher);
    crate::vector::font_generation().hash(&mut hasher);
    crate::asset::generation().hash(&mut hasher);
    if live || has_live_material(commands) {
        time_ms.to_bits().hash(&mut hasher);
    }
    for id in reads(commands) {
        (&id, version_of(&id)).hash(&mut hasher);
    }
    for command in commands {
        serde_json::to_string(command).ok()?.hash(&mut hasher);
    }
    Some(hasher.finish())
}

/// One texture of a target, with the serial that tells apart two texture generations of the same id.
#[derive(Clone)]
pub(crate) struct TargetTexture {
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) serial: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

struct Entry {
    slots: [Option<TargetTexture>; 2],
    current: usize,
    painted_frame: u64,
    hash: u64,
    version: u64,
}

/// The targets of one renderer: two textures each, so a reader that resolves before its writer keeps last frame's.
#[derive(Default)]
pub(crate) struct TargetStore {
    entries: HashMap<String, Entry>,
    frame: u64,
    serial: u64,
}

impl TargetStore {
    /// Start a frame: targets the last frame did not paint are dropped.
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.entries.retain(|_, e| e.painted_frame + 1 >= frame);
    }

    pub(crate) fn version(&self, id: &str) -> u64 {
        self.entries.get(id).map_or(0, |e| e.version)
    }

    /// The texture a reader resolving now sees: this frame's if the writer already painted, else the last one's.
    pub(crate) fn read(&self, id: &str) -> Option<TargetTexture> {
        let entry = self.entries.get(id)?;
        entry.slots[entry.current].clone()
    }

    /// Whether `id` already holds `hash` at `size`, so it need not repaint; marks it painted this frame if so.
    pub(crate) fn keep(&mut self, id: &str, size: (u32, u32), hash: u64) -> bool {
        let frame = self.frame;
        match self.entries.get_mut(id) {
            Some(e)
                if e.hash == hash
                    && e.slots[e.current]
                        .as_ref()
                        .is_some_and(|t| (t.width, t.height) == size) =>
            {
                e.painted_frame = frame;
                true
            }
            _ => false,
        }
    }

    /// The texture to paint `id` into now: the slot that does not hold what readers resolved earlier saw.
    pub(crate) fn writer(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        id: &str,
        size: (u32, u32),
    ) -> TargetTexture {
        let entry = self.entries.entry(id.to_owned()).or_insert(Entry {
            slots: [None, None],
            current: 1,
            painted_frame: 0,
            hash: 0,
            version: 0,
        });
        entry.current ^= 1;
        let slot = entry.current;
        let fits = entry.slots[slot]
            .as_ref()
            .is_some_and(|t| (t.width, t.height) == size);
        if !fits {
            self.serial += 1;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("render target"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            entry.slots[slot] = Some(TargetTexture {
                texture,
                view,
                serial: self.serial,
                width: size.0,
                height: size.1,
            });
        } else if let Some(t) = entry.slots[slot].as_mut() {
            self.serial += 1;
            t.serial = self.serial;
        }
        entry.slots[slot].clone().expect("slot just filled")
    }

    /// Record that `id` was painted this frame with `hash`.
    pub(crate) fn painted(&mut self, id: &str, hash: u64) {
        if let Some(e) = self.entries.get_mut(id) {
            e.painted_frame = self.frame;
            e.hash = hash;
            e.version += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{Rect, SdfKind, SdfMaterial};

    fn sprite(texture: &str) -> DrawCommand {
        DrawCommand::Sprite {
            texture: texture.into(),
            src: Rect::default(),
            dst: Rect::default(),
            rotation: 0.0,
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
        }
    }

    fn target(id: &str, commands: Vec<DrawCommand>) -> DrawCommand {
        DrawCommand::Target {
            id: id.into(),
            width: 8,
            height: 8,
            live: false,
            commands,
        }
    }

    fn material_shape(targets: &[&str], live: bool) -> DrawCommand {
        DrawCommand::Sdf {
            kind: SdfKind::Rect,
            rect: Rect::default(),
            corners: [0.0; 4],
            corner_kind: 0,
            arc: [0.0; 3],
            polygon: [0.0; 2],
            fill: [1.0; 4],
            stroke: None,
            shadow: None,
            rotation: 0.0,
            opacity: 1.0,
            material: Some(SdfMaterial {
                wgsl: String::new(),
                params: Vec::new(),
                live,
                since: 0.0,
                targets: targets.iter().map(|t| (*t).to_owned()).collect(),
            }),
        }
    }

    #[test]
    fn reads_find_sprites_and_materials_and_skip_plain_textures() {
        let found = reads(&[
            sprite("target:a"),
            material_shape(&["b", "c"], false),
            sprite("hero.png"),
        ]);
        assert_eq!(found.into_iter().collect::<Vec<_>>(), ["a", "b", "c"]);
        assert!(
            reads(&[sprite("hero.png")]).is_empty(),
            "control: a file texture reads no target"
        );
    }

    #[test]
    fn a_target_that_reads_itself_or_loops_through_another_is_a_cycle() {
        assert_eq!(
            find_cycle(&[target("a", vec![sprite("target:a")])]),
            Some("a".into())
        );
        let loop_ = [
            target("a", vec![sprite("target:b")]),
            target("b", vec![material_shape(&["a"], false)]),
        ];
        assert!(find_cycle(&loop_).is_some());
        let chain = [target("a", vec![sprite("target:b")]), target("b", vec![])];
        assert_eq!(find_cycle(&chain), None, "control: a chain is no cycle");
        let backwards = [target("b", vec![]), target("a", vec![sprite("target:b")])];
        assert_eq!(find_cycle(&backwards), None, "control: order is no cycle");
    }

    #[test]
    fn the_content_hash_follows_children_size_clock_and_read_versions() {
        let kids = vec![sprite("target:x")];
        let v0 = |_: &str| 0u64;
        let base = content_hash(8, 8, false, &kids, 0.0, &v0).unwrap();
        assert_eq!(
            base,
            content_hash(8, 8, false, &kids, 99.0, &v0).unwrap(),
            "control: a still target ignores the clock"
        );
        assert_ne!(
            base,
            content_hash(9, 8, false, &kids, 0.0, &v0).unwrap(),
            "size"
        );
        assert_ne!(
            base,
            content_hash(8, 8, true, &kids, 99.0, &v0).unwrap(),
            "live reads the clock"
        );
        assert_ne!(
            base,
            content_hash(8, 8, false, &kids, 0.0, &|_: &str| 1u64).unwrap(),
            "a read target's version"
        );
        let live_kids = vec![material_shape(&[], true)];
        assert_ne!(
            content_hash(8, 8, false, &live_kids, 1.0, &v0).unwrap(),
            content_hash(8, 8, false, &live_kids, 2.0, &v0).unwrap(),
            "a live material reads the clock"
        );
    }
}
