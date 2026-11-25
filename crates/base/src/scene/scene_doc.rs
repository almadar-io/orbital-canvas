//! A canvas scene with its top-level drawables held one by one: a scene sent again shares every drawable it did not
//! change, so the stages after it copy, theme, compare and scan only the drawables that are new.

use serde_json::Value;
use std::sync::Arc;

/// A canvas node and, when it was split, its top-level `drawables` list as one handle per drawable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneDoc {
    /// The node, without its top-level `drawables` when they are held in `drawables`.
    pub frame: Arc<Value>,
    /// The node's top-level drawables, in order; `None` when the node was kept whole.
    pub drawables: Option<Arc<Vec<Arc<Value>>>>,
}

impl From<Arc<Value>> for SceneDoc {
    fn from(node: Arc<Value>) -> Self {
        Self::whole(node)
    }
}

impl SceneDoc {
    /// `node` kept whole: its drawables stay inside it.
    pub fn whole(node: impl Into<Arc<Value>>) -> Self {
        Self { frame: node.into(), drawables: None }
    }

    /// `node` with its top-level `drawables` list (under `props` when it has them) moved out, one handle each.
    pub fn split(mut node: Value) -> Self {
        let root = if node.get("props").is_some_and(Value::is_object) { &mut node["props"] } else { &mut node };
        let listed = match root.as_object_mut().and_then(|fields| fields.remove("drawables")) {
            Some(Value::Array(items)) => items.into_iter().map(Arc::new).collect(),
            Some(other) => {
                root["drawables"] = other;
                return Self::whole(node);
            }
            None => return Self::whole(node),
        };
        Self { frame: Arc::new(node), drawables: Some(Arc::new(listed)) }
    }

    /// The fields the node's props live in: its `props` when it has them, else the node itself.
    pub fn root(&self) -> &Value {
        self.frame.get("props").unwrap_or(&self.frame)
    }

    /// The node this stands for, drawables back in place.
    pub fn node(&self) -> Value {
        let mut node = (*self.frame).clone();
        if let Some(listed) = &self.drawables {
            let root = if node.get("props").is_some_and(Value::is_object) { &mut node["props"] } else { &mut node };
            if let Some(fields) = root.as_object_mut() {
                fields.insert("drawables".into(), Value::Array(listed.iter().map(|d| (**d).clone()).collect()));
            }
        }
        node
    }

    /// Every drawable of the scene, groups included, in the order [`crate::spring`] visits a whole node's.
    pub fn for_each_drawable(&self, f: &mut dyn FnMut(&Value)) {
        match &self.drawables {
            Some(listed) => {
                for drawable in listed.iter() {
                    crate::spring::visit_tree(drawable, f);
                }
                if let Some(children) = self.root().get("children") {
                    crate::spring::visit_items(children, f);
                }
            }
            None => crate::spring::drawables(&self.frame, f),
        }
    }
}

/// What a scene's text was last read as, piece by piece: its frame and each top-level drawable, by their text.
#[derive(Debug, Default)]
pub struct SceneTextMemo {
    /// Drawables of the last scene read whose text was parsed, the rest having been handed back (observability).
    pub parsed: usize,
    /// The text before and after the drawables list, and the frame it parsed to.
    frame: Option<(String, String, Arc<Value>)>,
    /// Each drawable of the last scene by its text.
    drawables: std::collections::HashMap<Box<str>, Arc<Value>>,
}

impl SceneDoc {
    /// [`Self::split`] of the node `text` holds. A frame or a top-level drawable whose text is the one `memo` last
    /// read is handed back as the value it was read to, shared, so a scene sent again as text costs its changed
    /// drawables, not all of them.
    pub fn split_text(text: &str, memo: &mut SceneTextMemo) -> serde_json::Result<SceneDoc> {
        let Some((list, items)) = text_drawables(text) else {
            *memo = SceneTextMemo::default();
            let doc = Self::split(serde_json::from_str(text)?);
            memo.parsed = doc.drawables.as_ref().map_or(0, |listed| listed.len());
            return Ok(doc);
        };
        let (before, after) = (&text[..list.0], &text[list.1..]);
        let frame = match memo.frame.as_ref().filter(|(b, a, _)| b == before && a == after) {
            Some((_, _, frame)) => Arc::clone(frame),
            None => {
                let mut node: Value = serde_json::from_str(&[before, "[]", after].concat())?;
                let root = if node.get("props").is_some_and(Value::is_object) { &mut node["props"] } else { &mut node };
                if let Some(fields) = root.as_object_mut() {
                    fields.remove("drawables");
                }
                let frame = Arc::new(node);
                memo.frame = Some((before.to_owned(), after.to_owned(), Arc::clone(&frame)));
                frame
            }
        };
        let mut held = std::collections::HashMap::with_capacity(items.len());
        let mut listed = Vec::with_capacity(items.len());
        let mut parsed = 0;
        for (start, end) in items {
            let piece = &text[start..end];
            let drawable = match memo.drawables.remove_entry(piece) {
                Some((key, drawable)) => {
                    held.insert(key, Arc::clone(&drawable));
                    drawable
                }
                None => match held.get(piece) {
                    Some(drawable) => Arc::clone(drawable),
                    None => {
                        parsed += 1;
                        let drawable: Arc<Value> = Arc::new(serde_json::from_str(piece)?);
                        held.insert(piece.into(), Arc::clone(&drawable));
                        drawable
                    }
                },
            };
            listed.push(drawable);
        }
        memo.drawables = held;
        memo.parsed = parsed;
        Ok(Self { frame, drawables: Some(Arc::new(listed)) })
    }
}

