//! Sprite sheets and SVG rasters on the GPU: uploaded once per size, kept while drawn, evicted when stale.

use super::*;

/// Rasterized textures kept per SVG URL. Raster scales are bucketed to
/// quarter-octaves ([`orbital_canvas_paint::svg::raster_scale`]), so four sizes span one
/// octave of zoom; older sizes are evicted so a long session across many
/// zoom levels holds bounded GPU memory.
pub(super) const MAX_SVG_RASTERS_PER_URL: usize = 4;

impl SharedDeviceRenderer {
    /// Upload a sprite to the GPU once its bytes have arrived (decode here, at
    /// upload time - the byte cache stays format-agnostic). Raster sheets are
    /// uploaded once per URL at their own size. SVGs have no pixel size, so
    /// they are rasterized at `target_scale` device pixels per intrinsic unit
    /// (bucketed, see [`orbital_canvas_paint::svg::raster_scale`]) and cached per
    /// (URL, texture size); UVs still index the intrinsic size, so `src`
    /// rects mean the same thing for both kinds. Returns the cached entry
    /// when ready; `None` while in-flight or on failure.
    pub(super) fn ensure_sprite(
        &mut self,
        url: &str,
        target_scale: f32,
    ) -> Option<(SpriteKey, &GpuSprite)> {
        let bytes = orbital_canvas_paint::asset::asset_bytes(url)?;
        self.use_clock += 1;
        let now = self.use_clock;
        let (key, rgba, sheet, tex) = match orbital_canvas_paint::asset::asset_kind(url)? {
            orbital_canvas_paint::asset::AssetKind::Raster => {
                let key = SpriteKey {
                    url: url.to_string(),
                    raster: None,
                };
                if let Some(sprite) = self.sprites.get_mut(&key) {
                    sprite.last_used = now;
                    return self.sprites.get(&key).map(|s| (key, s));
                }
                let decoded = match image::ImageReader::new(std::io::Cursor::new(&bytes))
                    .with_guessed_format()
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.decode().map_err(|e| e.to_string()))
                {
                    Ok(img) => img.into_rgba8(),
                    Err(e) => {
                        tracing::warn!(target: "almadar::game_renderer", url, error = %e, "sprite decode failed");
                        return None;
                    }
                };
                let dims = decoded.dimensions();
                (key, decoded.into_raw(), dims, dims)
            }
            orbital_canvas_paint::asset::AssetKind::Svg => {
                let sheet = orbital_canvas_paint::svg::intrinsic_size(url, &bytes)?;
                let tex = orbital_canvas_paint::svg::raster_size(
                    sheet,
                    orbital_canvas_paint::svg::raster_scale(target_scale),
                    self.device.limits().max_texture_dimension_2d,
                );
                let key = SpriteKey {
                    url: url.to_string(),
                    raster: Some(tex),
                };
                if let Some(sprite) = self.sprites.get_mut(&key) {
                    sprite.last_used = now;
                    return self.sprites.get(&key).map(|s| (key, s));
                }
                let raster = orbital_canvas_paint::svg::rasterize(url, &bytes, tex.0, tex.1)?;
                (key, raster.rgba, sheet, tex)
            }
        };
        let (width, height) = tex;
        let texture = crate::texture::upload_rgba(
            &self.device,
            &self.queue,
            "game sprite sheet",
            &rgba,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("game sprite bind group"),
            layout: &self.sprite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        tracing::info!(target: "almadar::game_renderer", url, width, height, sheet_w = sheet.0, sheet_h = sheet.1, "sprite uploaded");
        if key.raster.is_some() {
            self.evict_stale_svg_rasters(url);
        }
        self.sprites.insert(
            key.clone(),
            GpuSprite {
                width: sheet.0,
                height: sheet.1,
                bind_group,
                last_used: now,
            },
        );
        self.sprites.get(&key).map(|s| (key, s))
    }

    pub(super) fn upload_rgba_texture(
        &self,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> wgpu::Texture {
        crate::texture::upload_rgba(
            &self.device,
            &self.queue,
            "game vector layer",
            rgba,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING,
        )
    }

    /// Upload straight-alpha RGBA8 as a sampled texture with its bind group.
    pub(super) fn upload_rgba(&self, rgba: &[u8], width: u32, height: u32, now: u64) -> GpuSprite {
        let texture = self.upload_rgba_texture(rgba, width, height);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("game vector layer bind group"),
            layout: &self.sprite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        GpuSprite {
            width,
            height,
            bind_group,
            last_used: now,
        }
    }

    /// Make room for one more raster of `url`: while the URL holds
    /// `MAX_SVG_RASTERS_PER_URL` textures, drop the least recently used one
    /// that the in-progress frame has not already requested.
    pub(super) fn evict_stale_svg_rasters(&mut self, url: &str) {
        loop {
            let held = self
                .sprites
                .keys()
                .filter(|k| k.raster.is_some() && k.url == url)
                .count();
            if held < MAX_SVG_RASTERS_PER_URL {
                return;
            }
            let victim = self
                .sprites
                .iter()
                .filter(|(k, s)| {
                    k.raster.is_some() && k.url == url && s.last_used <= self.frame_epoch
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

    /// Rasterized SVG texture sizes currently cached (cache observability for tests).
    #[cfg(test)]
    pub(super) fn svg_texture_sizes(&self) -> Vec<(u32, u32)> {
        let mut sizes: Vec<_> = self.sprites.keys().filter_map(|k| k.raster).collect();
        sizes.sort_unstable();
        sizes
    }
}
