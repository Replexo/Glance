// Copyright (c) 2026 madarra16
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Saving edits that change pixels
//!
//! Crop, rotate and flip alone are saved through glycin's editor, which can
//! work losslessly. Pixel edits (adjustments) need the decoded image: it is
//! loaded with orientation applied, the geometric operations are replayed on
//! it, then the pixel edits are applied and the result is encoded again in
//! the original format.

use glycin::{MemoryFormat, MemoryFormatSelection, Operation, Operations};
use gufo::common::orientation::Rotation;
use image::RgbaImage;

use std::sync::Arc;

use super::adjust::Adjustments;
use super::markup::{self, Shape};
use crate::deps::*;

/// Edits that need the image to be decoded and encoded again
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PixelEdits {
    pub adjustments: Adjustments,
    pub markup: Arc<Vec<Shape>>,
}

impl PixelEdits {
    pub fn is_empty(&self) -> bool {
        self.adjustments.is_identity() && self.markup.is_empty()
    }

    /// Applies the pixel edits to an image that already has the geometric
    /// `operations` applied to its `original_size`
    ///
    /// Markup is drawn after the adjustments, so its colours stay as picked.
    pub fn apply(
        &self,
        image: &mut RgbaImage,
        operations: Option<&Operations>,
        original_size: (u32, u32),
    ) -> anyhow::Result<()> {
        self.adjustments.apply_rgba8(image.as_mut());

        let original_size = (original_size.0 as f64, original_size.1 as f64);
        let (matrix, _) = markup::operations_matrix(operations, original_size);
        markup::flatten(image, &self.markup, matrix)
    }
}

/// Loads `file`, applies `operations` and `edits` and returns the encoded
/// image in the file's original format
///
/// The colour profile and pixel density are kept when the format supports
/// them. Other metadata is not written. The pixel work runs on a thread pool.
pub async fn render(
    file: gio::File,
    operations: Option<Operations>,
    edits: PixelEdits,
    cancellable: &gio::Cancellable,
) -> anyhow::Result<Vec<u8>> {
    // Only reads the header, to know the format
    let mut loader = glycin::Loader::new(file.clone());
    loader.cancellable(cancellable.clone());
    let mime_type = loader.load().await?.mime_type();

    let keep_icc_profile = supports_icc_profile(mime_type.clone()).await;
    let mut loader = glycin::Loader::new(file);
    loader
        .accepted_memory_formats(MemoryFormatSelection::R8g8b8a8)
        .color_convert_icc_srgb(!keep_icc_profile)
        .cancellable(cancellable.clone());
    let mut image = loader.load().await?;
    let frame = image.next_frame().await?;

    if frame.memory_format() != MemoryFormat::R8g8b8a8 {
        anyhow::bail!("Unexpected memory format {:?}", frame.memory_format());
    }

    let details = frame.details();
    let icc_profile = details
        .color_icc_profile()
        .filter(|_| keep_icc_profile)
        .map(|x| x.to_vec());
    let pixel_density = details.pixel_density();

    let pixels = rgba_from_strided(
        frame.buf_slice(),
        frame.width(),
        frame.height(),
        frame.stride(),
    )?;
    drop(frame);
    drop(image);

    let (format, width, height, data) = gio::spawn_blocking(move || {
        let mut pixels = pixels;
        let original_size = pixels.dimensions();
        if let Some(operations) = &operations {
            pixels = apply_operations(pixels, operations)?;
        }

        edits.apply(&mut pixels, operations.as_ref(), original_size)?;

        let (width, height) = pixels.dimensions();
        // glycin's conversion from RGBA to a format without alpha prefers grey
        // over RGB, so opaque images are handed over as RGB
        anyhow::Ok(if pixels.pixels().all(|pixel| pixel.0[3] == 255) {
            let rgb = image::DynamicImage::ImageRgba8(pixels).into_rgb8();
            (MemoryFormat::R8g8b8, width, height, rgb.into_raw())
        } else {
            (MemoryFormat::R8g8b8a8, width, height, pixels.into_raw())
        })
    })
    .await
    .map_err(|_| anyhow::anyhow!("Editing thread panicked"))??;

    let mut creator = glycin::Creator::new(mime_type).await?;
    creator.cancellable(cancellable.clone());
    // Ignored for formats without a quality setting
    let _ = creator.set_encoding_quality(92);
    let new_frame = creator.add_frame(width, height, format, data)?;
    if new_frame.set_color_icc_profile(icc_profile).is_err() {
        tracing::warn!("Colour profile not supported by encoder");
    }
    // Not all formats store a density
    let _ = new_frame.set_pixel_density(pixel_density);
    let encoded = creator.create().await?;

    Ok(encoded.data_full())
}

