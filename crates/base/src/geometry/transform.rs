//! The scene-to-viewport transform mappers draw through.

/// Scene-to-packet transform: scene units are scaled by `s` and offset by
/// (`ox`, `oy`) into the host viewport (contain-fit, centered).
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Xf {
    pub s: f32,
    pub ox: f32,
    pub oy: f32,
}

impl Xf {
    pub fn identity() -> Self {
        Self {
            s: 1.0,
            ox: 0.0,
            oy: 0.0,
        }
    }

    pub fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.ox + x * self.s, self.oy + y * self.s)
    }
}
