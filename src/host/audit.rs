//! The painted-contrast audit: every text against the pixels actually drawn behind it.

use super::*;
use serde::Serialize;

/// One text's measured contrast. `ratio` is the worst over the sampled backdrop pixels, `None` when nothing is
/// painted behind the text (the page behind the canvas is not known here).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContrastFinding {
    /// Position of the text among the packet's texts, in draw order.
    pub index: usize,
    pub text: String,
    pub size: f32,
    pub weight: u16,
    pub ratio: Option<f32>,
    pub required: f32,
    pub rect: packet::Rect,
}

impl ContrastFinding {
    pub fn fails(&self) -> bool {
        self.ratio.is_some_and(|r| r < self.required)
    }
}

struct TextProbe {
    index: usize,
    text: String,
    size: f32,
    weight: u16,
    colors: Vec<[f32; 4]>,
    halo: Option<[f32; 4]>,
    rect: packet::Rect,
}

/// A packet's texts and the same packet drawn without any of them.
pub struct ContrastProbe {
    packet: GameRenderPacket,
    texts: Vec<TextProbe>,
}

fn required_ratio(size: f32, weight: u16) -> f32 {
    if size >= 24.0 || (size >= 18.66 && weight >= 600) {
        3.0
    } else {
        4.5
    }
}

fn bare(commands: &[DrawCommand], texts: &mut Vec<TextProbe>, audited: bool) -> Vec<DrawCommand> {
    let mut kept = Vec::with_capacity(commands.len());
    for command in commands {
        match command {
            DrawCommand::Text { .. } => {
                if audited {
                    probe_text(command, texts);
                }
            }
            DrawCommand::Blurred { command, .. }
                if matches!(**command, DrawCommand::Text { .. }) =>
            {
                if audited {
                    probe_text(command, texts);
                }
            }
            DrawCommand::ClipGroup { clip, commands } => kept.push(DrawCommand::ClipGroup {
                clip: clip.clone(),
                commands: bare(commands, texts, audited),
            }),
            DrawCommand::AdditiveGroup { commands } => kept.push(DrawCommand::AdditiveGroup {
                commands: bare(commands, texts, audited),
            }),
            DrawCommand::Faded { opacity, commands } => kept.push(DrawCommand::Faded {
                opacity: *opacity,
                commands: bare(commands, texts, audited),
            }),
            DrawCommand::Shadow {
                commands,
                color,
                sigma,
            } => kept.push(DrawCommand::Shadow {
                commands: bare(commands, texts, audited),
                color: *color,
                sigma: *sigma,
            }),
            DrawCommand::Target {
                id,
                width,
                height,
                live,
                commands,
            } => kept.push(DrawCommand::Target {
                id: id.clone(),
                width: *width,
                height: *height,
                live: *live,
                commands: bare(commands, texts, false),
            }),
            other => kept.push(other.clone()),
        }
    }
    kept
}

fn probe_text(command: &DrawCommand, texts: &mut Vec<TextProbe>) {
    let DrawCommand::Text {
        text,
        size,
        color,
        weight,
        halo,
        color_runs,
        ..
    } = command
    else {
        return;
    };
    if color[3] <= 0.0 && color_runs.iter().all(|run| run.color[3] <= 0.0) {
        return;
    }
    let mut unhaloed = command.clone();
    if let DrawCommand::Text { halo, .. } = &mut unhaloed {
        *halo = None;
    }
    let crate::vector::RunBounds::Box { x0, y0, x1, y1 } = crate::vector::command_bounds(&unhaloed)
    else {
        return;
    };
    let mut colors = Vec::new();
    let covered: usize = color_runs
        .iter()
        .map(|r| r.to.min(text.chars().count()).saturating_sub(r.from))
        .sum();
    if covered < text.chars().count() {
        colors.push(*color);
    }
    for run in color_runs {
        if !colors.contains(&run.color) {
            colors.push(run.color);
        }
    }
    texts.push(TextProbe {
        index: texts.len(),
        text: text.clone(),
        size: *size,
        weight: *weight,
        colors,
        halo: halo.map(|h| h.color),
        rect: packet::Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        },
    });
}

