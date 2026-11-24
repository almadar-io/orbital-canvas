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

#[cfg(test)]
mod tests {
    use super::SceneDoc;
    use serde_json::json;

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
