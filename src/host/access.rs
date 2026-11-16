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