/// Whether glycin can write colour profiles for this format
async fn supports_icc_profile(mime_type: glycin::MimeType) -> bool {
    let Ok(mut creator) = glycin::Creator::new(mime_type).await else {
        return false;
    };
    creator
        .add_frame(1, 1, MemoryFormat::R8g8b8a8, vec![0; 4])
        .is_ok_and(|frame| frame.set_color_icc_profile(Some(vec![0])).is_ok())
}

/// Loads `file` and returns RGBA8 pixels of a version that is at most about
/// `size` pixels wide or high, for histograms
pub async fn load_sample(file: gio::File, size: u32) -> anyhow::Result<Vec<u8>> {
    let mut loader = glycin::Loader::new(file);
    loader.accepted_memory_formats(MemoryFormatSelection::R8g8b8a8);
    let mut image = loader.load().await?;
    // Loaders may ignore the requested size, so the result is thinned out
    let frame = image
        .specific_frame(glycin::FrameRequest::new().scale(size, size))
        .await?;
    let (width, height, stride) = (frame.width(), frame.height(), frame.stride());
    let buf = frame.buf_bytes();
    drop(frame);

    gio::spawn_blocking(move || {
        let step = (width.max(height) / size.max(1)).max(1) as usize;
        let mut sample = Vec::new();
        for y in (0..height as usize).step_by(step) {
            let row = buf
                .get(y * stride as usize..y * stride as usize + width as usize * 4)
                .ok_or_else(|| anyhow::anyhow!("Frame buffer too small"))?;
            for pixel in row.chunks_exact(4).step_by(step) {
                sample.extend_from_slice(pixel);
            }
        }
        Ok(sample)
    })
    .await
    .map_err(|_| anyhow::anyhow!("Sampling thread panicked"))?
}

fn rgba_from_strided(
    buf: &[u8],
    width: u32,
    height: u32,
    stride: u32,
) -> anyhow::Result<RgbaImage> {
    let row_len = width as usize * 4;
    let stride = stride as usize;
    let mut data = Vec::with_capacity(row_len * height as usize);

    for row in 0..height as usize {
        let start = row * stride;
        let row_data = buf
            .get(start..start + row_len)
            .ok_or_else(|| anyhow::anyhow!("Frame buffer too small"))?;
        data.extend_from_slice(row_data);
    }

    RgbaImage::from_raw(width, height, data)
        .ok_or_else(|| anyhow::anyhow!("Invalid frame dimensions"))
}

