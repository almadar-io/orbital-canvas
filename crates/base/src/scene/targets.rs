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
            | DrawCommand::Faded { commands, .. }
            | DrawCommand::ClipGroup { commands, .. } => visit(commands, into_targets, f),
            DrawCommand::Target { commands, .. } if into_targets => {
                visit(commands, into_targets, f)
            }
            _ => {}
        }
    }
}

/// Set every anchored material's `since` to the seconds from its anchor to `epoch_ms`, everywhere in `commands`.
pub fn stamp_since(commands: &mut [DrawCommand], epoch_ms: f64) {
    for command in commands {
        match command {
            DrawCommand::Sdf {
                material: Some(material),
                ..
            } => {
                if let Some(anchor) = material.anchor_ms {
                    material.since = ((epoch_ms - anchor) / 1000.0) as f32;
                }
            }
            DrawCommand::SdfBackdrop { shape, .. } => {
                stamp_since(std::slice::from_mut(shape), epoch_ms)
            }
            DrawCommand::SdfBlend { first, second, .. } => {
                stamp_since(std::slice::from_mut(first), epoch_ms);
                stamp_since(std::slice::from_mut(second), epoch_ms);
            }
            DrawCommand::Blurred { command, .. } => {
                stamp_since(std::slice::from_mut(command), epoch_ms)
            }
            DrawCommand::Shadow { commands, .. }
            | DrawCommand::AdditiveGroup { commands }
            | DrawCommand::Faded { commands, .. }
            | DrawCommand::ClipGroup { commands, .. }
            | DrawCommand::Target { commands, .. } => stamp_since(commands, epoch_ms),
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

/// Steps per unit a number is rounded to in [`hash_commands_from`]: finer than any pixel or 8-bit colour step.
const HASH_STEPS_PER_UNIT: f64 = 4096.0;

/// [`hash_commands`] of the commands as they draw relative to `origin`, every number rounded to
/// 1/[`HASH_STEPS_PER_UNIT`]: a run moved as a whole, as scrolling and translating motion move it, hashes the same.
pub fn hash_commands_from<'a>(
    commands: impl IntoIterator<Item = &'a DrawCommand>,
    origin: crate::packet::Vec2,
    state: &mut impl Hasher,
) -> serde_json::Result<()> {
    let mut moved: Vec<DrawCommand> = commands.into_iter().cloned().collect();
    crate::animate::transform_group_commands(
        &mut moved,
        crate::packet::Vec2::default(),
        crate::packet::Vec2 {
            x: -origin.x,
            y: -origin.y,
        },
        1.0,
        0.0,
        1.0,
    );
    for command in &moved {
        serde::Serialize::serialize(command, &mut RoundedHash(state))?;
    }
    Ok(())
}

/// A serializer that feeds what it is given into a hasher, every number rounded to 1/[`HASH_STEPS_PER_UNIT`]: the
/// content of a command without building it as a value first.
struct RoundedHash<'a, H: Hasher>(&'a mut H);

const HASH_NULL: u8 = 0;
const HASH_BOOL: u8 = 1;
const HASH_NUMBER: u8 = 2;
const HASH_TEXT: u8 = 4;
const HASH_LIST: u8 = 5;
const HASH_FIELDS: u8 = 6;
const HASH_END: u8 = 7;

impl<H: Hasher> RoundedHash<'_, H> {
    fn number(&mut self, n: f64) {
        (HASH_NUMBER, (n * HASH_STEPS_PER_UNIT).round() as i64).hash(self.0);
    }
}

type HashResult = Result<(), serde_json::Error>;

