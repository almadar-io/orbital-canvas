//! SDF shapes and materials: batched per pipeline, with the render targets their materials read.

use super::*;

/// A run of SDF shapes of one pipeline and one target, waiting to be cut into uniform-sized batches.
pub(super) struct PendingSdf {
    pub(super) key: u64,
    pub(super) wgsl: Option<String>,
    pub(super) instances: Vec<crate::sdf::SdfInstance>,
    pub(super) target: Option<crate::render_target::TargetTexture>,
}

impl SharedDeviceRenderer {
    /// Merge adjacent SDF runs of one pipeline and one target (nothing is drawn between them, so z-order holds),
    /// compile each pipeline once, and bind each batch of up to [`crate::sdf::MAX_INSTANCES`] shapes.
    pub(super) fn finalize_sdf(
        &mut self,
        draws: Vec<Draw>,
        packet: &GameRenderPacket,
    ) -> Vec<Draw> {
        let mut out: Vec<Draw> = Vec::with_capacity(draws.len());
        let mut pending: Option<PendingSdf> = None;
        for draw in draws {
            match draw {
                Draw::SdfRun {
                    key,
                    wgsl,
                    mut instances,
                    target,
                } => match pending.take() {
                    Some(mut run)
                        if run.key == key
                            && run.target.as_ref().map(|t| t.serial)
                                == target.as_ref().map(|t| t.serial) =>
                    {
                        run.instances.append(&mut instances);
                        pending = Some(run);
                    }
                    other => {
                        if let Some(run) = other {
                            self.bind_sdf(run, packet, &mut out);
                        }
                        pending = Some(PendingSdf {
                            key,
                            wgsl,
                            instances,
                            target,
                        });
                    }
                },
                other => {
                    if let Some(run) = pending.take() {
                        self.bind_sdf(run, packet, &mut out);
                    }
                    out.push(other);
                }
            }
        }
        if let Some(run) = pending {
            self.bind_sdf(run, packet, &mut out);
        }
        out
    }

    pub(super) fn bind_sdf(
        &mut self,
        run: PendingSdf,
        packet: &GameRenderPacket,
        out: &mut Vec<Draw>,
    ) {
        let (device, format) = (self.device.clone(), self.format);
        let pass = self
            .sdf
            .get_or_insert_with(|| crate::sdf::SdfPass::new(&device, format));
        if let Err(error) = pass.ensure(&device, run.key, run.wgsl.as_deref()) {
            tracing::error!(target: "almadar::game_renderer", %error, "sdf material not drawn");
            return;
        }
        let aux = pass.aux_group(&device, None, run.target.as_ref().map(|t| &t.view));
        let mut kept = Vec::new();
        for chunk in crate::sdf::batches(&run.instances) {
            let (bind_group, buffer) = pass.bind_group(
                &device,
                chunk,
                (packet.width, packet.height),
                packet.time_ms,
                packet.pointer,
                packet.scroll,
            );
            kept.push(buffer);
            out.push(Draw::Sdf {
                key: run.key,
                bind_group,
                aux: aux.clone(),
                count: chunk.len() as u32,
            });
        }
        for buffer in &kept {
            self.keep(buffer);
        }
    }

    /// The first render target a material names, as the reader resolving now sees it.
    pub(super) fn material_target(
        &self,
        material: Option<&orbital_canvas_base::packet::SdfMaterial>,
    ) -> Option<crate::render_target::TargetTexture> {
        material
            .and_then(|m| m.targets.first())
            .and_then(|id| self.targets.read(id))
    }
}
