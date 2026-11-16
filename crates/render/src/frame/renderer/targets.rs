//! Render targets painted where they stand in the command list.

use super::*;

impl SharedDeviceRenderer {
    /// Paint a `Target` command's children into its texture unless they are what it already holds.
    pub(super) fn paint_target(
        &mut self,
        id: &str,
        size: (u32, u32),
        live: bool,
        commands: &[DrawCommand],
        packet: &GameRenderPacket,
    ) {
        let limit = self.device.limits().max_texture_dimension_2d;
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
        let slot = self.targets.writer(&self.device, self.format, id, size);
        let child = GameRenderPacket {
            pattern: packet.pattern.clone(),
            width: size.0 as f32,
            height: size.1 as f32,
            draw_commands: commands.to_vec(),
            time_ms: packet.time_ms,
            ..GameRenderPacket::default()
        };
        self.render_packet_to_texture(&child, &slot.texture);
        self.targets.painted(id, hash);
    }
}