impl<'a, 'b, H: Hasher> serde::Serializer for &'a mut RoundedHash<'b, H> {
    type Ok = ();
    type Error = serde_json::Error;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> HashResult {
        (HASH_BOOL, v).hash(self.0);
        Ok(())
    }
    fn serialize_i8(self, v: i8) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_i16(self, v: i16) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_i32(self, v: i32) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_i64(self, v: i64) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_u8(self, v: u8) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_u16(self, v: u16) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_u32(self, v: u32) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_u64(self, v: u64) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> HashResult {
        self.number(v as f64);
        Ok(())
    }
    fn serialize_f64(self, v: f64) -> HashResult {
        self.number(v);
        Ok(())
    }
    fn serialize_char(self, v: char) -> HashResult {
        (HASH_TEXT, v).hash(self.0);
        Ok(())
    }
    fn serialize_str(self, v: &str) -> HashResult {
        (HASH_TEXT, v).hash(self.0);
        Ok(())
    }
    fn serialize_bytes(self, v: &[u8]) -> HashResult {
        (HASH_LIST, v).hash(self.0);
        HASH_END.hash(self.0);
        Ok(())
    }
    fn serialize_none(self) -> HashResult {
        HASH_NULL.hash(self.0);
        Ok(())
    }
    fn serialize_some<T: ?Sized + serde::Serialize>(self, value: &T) -> HashResult {
        value.serialize(self)
    }
    fn serialize_unit(self) -> HashResult {
        HASH_NULL.hash(self.0);
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> HashResult {
        HASH_NULL.hash(self.0);
        Ok(())
    }
    fn serialize_unit_variant(self, _name: &'static str, _index: u32, variant: &'static str) -> HashResult {
        (HASH_TEXT, variant).hash(self.0);
        Ok(())
    }
    fn serialize_newtype_struct<T: ?Sized + serde::Serialize>(self, _name: &'static str, value: &T) -> HashResult {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + serde::Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> HashResult {
        (HASH_FIELDS, variant).hash(self.0);
        value.serialize(&mut *self)?;
        HASH_END.hash(self.0);
        Ok(())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self, serde_json::Error> {
        HASH_LIST.hash(self.0);
        Ok(self)
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self, serde_json::Error> {
        HASH_LIST.hash(self.0);
        Ok(self)
    }
    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Self, serde_json::Error> {
        HASH_LIST.hash(self.0);
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self, serde_json::Error> {
        (HASH_FIELDS, variant, HASH_LIST).hash(self.0);
        Ok(self)
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self, serde_json::Error> {
        HASH_FIELDS.hash(self.0);
        Ok(self)
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self, serde_json::Error> {
        HASH_FIELDS.hash(self.0);
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self, serde_json::Error> {
        (HASH_FIELDS, variant, HASH_FIELDS).hash(self.0);
        Ok(self)
    }
}

impl<H: Hasher> serde::ser::SerializeSeq for &mut RoundedHash<'_, H> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_element<T: ?Sized + serde::Serialize>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }
    fn end(self) -> HashResult {
        HASH_END.hash(self.0);
        Ok(())
    }
}

impl<H: Hasher> serde::ser::SerializeTuple for &mut RoundedHash<'_, H> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_element<T: ?Sized + serde::Serialize>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }
    fn end(self) -> HashResult {
        HASH_END.hash(self.0);
        Ok(())
    }
}

impl<H: Hasher> serde::ser::SerializeTupleStruct for &mut RoundedHash<'_, H> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + serde::Serialize>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }
    fn end(self) -> HashResult {
        HASH_END.hash(self.0);
        Ok(())
    }
}

impl<H: Hasher> serde::ser::SerializeTupleVariant for &mut RoundedHash<'_, H> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + serde::Serialize>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }
    fn end(self) -> HashResult {
        (HASH_END, HASH_END).hash(self.0);
        Ok(())
    }
}

impl<H: Hasher> serde::ser::SerializeMap for &mut RoundedHash<'_, H> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_key<T: ?Sized + serde::Serialize>(&mut self, key: &T) -> HashResult {
        key.serialize(&mut **self)
    }
    fn serialize_value<T: ?Sized + serde::Serialize>(&mut self, value: &T) -> HashResult {
        value.serialize(&mut **self)
    }
    fn end(self) -> HashResult {
        HASH_END.hash(self.0);
        Ok(())
    }
}

impl<H: Hasher> serde::ser::SerializeStruct for &mut RoundedHash<'_, H> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + serde::Serialize>(&mut self, key: &'static str, value: &T) -> HashResult {
        key.hash(self.0);
        value.serialize(&mut **self)
    }
    fn end(self) -> HashResult {
        HASH_END.hash(self.0);
        Ok(())
    }
}

