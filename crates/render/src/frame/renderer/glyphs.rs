//! Plain text through the glyph atlas, merged into one instance buffer per page.

use super::*;

/// A glyph instance buffer kept across frames, its size and the frame that last drew from it.
pub(super) struct GlyphBuffer {
    buffer: wgpu::Buffer,
    capacity: u64,
    used_in: u64,
}

/// The smallest glyph instance buffer made, in bytes.
const GLYPH_BUFFER_MIN: u64 = 1024;

/// Frames a glyph instance buffer is kept without being drawn from.
const GLYPH_BUFFER_FRAMES: u64 = 120;

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

    /// A buffer holding `bytes` for this frame's draws: one kept from an earlier frame when it is large enough and no
    /// draw of this frame uses it, else a new one. Creating a buffer per run per frame is what this avoids.
    fn glyph_instances(&self, bytes: &[u8]) -> wgpu::Buffer {
        let mut held = self
            .glyph_buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let need = bytes.len() as u64;
        let free = held
            .iter_mut()
            .filter(|b| b.used_in != self.frame_count && b.capacity >= need)
            .min_by_key(|b| b.capacity);
        let buffer = match free {
            Some(kept) => {
                kept.used_in = self.frame_count;
                kept.buffer.clone()
            }
            None => {
                let capacity = need.max(GLYPH_BUFFER_MIN).next_power_of_two();
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("glyph instances"),
                    size: capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.glyph_buffers_made
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                held.push(GlyphBuffer {
                    buffer: buffer.clone(),
                    capacity,
                    used_in: self.frame_count,
                });
                buffer
            }
        };
        self.queue.write_buffer(&buffer, 0, bytes);
        buffer
    }

    /// Destroy the glyph buffers no frame has used for [`GLYPH_BUFFER_FRAMES`] (the web backend frees nothing on drop).
    pub(super) fn prune_glyph_buffers(&mut self) {
        let now = self.frame_count;
        self.glyph_buffers
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|held| {
                let keep = now.saturating_sub(held.used_in) <= GLYPH_BUFFER_FRAMES;
                if !keep {
                    held.buffer.destroy();
                }
                keep
            });
    }

    /// Glyph instance buffers made so far (observability for tests).
    #[cfg(test)]
    pub(super) fn glyph_buffers_made(&self) -> u64 {
        self.glyph_buffers_made
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Merge adjacent glyph runs of one page into one instance buffer each.
    pub(super) fn finalize_glyphs(&self, draws: Vec<Draw>) -> Vec<Draw> {
        let mut out: Vec<Draw> = Vec::with_capacity(draws.len());
        let mut pending: Option<(u64, Vec<crate::glyph_atlas::GlyphInstance>)> = None;
        let flush = |pending: Option<(u64, Vec<crate::glyph_atlas::GlyphInstance>)>,
                     out: &mut Vec<Draw>| {
            if let Some((page, instances)) = pending {
                let buffer = self.glyph_instances(bytemuck::cast_slice(&instances));
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
