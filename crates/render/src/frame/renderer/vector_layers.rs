//! Runs of vector commands rasterized on the CPU and drawn as one layer texture, cached by what they draw.

use super::*;

/// Cache-key prefix of vector layer textures (never a fetchable URL).
pub(super) const VECTOR_LAYER_PREFIX: &str = "vector-layer:";

/// Vector layer textures kept across frames, in bytes, as a multiple of the target's own size; a run re-submitted
/// unchanged or moved by whole pixels reuses its layer, older layers are evicted least recently used first.
pub(super) const VECTOR_LAYER_TARGETS: usize = 3;

/// A run's whole box is rasterized (not only its visible part) while it is at most this many targets in area, so a
/// run scrolling across the edge keeps one layer.
const WHOLE_RUN_TARGETS: u64 = 2;

/// A run's sub-pixel phase snaps to this many steps per device pixel, so a run drifting by fractions of a pixel
/// reuses one of a few cached layers instead of re-rasterizing every frame.
const PHASE_STEPS: f32 = 4.0;

impl SharedDeviceRenderer {
    /// Rasterize a run of consecutive vector commands (paths, lines, text) into
    /// a texture the size of the run's box ([`orbital_canvas_paint::vector::run_bounds`]) and
    /// queue it as a sprite over that box; a run of unknown reach covers the
    /// whole target. The layer is cached by the run's content relative to its
    /// box, the box's size and the scale.
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
        self.flush_vector_run_faded(run, packet, target, draws, additive, 1.0);
    }

    /// [`Self::flush_vector_run`] drawn at `opacity`; the opacity is not part of the layer's cache key.
    pub(super) fn flush_vector_run_faded(
        &mut self,
        run: &mut Vec<&DrawCommand>,
        packet: &GameRenderPacket,
        target: &wgpu::Texture,
        draws: &mut Vec<Draw>,
        additive: bool,
        opacity: f32,
    ) {
        if run.is_empty() {
            return;
        }
        let commands = std::mem::take(run);
        if commands.len() > 1
            && commands
                .iter()
                .any(|command| orbital_canvas_paint::vector::is_blur(command))
        {
            let mut segment: Vec<&DrawCommand> = Vec::new();
            for command in commands {
                if orbital_canvas_paint::vector::is_blur(command) {
                    self.flush_vector_run_faded(
                        &mut segment,
                        packet,
                        target,
                        draws,
                        additive,
                        opacity,
                    );
                    let mut alone = vec![command];
                    self.flush_vector_run_faded(
                        &mut alone, packet, target, draws, additive, opacity,
                    );
                } else {
                    segment.push(command);
                }
            }
            self.flush_vector_run_faded(&mut segment, packet, target, draws, additive, opacity);
            return;
        }
        let (tw, th) = (target.width(), target.height());
        let max_dim = self.device.limits().max_texture_dimension_2d;
        if tw > max_dim || th > max_dim {
            tracing::warn!(target: "almadar::game_renderer", tw, th, "vector layer exceeds the device texture limit — skipped");
            return;
        }
        let width = packet.width.max(1.0);
        let height = packet.height.max(1.0);
        let sx = tw as f32 / width;
        let sy = th as f32 / height;
        let mut shift = (0.0_f32, 0.0_f32);
        let (px0, py0, px1, py1): (i64, i64, i64, i64) =
            match orbital_canvas_paint::vector::run_bounds(&commands) {
                orbital_canvas_paint::vector::RunBounds::Empty => return,
                orbital_canvas_paint::vector::RunBounds::Anywhere => (0, 0, tw as i64, th as i64),
                orbital_canvas_paint::vector::RunBounds::Box { x0, y0, x1, y1 } => {
                    let snap = |v: f32| (v * PHASE_STEPS).round() / PHASE_STEPS - v;
                    shift = (snap(x0 * sx), snap(y0 * sy));
                    let whole = (
                        (x0 * sx + shift.0).floor() as i64,
                        (y0 * sy + shift.1).floor() as i64,
                        (x1 * sx + shift.0).ceil() as i64,
                        (y1 * sy + shift.1).ceil() as i64,
                    );
                    let (ww, wh) = (whole.2 - whole.0, whole.3 - whole.1);
                    let fits = ww <= max_dim as i64
                        && wh <= max_dim as i64
                        && (ww.max(0) as u64) * (wh.max(0) as u64)
                            <= WHOLE_RUN_TARGETS * tw as u64 * th as u64;
                    let visible =
                        whole.2 > 0 && whole.3 > 0 && whole.0 < tw as i64 && whole.1 < th as i64;
                    if !visible {
                        return;
                    }
                    if fits {
                        whole
                    } else {
                        (
                            whole.0.clamp(0, tw as i64),
                            whole.1.clamp(0, th as i64),
                            whole.2.clamp(0, tw as i64),
                            whole.3.clamp(0, th as i64),
                        )
                    }
                }
            };
        if px1 <= px0 || py1 <= py0 {
            return;
        }
        let (lw, lh) = ((px1 - px0) as u32, (py1 - py0) as u32);
        let (ox, oy) = (px0 as f32 / sx, py0 as f32 / sy);
        let (rx, ry) = (ox - shift.0 / sx, oy - shift.1 / sy);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(sx.to_bits(), sy.to_bits()), &mut hasher);
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
        if let Err(err) = orbital_canvas_base::targets::hash_commands_from(
            commands.iter().copied(),
            Vec2 { x: rx, y: ry },
            &mut hasher,
        ) {
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
            let sprite = match commands.as_slice() {
                [command] if orbital_canvas_paint::vector::is_blur(command) => {
                    self.blurred_layer(command, lw, lh, (sx, sy), (rx, ry), now)
                }
                _ => orbital_canvas_paint::vector::rasterize_box(&commands, lw, lh, sx, sy, rx, ry)
                    .map(|rgba| self.upload_rgba(&rgba, lw, lh, now)),
            };
            let Some(sprite) = sprite else {
                tracing::warn!(target: "almadar::game_renderer", lw, lh, "vector layer surface not allocatable — skipped");
                return;
            };
            self.sprites.insert(key.clone(), sprite);
            self.evict_stale_vector_layers(VECTOR_LAYER_TARGETS * tw as usize * th as usize * 4);
        }
        let verts = sprite_vertices(
            &Rect {
                x: 0.0,
                y: 0.0,
                w: lw as f32,
                h: lh as f32,
            },
            &Rect {
                x: ox,
                y: oy,
                w: lw as f32 / sx,
                h: lh as f32 / sy,
            },
            (lw, lh),
            0.0,
            opacity,
            (false, false),
            None,
            (width as u32, height as u32),
        );
        draws.push(Draw::Sprite {
            key,
            verts: verts.to_vec(),
            additive,
            tinted: false,
        });
    }

    /// Evict vector layers the current frame has not drawn, least recently used first, until the layers held fit in
    /// `budget` bytes; their textures are destroyed (the web backend frees nothing on drop).
    pub(super) fn evict_stale_vector_layers(&mut self, budget: usize) {
        let layer_bytes = |k: &SpriteKey| {
            k.raster
                .map(|(w, h)| w as usize * h as usize * 4)
                .unwrap_or(0)
        };
        let mut held: usize = self
            .sprites
            .keys()
            .filter(|k| k.url.starts_with(VECTOR_LAYER_PREFIX))
            .map(layer_bytes)
            .sum();
        while held > budget {
            let victim = self
                .sprites
                .iter()
                .filter(|(k, s)| {
                    k.url.starts_with(VECTOR_LAYER_PREFIX) && s.last_used <= self.frame_epoch
                })
                .min_by_key(|(_, s)| s.last_used)
                .map(|(k, _)| k.clone());
            let Some(k) = victim else { return };
            held -= layer_bytes(&k);
            self.drop_sprite(&k);
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

    /// Bytes of the cached vector layer textures (cache observability for tests).
    #[cfg(test)]
    pub(super) fn vector_layer_bytes(&self) -> usize {
        self.vector_layer_sizes()
            .iter()
            .map(|(w, h)| *w as usize * *h as usize * 4)
            .sum()
    }
}