impl<H: Hasher> serde::ser::SerializeStructVariant for &mut RoundedHash<'_, H> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + serde::Serialize>(&mut self, key: &'static str, value: &T) -> HashResult {
        key.hash(self.0);
        value.serialize(&mut **self)
    }
    fn end(self) -> HashResult {
        (HASH_END, HASH_END).hash(self.0);
        Ok(())
    }
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
            tint: None,
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
                anchor_ms: None,
                targets: targets.iter().map(|t| (*t).to_owned()).collect(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn since_of(command: &DrawCommand) -> f32 {
        match command {
            DrawCommand::Sdf {
                material: Some(material),
                ..
            } => material.since,
            _ => panic!("not a material shape"),
        }
    }

    fn anchored_at(anchor_ms: Option<f64>) -> DrawCommand {
        let mut command = material_shape(&[], false);
        if let DrawCommand::Sdf {
            material: Some(material),
            ..
        } = &mut command
        {
            material.anchor_ms = anchor_ms;
            material.since = 9.0;
        }
        command
    }

    #[test]
    fn stamp_since_sets_the_seconds_from_each_anchor_and_leaves_unanchored_materials() {
        let mut commands = vec![
            anchored_at(Some(1000.0)),
            anchored_at(None),
            DrawCommand::AdditiveGroup {
                commands: vec![anchored_at(Some(4000.0))],
            },
        ];
        stamp_since(&mut commands, 5500.0);
        assert_eq!(since_of(&commands[0]), 4.5);
        assert_eq!(since_of(&commands[1]), 9.0, "control: no anchor, untouched");
        let DrawCommand::AdditiveGroup { commands: nested } = &commands[2] else {
            panic!("a group");
        };
        assert_eq!(since_of(&nested[0]), 1.5, "nested commands are reached");
        stamp_since(&mut commands, 500.0);
        assert_eq!(
            since_of(&commands[0]),
            -0.5,
            "edge: before its anchor the time is negative"
        );
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
    fn commands_hashed_from_their_origin_ignore_where_the_run_sits() {
        use crate::packet::{PathSegment, Vec2};
        let path = |x: f32, y: f32, red: f32| DrawCommand::Path {
            segments: vec![
                PathSegment::MoveTo { x, y },
                PathSegment::LineTo { x: x + 8.0, y },
                PathSegment::LineTo {
                    x: x + 8.0,
                    y: y + 8.0,
                },
                PathSegment::Close,
            ],
            fill: Some([red, 0.0, 0.0, 1.0]),
            stroke: None,
        };
        let key = |command: DrawCommand, x: f32, y: f32| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            hash_commands_from([&command], Vec2 { x, y }, &mut hasher).expect("serializable");
            hasher.finish()
        };
        assert_eq!(
            key(path(100.25, 10.3, 1.0), 100.0, 10.0),
            key(path(763.25, -411.7, 1.0), 763.0, -412.0),
            "the same run moved by whole units"
        );
        assert_ne!(
            key(path(100.25, 10.3, 1.0), 100.0, 10.0),
            key(path(100.75, 10.3, 1.0), 100.0, 10.0),
            "control: another offset inside its box"
        );
        assert_ne!(
            key(path(100.25, 10.3, 1.0), 100.0, 10.0),
            key(path(100.25, 10.3, 0.5), 100.0, 10.0),
            "control: another colour"
        );
    }

    #[test]
    fn commands_hashed_from_their_origin_round_numbers_and_tell_structure_apart() {
        use crate::packet::{PathSegment, Vec2};
        let line = |x: f32, segments: usize| DrawCommand::Path {
            segments: std::iter::once(PathSegment::MoveTo { x, y: 0.0 })
                .chain((0..segments).map(|i| PathSegment::LineTo {
                    x: x + 1.0 + i as f32,
                    y: 2.0,
                }))
                .collect(),
            fill: None,
            stroke: None,
        };
        let key = |commands: &[DrawCommand]| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            hash_commands_from(commands, Vec2::default(), &mut hasher).expect("serializable");
            hasher.finish()
        };
        assert_eq!(
            key(&[line(1.0, 2)]),
            key(&[line(1.00005, 2)]),
            "a difference below a 4096th of a unit is the same run"
        );
        assert_ne!(
            key(&[line(1.0, 2)]),
            key(&[line(1.001, 2)]),
            "control: a difference above it is another run"
        );
        assert_ne!(
            key(&[line(1.0, 2)]),
            key(&[line(1.0, 3)]),
            "one more segment is another run"
        );
        assert_ne!(
            key(&[line(1.0, 1), line(1.0, 1)]),
            key(&[line(1.0, 1)]),
            "one more command is another run"
        );
        assert_ne!(
            key(&[DrawCommand::Path {
                segments: vec![PathSegment::MoveTo { x: 1.0, y: 0.0 }],
                fill: Some([0.0, 0.0, 0.0, 1.0]),
                stroke: None,
            }]),
            key(&[DrawCommand::Path {
                segments: vec![PathSegment::MoveTo { x: 1.0, y: 0.0 }],
                fill: None,
                stroke: None,
            }]),
            "a fill that is given is not a fill that is absent"
        );
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