/// Where a piece of text starts and ends.
type Span = (usize, usize);

/// An object member as written: its key and where its value sits.
type Member<'a> = (&'a str, Span);

/// Where the list [`SceneDoc::split`] moves out sits in a node's text, and where each of its items does; `None`
/// when the text is not plainly an object with one such list (the caller then parses it whole).
fn text_drawables(text: &str) -> Option<(Span, Vec<Span>)> {
    let bytes = text.as_bytes();
    let (root, end) = text_members(text, skip_space(bytes, 0))?;
    if skip_space(bytes, end) != bytes.len() {
        return None;
    }
    let only = |members: &[Member], key: &str| {
        let mut found = members.iter().filter(|(k, _)| *k == key);
        match (found.next(), found.next()) {
            (first, None) => Some(first.map(|(_, span)| *span)),
            _ => None,
        }
    };
    let props = only(&root, "props")?.filter(|span| bytes[span.0] == b'{');
    let list = match props {
        Some(span) => only(&text_members(text, span.0)?.0, "drawables")?,
        None => only(&root, "drawables")?,
    }?;
    if bytes[list.0] != b'[' {
        return None;
    }
    let mut items = Vec::new();
    let mut at = skip_space(bytes, list.0 + 1);
    if bytes.get(at) != Some(&b']') {
        loop {
            let end = skip_value(bytes, at)?;
            items.push((at, end));
            at = skip_space(bytes, end);
            match bytes.get(at)? {
                b',' => at = skip_space(bytes, at + 1),
                b']' => break,
                _ => return None,
            }
        }
    }
    Some((list, items))
}

/// The members of the object that opens at `at`, each key (as written, never one with an escape) with the span of
/// its value, and where the object ends.
fn text_members(text: &str, at: usize) -> Option<(Vec<Member<'_>>, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(at) != Some(&b'{') {
        return None;
    }
    let mut members = Vec::new();
    let mut at = skip_space(bytes, at + 1);
    if bytes.get(at) == Some(&b'}') {
        return Some((members, at + 1));
    }
    loop {
        let key_end = skip_string(bytes, at)?;
        let key = text.get(at + 1..key_end - 1)?;
        if key.contains('\\') {
            return None;
        }
        at = skip_space(bytes, key_end);
        if bytes.get(at) != Some(&b':') {
            return None;
        }
        at = skip_space(bytes, at + 1);
        let end = skip_value(bytes, at)?;
        members.push((key, (at, end)));
        at = skip_space(bytes, end);
        match bytes.get(at)? {
            b',' => at = skip_space(bytes, at + 1),
            b'}' => return Some((members, at + 1)),
            _ => return None,
        }
    }
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\n' | b'\r' | b'\t')) {
        at += 1;
    }
    at
}

/// Past the string that opens at `at`.
fn skip_string(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    let mut at = at + 1;
    loop {
        match bytes.get(at)? {
            b'"' => return Some(at + 1),
            b'\\' => at += 2,
            _ => at += 1,
        }
    }
}

