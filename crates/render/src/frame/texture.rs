//! Moving RGBA8 pixels between the CPU and a texture: one upload and one readback for every pass.

use crate::{RendererError, Result};

/// A new 2D texture of `width` x `height` holding `rgba` (tightly packed RGBA8, top row first) in its first mip level;
/// `usage` is added to `COPY_DST`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    rgba: &[u8],
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    mip_levels: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: mip_levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: usage | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    texture
}

fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

/// Record a texture→buffer copy into `encoder`, submit, and read the pixels
/// back as a tightly-packed RGBA8 buffer (top-to-bottom rows). Takes the
/// encoder by value (it finishes + submits it). Shared by the headless
/// readback path, tests of the shared-device renderer, and the
/// `dump-canvas-packet` example (fixture → PNG visual proof).
pub fn copy_texture_to_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let bytes_per_row_unpadded = width * 4;
    let bytes_per_row_padded = align_up(
        bytes_per_row_unpadded as usize,
        wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize,
    );
    let buffer_size = bytes_per_row_padded * height as usize;

    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("game readback"),
        size: buffer_size as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row_padded as u32),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device.poll(wgpu::PollType::wait()).ok();
    rx.recv()
        .map_err(|_| RendererError::Surface("readback channel closed".into()))?
        .map_err(|e| RendererError::Surface(format!("buffer map failed: {e}")))?;

    let data = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = row as usize * bytes_per_row_padded;
        rgba.extend_from_slice(&data[start..start + bytes_per_row_unpadded as usize]);
    }
    drop(data);
    readback_buffer.unmap();

    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::{copy_texture_to_rgba, upload_rgba};

    #[test]
    fn uploaded_pixels_read_back_unchanged() {
        let Ok(shared) = crate::device::shared_device() else {
            return;
        };
        let rgba: Vec<u8> = (0..3 * 2 * 4).map(|i| (i * 9) as u8).collect();
        let texture = upload_rgba(
            &shared.device,
            &shared.queue,
            "upload test",
            &rgba,
            3,
            2,
            wgpu::TextureFormat::Rgba8Unorm,
            1,
            wgpu::TextureUsages::COPY_SRC,
        );
        let encoder = shared
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let back = copy_texture_to_rgba(&shared.device, &shared.queue, encoder, &texture, 3, 2)
            .expect("readback");
        assert_eq!(back, rgba, "an odd width still reads back row by row");
        let other: Vec<u8> = vec![7; 3 * 2 * 4];
        assert_ne!(back, other, "control: different pixels differ");
    }
}