impl ContrastProbe {
    /// The packet to draw: the frame without any text.
    pub fn packet(&self) -> &GameRenderPacket {
        &self.packet
    }

    /// Every text measured against `rgba` (`width` x `height` readback of [`Self::packet`], top row first).
    pub fn measure(&self, rgba: &[u8], width: u32, height: u32) -> Vec<ContrastFinding> {
        let scale = width as f32 / self.packet.width.max(1.0);
        self.texts
            .iter()
            .map(|t| ContrastFinding {
                index: t.index,
                text: t.text.clone(),
                size: t.size,
                weight: t.weight,
                ratio: worst_ratio(t, rgba, width, height, scale),
                required: required_ratio(t.size, t.weight),
                rect: t.rect,
            })
            .collect()
    }
}

fn worst_ratio(t: &TextProbe, rgba: &[u8], width: u32, height: u32, scale: f32) -> Option<f32> {
    let x0 = (t.rect.x * scale).floor().max(0.0) as u32;
    let y0 = (t.rect.y * scale).floor().max(0.0) as u32;
    let x1 = (((t.rect.x + t.rect.w) * scale).ceil().max(0.0) as u32).min(width);
    let y1 = (((t.rect.y + t.rect.h) * scale).ceil().max(0.0) as u32).min(height);
    let mut worst: Option<f32> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            let at = ((y * width + x) * 4) as usize;
            let Some(px) = rgba.get(at..at + 4) else {
                continue;
            };
            if px[3] != 255 {
                continue;
            }
            let mut bg = [
                f32::from(px[0]) / 255.0,
                f32::from(px[1]) / 255.0,
                f32::from(px[2]) / 255.0,
                1.0,
            ];
            if let Some(halo) = t.halo {
                let a = halo[3].clamp(0.0, 1.0);
                for c in 0..3 {
                    bg[c] = halo[c] * a + bg[c] * (1.0 - a);
                }
            }
            for color in &t.colors {
                let ratio = crate::contrast::wcag_ratio(*color, bg);
                worst = Some(worst.map_or(ratio, |w| w.min(ratio)));
            }
        }
    }
    worst
}

impl SceneHost {
    /// The frame as currently drawn, split into its texts and a text-free packet; `None` without a scene.
    pub fn contrast_probe(&self) -> Option<ContrastProbe> {
        let mut packet = self.frame_packet()?;
        let mut texts = Vec::new();
        packet.draw_commands = bare(&packet.draw_commands, &mut texts, true);
        Some(ContrastProbe { packet, texts })
    }

    /// The size of the readback of a probe: the host surface in physical pixels, else the packet's own size.
    pub fn contrast_readback_size(&self, probe: &ContrastProbe) -> (u32, u32) {
        self.viewport.unwrap_or((
            probe.packet.width.max(1.0) as u32,
            probe.packet.height.max(1.0) as u32,
        ))
    }

    /// Every text of the scene with its measured ratio, drawn on a headless device. An error means the audit
    /// could not run (no adapter), never that nothing failed.
    pub fn contrast_audit_all(&mut self) -> crate::Result<Vec<ContrastFinding>> {
        let Some(probe) = self.contrast_probe() else {
            return Ok(Vec::new());
        };
        if probe.texts.is_empty() {
            return Ok(Vec::new());
        }
        let (width, height) = self.contrast_readback_size(&probe);
        let mut canvas = Canvas::new_headless()?;
        canvas.viewport = Some((width, height));
        let rgba = canvas.render_packet_to_rgba(&probe.packet)?;
        Ok(probe.measure(&rgba, width, height))
    }

    /// The texts whose painted contrast is below what WCAG 2.2 AA requires.
    pub fn contrast_audit(&mut self) -> crate::Result<Vec<ContrastFinding>> {
        Ok(self
            .contrast_audit_all()?
            .into_iter()
            .filter(ContrastFinding::fails)
            .collect())
    }
}