/// Past the value that starts at `at`: a string, an object or a list with everything inside it, or a bare word.
fn skip_value(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at)? {
        b'"' => skip_string(bytes, at),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut at = at;
            loop {
                match bytes.get(at)? {
                    b'"' => {
                        at = skip_string(bytes, at)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(at + 1);
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
        }
        b',' | b'}' | b']' | b':' => None,
        _ => {
            let mut end = at;
            while !matches!(bytes.get(end), None | Some(b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t')) {
                end += 1;
            }
            Some(end)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SceneDoc, SceneTextMemo};
    use serde_json::json;

    fn scene_text(label: &str, extra: &str) -> String {
        format!(
            r#"{{"type":"canvas","width":3,"drawables":[{{"id":"a","text":"br[ack]et {{ \" , ]"}},{{"id":"b","text":"{label}","items":[[1,2],[3,{{"k":"]"}}]]}}{extra}],"children":[{{"id":"c"}}]}}"#
        )
    }

    #[test]
    fn a_scene_read_from_text_is_the_scene_split_from_its_parsed_node() {
        let texts = [
            scene_text("one", ""),
            scene_text("one", r#",{"id":"d"}"#),
            r#"{"type":"canvas","props":{"width":3,"drawables":[{"id":"a"},  {"id":"b"} ]},"drawables":"kept"}"#.to_owned(),
            " {\n \"type\" : \"canvas\" ,\n \"drawables\" : [ ] , \"z\" : [1, 2] }\n".to_owned(),
            r#"{"type":"canvas","drawables":[1,"two",null,[3],{"id":"e"}]}"#.to_owned(),
            r#"{"type":"canvas","children":[{"id":"c"}]}"#.to_owned(),
            r#"{"type":"canvas","drawables":{"not":"a list"}}"#.to_owned(),
            r#"{"type":"canvas","drawables":[{"id":"a"}],"drawables":[{"id":"z"}]}"#.to_owned(),
            r#"{"type":"canvas","props":"flat","drawables":[{"id":"a"}]}"#.to_owned(),
            r#"["not","an","object"]"#.to_owned(),
        ];
        let mut memo = SceneTextMemo::default();
        for text in &texts {
            let node: serde_json::Value = serde_json::from_str(text).unwrap();
            let expected = SceneDoc::split(node);
            assert_eq!(SceneDoc::split_text(text, &mut memo).unwrap(), expected, "{text}");
            assert_eq!(SceneDoc::split_text(text, &mut memo).unwrap(), expected, "read again from the memo: {text}");
        }
        assert!(SceneDoc::split_text(r#"{"type":"canvas","drawables":[{"id":"a"},]}"#, &mut memo).is_err(), "edge: malformed text is refused");
        assert!(SceneDoc::split_text("", &mut memo).is_err(), "edge: empty text is refused");
    }

    #[test]
    fn a_scene_read_again_shares_every_piece_whose_text_did_not_change() {
        let mut memo = SceneTextMemo::default();
        let first = SceneDoc::split_text(&scene_text("one", ""), &mut memo).unwrap();
        assert_eq!(memo.parsed, 2, "control: a first read parses every drawable");
        let again = SceneDoc::split_text(&scene_text("two", ""), &mut memo).unwrap();
        let (before, after) = (first.drawables.as_ref().unwrap(), again.drawables.as_ref().unwrap());
        assert_eq!(memo.parsed, 1, "only the drawable whose text changed is parsed");
        assert!(std::sync::Arc::ptr_eq(&before[0], &after[0]), "the unchanged drawable is the one already held");
        assert!(!std::sync::Arc::ptr_eq(&before[1], &after[1]), "control: the changed one is new");
        assert_eq!(after[1]["text"], json!("two"));
        assert!(std::sync::Arc::ptr_eq(&first.frame, &again.frame), "the frame around them did not change either");
        let grown = SceneDoc::split_text(&scene_text("two", r#",{"id":"d"}"#), &mut memo).unwrap();
        let later = grown.drawables.as_ref().unwrap();
        assert_eq!(memo.parsed, 1, "an added drawable is the only one parsed");
        assert!(std::sync::Arc::ptr_eq(&after[1], &later[1]));
        let moved = SceneDoc::split_text(
            r#"{"type":"canvas","width":3,"drawables":[{"id":"d"},{"id":"a","text":"br[ack]et { \" , ]"}],"children":[{"id":"c"}]}"#,
            &mut memo,
        )
        .unwrap();
        assert_eq!(memo.parsed, 0, "a drawable at another place in the list is still the one held");
        assert!(std::sync::Arc::ptr_eq(&later[2], &moved.drawables.as_ref().unwrap()[0]));
        assert!(std::sync::Arc::ptr_eq(&later[0], &moved.drawables.as_ref().unwrap()[1]));
        let resized = SceneDoc::split_text(
            r#"{"type":"canvas","width":9,"drawables":[{"id":"d"}],"children":[{"id":"c"}]}"#,
            &mut memo,
        )
        .unwrap();
        assert!(!std::sync::Arc::ptr_eq(&moved.frame, &resized.frame), "control: a changed frame is read again");
        assert_eq!(resized.frame["width"], json!(9));
    }

    #[test]
    fn a_split_scene_is_the_same_scene_and_visits_its_drawables_in_the_same_order() {
        for node in [
            json!({"type": "canvas", "drawables": [{"id": "a", "children": [{"id": "a1"}]}, {"id": "b"}], "children": [{"id": "c"}]}),
            json!({"type": "canvas", "props": {"drawables": [{"id": "a"}], "width": 3}}),
        ] {
            let doc = SceneDoc::split(node.clone());
            assert_eq!(doc.node(), node, "splitting moves the drawables out and back without changing the scene");
            let (mut whole, mut split) = (Vec::new(), Vec::new());
            crate::spring::drawables(&node, &mut |d| whole.push(d.clone()));
            doc.for_each_drawable(&mut |d| split.push(d.clone()));
            assert_eq!(split, whole);
        }
        let plain = json!({"type": "canvas", "children": [{"id": "c"}]});
        let doc = SceneDoc::split(plain.clone());
        assert!(doc.drawables.is_none(), "control: a node with no drawables list is kept whole");
        assert_eq!(doc.node(), plain);
    }

    #[test]
    fn springs_on_a_split_scene_show_what_they_show_on_the_whole_one_and_copy_only_what_they_drive() {
        let scene = |w: f64| {
            json!({"type": "canvas", "drawables": [
                {"id": "card", "type": "draw-shape", "width": w, "spring": {"stiffness": 170, "damping": 26}},
                {"id": "logo", "type": "draw-text", "text": "hi"}
            ], "children": [{"id": "kid", "type": "draw-shape", "width": w, "spring": {"stiffness": 170, "damping": 26}}]})
        };
        let mut whole = crate::spring::SpringBank::new();
        let mut split = crate::spring::SpringBank::new();
        whole.retarget(&scene(10.0));
        split.retarget_doc(&SceneDoc::split(scene(10.0)));
        whole.retarget(&scene(50.0));
        let doc = SceneDoc::split(scene(50.0));
        split.retarget_doc(&doc);
        whole.step(0.05);
        split.step(0.05);
        let shown = split.apply_doc(&doc);
        assert_eq!(shown.node(), whole.apply(&scene(50.0)), "the same displayed values");
        let (before, after) = (doc.drawables.as_ref().unwrap(), shown.drawables.as_ref().unwrap());
        assert!(std::sync::Arc::ptr_eq(&before[1], &after[1]), "a drawable no spring drives is shared");
        assert!(!std::sync::Arc::ptr_eq(&before[0], &after[0]), "control: the driven one is a copy");
        assert!(!std::sync::Arc::ptr_eq(&doc.frame, &shown.frame), "a driven child of the frame copies the frame");
    }

    #[test]
    fn springs_read_from_kept_parts_match_springs_read_afresh() {
        let scene = |w: f64, text: &str| {
            json!({"type": "canvas", "drawables": [
                {"id": "card", "type": "draw-shape", "width": w, "spring": {"stiffness": 170, "damping": 26}},
                {"id": "logo", "type": "draw-text", "text": text},
                {"id": "card", "type": "draw-shape", "width": 1}
            ]})
        };
        let mut fresh = crate::spring::SpringBank::new();
        let mut kept = crate::spring::SpringBank::new();
        let mut parts = crate::spring::SpringParts::default();
        let first = SceneDoc::split(scene(10.0, "a"));
        fresh.retarget_doc(&first);
        kept.retarget_doc_with(&first, &mut parts);
        let mut next = SceneDoc::split(scene(50.0, "b"));
        std::sync::Arc::make_mut(next.drawables.as_mut().unwrap())[2] = std::sync::Arc::clone(&first.drawables.as_ref().unwrap()[2]);
        fresh.retarget_doc(&next);
        kept.retarget_doc_with(&next, &mut parts);
        fresh.step(0.05);
        kept.step(0.05);
        assert_eq!(kept.apply_doc_with(&next, &parts).node(), fresh.apply_doc(&next).node(), "the same displayed values");
        let shown = kept.apply_doc_with(&next, &parts);
        assert!(
            !std::sync::Arc::ptr_eq(&shown.drawables.as_ref().unwrap()[2], &next.drawables.as_ref().unwrap()[2]),
            "a kept springless drawable that shares a driven id is still written, as a fresh read writes it"
        );
        assert!(std::sync::Arc::ptr_eq(&shown.drawables.as_ref().unwrap()[1], &next.drawables.as_ref().unwrap()[1]), "control: one with no driven id is shared");
    }
}