/// Replays glycin's geometric operations on decoded pixels
///
/// Matches `editing::preview`: each operation works on the result of the
/// previous one and rotations are counter-clockwise.
pub fn apply_operations(
    mut image: RgbaImage,
    operations: &Operations,
) -> anyhow::Result<RgbaImage> {
    use image::imageops;

    for operation in operations.operations() {
        image = match operation {
            Operation::Clip((x, y, width, height)) => {
                let (image_width, image_height) = image.dimensions();
                if *width == 0
                    || *height == 0
                    || x.saturating_add(*width) > image_width
                    || y.saturating_add(*height) > image_height
                {
                    anyhow::bail!("Clip {operation:?} outside of {image_width}×{image_height}");
                }
                imageops::crop_imm(&image, *x, *y, *width, *height).to_image()
            }
            Operation::MirrorHorizontally => imageops::flip_horizontal(&image),
            Operation::MirrorVertically => imageops::flip_vertical(&image),
            Operation::Rotate(rotation) => match rotation {
                Rotation::_0 => image,
                Rotation::_90 => imageops::rotate270(&image),
                Rotation::_180 => imageops::rotate180(&image),
                Rotation::_270 => imageops::rotate90(&image),
            },
            unsupported => anyhow::bail!("Unsupported operation {unsupported:?}"),
        };
    }

    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3×2 image where each pixel's red value is its index
    fn numbered() -> RgbaImage {
        RgbaImage::from_fn(3, 2, |x, y| image::Rgba([(y * 3 + x) as u8, 0, 0, 255]))
    }

    fn reds(image: &RgbaImage) -> Vec<u8> {
        image.pixels().map(|p| p.0[0]).collect()
    }

    fn run(operations: Vec<Operation>) -> RgbaImage {
        apply_operations(numbered(), &Operations::new(operations)).unwrap()
    }

    #[test]
    fn clip() {
        let image = run(vec![Operation::Clip((1, 0, 2, 2))]);
        assert_eq!(image.dimensions(), (2, 2));
        assert_eq!(reds(&image), [1, 2, 4, 5]);
    }

    #[test]
    fn clip_outside_fails() {
        let result = apply_operations(
            numbered(),
            &Operations::new(vec![Operation::Clip((2, 0, 2, 2))]),
        );
        assert!(result.is_err());
        let result = apply_operations(
            numbered(),
            &Operations::new(vec![Operation::Clip((0, 0, 0, 2))]),
        );
        assert!(result.is_err());
    }

    #[test]
    fn mirror() {
        assert_eq!(
            reds(&run(vec![Operation::MirrorHorizontally])),
            [2, 1, 0, 5, 4, 3]
        );
        assert_eq!(
            reds(&run(vec![Operation::MirrorVertically])),
            [3, 4, 5, 0, 1, 2]
        );
    }

    #[test]
    fn rotate_counter_clockwise() {
        // 0 1 2      2 5
        // 3 4 5  ->  1 4
        //            0 3
        let image = run(vec![Operation::Rotate(Rotation::_90)]);
        assert_eq!(image.dimensions(), (2, 3));
        assert_eq!(reds(&image), [2, 5, 1, 4, 0, 3]);

        let image = run(vec![Operation::Rotate(Rotation::_270)]);
        assert_eq!(reds(&image), [3, 0, 4, 1, 5, 2]);

        let image = run(vec![Operation::Rotate(Rotation::_180)]);
        assert_eq!(reds(&image), [5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn clip_after_rotate_uses_rotated_coordinates() {
        let image = run(vec![
            Operation::Rotate(Rotation::_90),
            Operation::Clip((0, 1, 2, 2)),
        ]);
        assert_eq!(reds(&image), [1, 4, 0, 3]);
    }

    #[test]
    fn strided_rows() {
        // Two pixels per row plus 4 bytes of padding
        let buf = [
            1, 1, 1, 1, 2, 2, 2, 2, 0, 0, 0, 0, 3, 3, 3, 3, 4, 4, 4, 4, 0, 0, 0, 0,
        ];
        let image = rgba_from_strided(&buf, 2, 2, 12).unwrap();
        assert_eq!(reds(&image), [1, 2, 3, 4]);
        // The last row needs no padding, but all of its pixels
        assert!(rgba_from_strided(&buf[..20], 2, 2, 12).is_ok());
        assert!(rgba_from_strided(&buf[..19], 2, 2, 12).is_err());
    }
}
