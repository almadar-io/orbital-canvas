//! Render targets as data (rasm floor contract 14.7): which commands read a target, whether a target's paint
//! depends on the clock, cycles between readers, and the hash of what a run of commands draws.

use crate::packet::DrawCommand;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

/// The prefix of a sprite `texture` that names a render target.
pub const TARGET_PREFIX: &str = "target:";

/// Visit `commands` and every command nested in them; `into_targets` also descends into a `Target`'s children.
pub fn visit(commands: &[DrawCommand], into_targets: bool, f: &mut dyn FnMut(&DrawCommand)) {
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
pub fn reads(commands: &[DrawCommand]) -> BTreeSet<String> {
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
pub fn has_live_material(commands: &[DrawCommand]) -> bool {
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
pub fn find_cycle(commands: &[DrawCommand]) -> Option<String> {
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

/// Feed each command's content, in order, into `state`: the one hash of what a run of commands draws.
pub fn hash_commands<'a>(
    commands: impl IntoIterator<Item = &'a DrawCommand>,
    state: &mut impl Hasher,
) -> serde_json::Result<()> {
    for command in commands {
        serde_json::to_string(command)?.hash(state);
    }
    Ok(())
}

/// Commands that read targets, for the tests here and in the crates that paint them.
#[cfg(any(test, feature = "test-support"))]
pub mod fixtures {
    use crate::packet::{DrawCommand, Rect, SdfKind, SdfMaterial};

    pub fn sprite(texture: &str) -> DrawCommand {
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

    pub fn target(id: &str, commands: Vec<DrawCommand>) -> DrawCommand {
        DrawCommand::Target {
            id: id.into(),
            width: 8,
            height: 8,
            live: false,
            commands,
        }
    }

    pub fn material_shape(targets: &[&str], live: bool) -> DrawCommand {
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
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

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
    fn hashed_commands_follow_their_content() {
        let key = |commands: &[DrawCommand]| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            hash_commands(commands, &mut hasher).expect("serializable");
            hasher.finish()
        };
        assert_eq!(
            key(&[sprite("a"), sprite("b")]),
            key(&[sprite("a"), sprite("b")])
        );
        assert_ne!(
            key(&[sprite("a"), sprite("b")]),
            key(&[sprite("b"), sprite("a")]),
            "order matters"
        );
        assert_ne!(
            key(&[sprite("a")]),
            key(&[sprite("c")]),
            "control: other content"
        );
    }
}
