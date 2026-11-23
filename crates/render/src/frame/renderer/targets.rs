//! Render targets painted where they stand in the command list.

use super::*;

impl SharedDeviceRenderer {
    /// Paint a `Target` command's children into its texture unless they are what it already holds. `points` is the
    /// target's logical size; the texture holds it at the `density` of the pass that paints it, so a target shown at
    /// its logical size is as sharp as the frame around it.
    pub(super) fn paint_target(
        &mut self,
        id: &str,
        points: (u32, u32),
        density: f32,
        live: bool,
        commands: &[DrawCommand],
        packet: &GameRenderPacket,
    ) {
        let limit = self.device.limits().max_texture_dimension_2d;
        let density = crate::render_target::density_within(points, density, limit);
        let size = (
            ((points.0 as f32) * density).round().max(1.0) as u32,
            ((points.1 as f32) * density).round().max(1.0) as u32,
        );
        if size.0 > limit || size.1 > limit {
            tracing::warn!(target: "almadar::game_renderer", id, ?size, "render target exceeds the device texture limit; skipped");
            return;
        }
        let versions = |read: &str| self.targets.version(read);
        let Some(hash) = crate::render_target::content_hash(
            size.0,
            size.1,
            live,
            commands,
            packet.time_ms,
            &versions,
        ) else {
            tracing::warn!(target: "almadar::game_renderer", id, "render target commands not serializable; skipped");
            return;
        };
        if self.targets.keep(id, size, hash) {
            return;
        }
        let slot = self.targets.writer(&self.device, self.format, id, size, points);
        let child = GameRenderPacket {
            pattern: packet.pattern.clone(),
            width: points.0 as f32,
            height: points.1 as f32,
            draw_commands: commands.to_vec(),
            time_ms: packet.time_ms,
            ..GameRenderPacket::default()
        };
        self.render_packet_to_texture(&child, &slot.texture);
        self.targets.painted(id, hash);
    }
}
