//! Runs of vector commands rasterized on the CPU and drawn as one layer texture, cached by what they draw.

use super::*;

/// Cache-key prefix of vector layer textures (never a fetchable URL).
pub(super) const VECTOR_LAYER_PREFIX: &str = "vector-layer:";

/// Vector layer textures kept across frames; a packet re-submitted unchanged
/// reuses its layer, older layers are evicted least recently used first.
pub(super) const MAX_VECTOR_LAYERS: usize = 8;

impl SharedDeviceRenderer {
    /// Rasterize a run of consecutive vector commands (paths, lines, text) into
    /// a texture the size of the run's box ([`orbital_canvas_paint::vector::run_bounds`]) and
    /// queue it as a sprite over that box; a run of unknown reach covers the
    /// whole target. The layer is cached by the run's content, box and scale.
    pub(super) fn flush_vector_run(
        &mut self,
        run: &mut Vec<&DrawCommand>,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
        draws: &mut Vec<Draw>,
    ) {
        self.flush_vector_run_with_blend(run, packet, target, draws, false);
    }

    pub(super) fn flush_vector_run_with_blend(
        &mut self,
        run: &mut Vec<&DrawCommand>,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
        draws: &mut Vec<Draw>,
        additive: bool,
    ) {
        if run.is_empty() {
            return;
        }
        let commands = std::mem::take(run);
        let (tw, th) = (target.width(), target.height());
        if tw > self.device.limits().max_texture_dimension_2d
            || th > self.device.limits().max_texture_dimension_2d
        {
            tracing::warn!(target: "almadar::game_renderer", tw, th, "vector layer exceeds the device texture limit — skipped");
            return;
        }
        let width = packet.width.max(1.0);
        let height = packet.height.max(1.0);
        let sx = tw as f32 / width;
        let sy = th as f32 / height;
        let (px0, py0, px1, py1) = match orbital_canvas_paint::vector::run_bounds(&commands) {
            orbital_canvas_paint::vector::RunBounds::Empty => return,
            orbital_canvas_paint::vector::RunBounds::Anywhere => (0, 0, tw, th),
            orbital_canvas_paint::vector::RunBounds::Box { x0, y0, x1, y1 } => (
                (x0 * sx).floor().clamp(0.0, tw as f32) as u32,
                (y0 * sy).floor().clamp(0.0, th as f32) as u32,
                (x1 * sx).ceil().clamp(0.0, tw as f32) as u32,
                (y1 * sy).ceil().clamp(0.0, th as f32) as u32,
            ),
        };
        if px1 <= px0 || py1 <= py0 {
            return;
        }
        let (lw, lh) = (px1 - px0, py1 - py0);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(px0, py0, sx.to_bits(), sy.to_bits()), &mut hasher);
        if commands
            .iter()
            .any(|command| orbital_canvas_paint::vector::uses_text(command))
        {
            std::hash::Hash::hash(
                &orbital_canvas_paint::vector::font_generation(),
                &mut hasher,
            );
        }
        if commands
            .iter()
            .any(|command| orbital_canvas_paint::vector::uses_image_pattern(command))
        {
            std::hash::Hash::hash(&orbital_canvas_paint::asset::generation(), &mut hasher);
        }
        if let Err(err) =
            orbital_canvas_base::targets::hash_commands(commands.iter().copied(), &mut hasher)
        {
            tracing::warn!(target: "almadar::game_renderer", error = %err, "vector command not serializable — skipped");
            return;
        }
        let key = SpriteKey {
            url: format!(
                "{VECTOR_LAYER_PREFIX}{:016x}",
                std::hash::Hasher::finish(&hasher)
            ),
            raster: Some((lw, lh)),
        };
        self.use_clock += 1;
        let now = self.use_clock;
        if let Some(sprite) = self.sprites.get_mut(&key) {
            sprite.last_used = now;
        } else {
            let (ox, oy) = (px0 as f32 / sx, py0 as f32 / sy);
            let Some(rgba) =
                orbital_canvas_paint::vector::rasterize_box(&commands, lw, lh, sx, sy, ox, oy)
            else {
                tracing::warn!(target: "almadar::game_renderer", lw, lh, "vector layer surface not allocatable — skipped");
                return;
            };
            self.evict_stale_vector_layers();
            let sprite = self.upload_rgba(&rgba, lw, lh, now);
            self.sprites.insert(key.clone(), sprite);
        }
        let verts = sprite_vertices(
            &Rect {
                x: 0.0,
                y: 0.0,
                w: lw as f32,
                h: lh as f32,
            },
            &Rect {
                x: px0 as f32 / sx,
                y: py0 as f32 / sy,
                w: lw as f32 / sx,
                h: lh as f32 / sy,
            },
            (lw, lh),
            0.0,
            1.0,
            (false, false),
            (width as u32, height as u32),
        );
        draws.push(Draw::Sprite {
            key,
            verts: verts.to_vec(),
            additive,
        });
    }

    pub(super) fn evict_stale_vector_layers(&mut self) {
        loop {
            let held = self
                .sprites
                .keys()
                .filter(|k| k.url.starts_with(VECTOR_LAYER_PREFIX))
                .count();
            if held < MAX_VECTOR_LAYERS {
                return;
            }
            let victim = self
                .sprites
                .iter()
                .filter(|(k, s)| {
                    k.url.starts_with(VECTOR_LAYER_PREFIX) && s.last_used <= self.frame_epoch
                })
                .min_by_key(|(_, s)| s.last_used)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    self.sprites.remove(&k);
                }
                None => return,
            }
        }
    }

    /// The pixel sizes of the cached vector layers (cache observability for tests).
    #[cfg(test)]
    pub(super) fn vector_layer_sizes(&self) -> Vec<(u32, u32)> {
        self.sprites
            .keys()
            .filter(|k| k.url.starts_with(VECTOR_LAYER_PREFIX))
            .filter_map(|k| k.raster)
            .collect()
    }

    /// Vector layer textures currently cached (cache observability for tests).
    #[cfg(test)]
    pub(super) fn vector_layer_count(&self) -> usize {
        self.sprites
            .keys()
            .filter(|k| k.url.starts_with(VECTOR_LAYER_PREFIX))
            .count()
    }
}
