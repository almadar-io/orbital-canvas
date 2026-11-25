//! What assistive technology and text input reach: the semantic tree, typed text, editing keys and access actions.

use super::*;

/// Modifier keys held with a key press.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyMods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
}

/// What an assistive technology asks of a semantic node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessAction {
    Focus,
    Activate,
    Increment,
    Decrement,
}

impl AccessAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Activate => "activate",
            Self::Increment => "increment",
            Self::Decrement => "decrement",
        }
    }
}

impl SceneHost {
    /// A number that moves whenever [`Self::semantic_tree`] would answer differently (the nodes were replaced, or the
    /// page scrolled) and stays while it would not: a host reads the tree again only when this moves.
    pub fn semantic_revision(&self) -> u32 {
        let now = (self.semantic_installs, self.page_scroll.offset.to_bits());
        let mut seen = self
            .semantic_seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.0 != now {
            *seen = (now, seen.1.wrapping_add(1));
        }
        seen.1
    }

    /// [`Self::semantic_tree`] as a JSON array in reading order, each node's rect in CSS pixels.
    pub fn semantic_tree_json(&self) -> String {
        let revision = self.semantic_revision();
        let mut kept = self
            .semantic_json
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, json)) = kept.as_ref().filter(|(at, _)| *at == revision) {
            return json.clone();
        }
        #[cfg(test)]
        self.semantic_json_builds
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nodes: Vec<serde_json::Value> = self
            .semantic_tree()
            .iter()
            .map(|n| {
                let mut o = serde_json::Map::new();
                o.insert("id".into(), n.id.clone().into());
                o.insert("role".into(), n.role.clone().into());
                o.insert("name".into(), n.name.clone().into());
                if let Some(v) = &n.value {
                    o.insert("value".into(), v.clone().into());
                }
                if let Some(c) = n.checked {
                    o.insert("checked".into(), c.into());
                }
                o.insert("disabled".into(), n.disabled.into());
                o.insert("focused".into(), n.focused.into());
                if let Some(c) = n.caret {
                    o.insert("caret".into(), c.into());
                }
                if let Some(c) = n.selection_start {
                    o.insert("selectionStart".into(), c.into());
                }
                if let Some(c) = n.selection_end {
                    o.insert("selectionEnd".into(), c.into());
                }
                o.insert("multiline".into(), n.multiline.into());
                if let Some(l) = n.level {
                    o.insert("level".into(), l.into());
                }
                if let Some(h) = &n.href {
                    o.insert("href".into(), h.clone().into());
                }
                if let Some(e) = n.expanded {
                    o.insert("expanded".into(), e.into());
                }
                if let Some(w) = &n.owner {
                    o.insert("owner".into(), w.clone().into());
                }
                o.insert("order".into(), serde_json::json!(n.order));
                o.insert(
                    "rect".into(),
                    serde_json::json!({
                        "x": n.rect.x, "y": n.rect.y, "w": n.rect.w, "h": n.rect.h
                    }),
                );
                serde_json::Value::Object(o)
            })
            .collect();
        let json = serde_json::Value::Array(nodes).to_string();
        *kept = Some((revision, json.clone()));
        json
    }

    /// The semantic tree of the current scene, in reading order.
    pub fn semantic_tree(&self) -> Vec<mappers::scene::SemanticNode> {
        let offset = self.page_scroll.offset;
        self.semantic
            .iter()
            .map(|node| {
                let mut node = node.clone();
                if !node.fixed {
                    node.rect.y -= offset;
                }
                node
            })
            .collect()
    }

    /// Text for the focused text field (`composing`: the input method's preedit).
    pub fn text_input(&mut self, target: &str, text: &str, composing: bool) {
        self.queue_canvas_event(
            "textInputEvent",
            serde_json::json!({ "target": target, "text": text, "composing": composing }),
        );
    }

    /// An editing key for the focused text field.
    pub fn key_input(&mut self, target: &str, key: &str, mods: KeyMods) {
        self.queue_canvas_event(
            "keyEvent",
            serde_json::json!({
                "target": target, "key": key,
                "shift": mods.shift, "alt": mods.alt, "ctrl": mods.ctrl, "meta": mods.meta,
            }),
        );
    }

    /// An assistive technology acted on a node of the semantic tree.
    pub fn access_action(&mut self, target: &str, action: AccessAction) {
        self.queue_canvas_event(
            "accessEvent",
            serde_json::json!({ "target": target, "action": action.as_str() }),
        );
    }
}
