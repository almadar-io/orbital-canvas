//! Plain text through the glyph atlas, merged into one instance buffer per page.

use super::*;

impl SharedDeviceRenderer {
    /// The atlas draws for a plain `Text`; `None` sends it to the CPU vector layer.
    pub(super) fn atlas_text(
        &mut self,
        command: &DrawCommand,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
    ) -> Option<Vec<Draw>> {
        let (device, queue, format) = (self.device.clone(), self.queue.clone(), self.format);
        let (tw, th) = (target.width(), target.height());
        let scale = (
            tw as f32 / packet.width.max(1.0),
            th as f32 / packet.height.max(1.0),
        );
        let pass = self
            .glyphs
            .get_or_insert_with(|| crate::glyph_atlas::GlyphPass::new(&device, format));
        let runs = pass.place_text(&device, &queue, command, scale, (tw, th))?;
        Some(
            runs.into_iter()
                .map(|(page, instances)| Draw::GlyphRun { page, instances })
                .collect(),
        )
    }

    /// Merge adjacent glyph runs of one page into one instance buffer each.
    pub(super) fn finalize_glyphs(&self, draws: Vec<Draw>) -> Vec<Draw> {
        let mut out: Vec<Draw> = Vec::with_capacity(draws.len());
        let mut pending: Option<(u64, Vec<crate::glyph_atlas::GlyphInstance>)> = None;
        let flush = |pending: Option<(u64, Vec<crate::glyph_atlas::GlyphInstance>)>,
                     out: &mut Vec<Draw>| {
            if let Some((page, instances)) = pending {
                let buffer =
                    crate::glyph_atlas::GlyphPass::instance_buffer(&self.device, &instances);
                self.keep(&buffer);
                out.push(Draw::Glyphs {
                    page,
                    buffer,
                    count: instances.len() as u32,
                });
            }
        };
        for draw in draws {
            match draw {
                Draw::GlyphRun {
                    page,
                    mut instances,
                } => match pending.take() {
                    Some((p, mut list)) if p == page => {
                        list.append(&mut instances);
                        pending = Some((p, list));
                    }
                    other => {
                        flush(other, &mut out);
                        pending = Some((page, instances));
                    }
                },
                other => {
                    flush(pending.take(), &mut out);
                    out.push(other);
                }
            }
        }
        flush(pending, &mut out);
        out
    }
}
