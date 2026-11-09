// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Write a new image from pixels: JPEG, or PNG when the image uses
//! transparency. Metadata, trailing data and any structure aimed at a decoder
//! are gone by construction, since nothing of the old file is here to copy.

use super::{Avatar, Pixels};
use crate::error::KupferdrahtError;

/// JPEG quality of a written image, as Wire's webapp compresses a profile image.
const JPEG_QUALITY: u8 = 80;

pub(super) fn write(pixels: &Pixels) -> Result<Avatar, KupferdrahtError> {
    if pixels.is_transparent() {
        Ok(Avatar {
            bytes: write_png(pixels)?,
            mime_type: "image/png".to_owned(),
        })
    } else {
        Ok(Avatar {
            bytes: write_jpeg(pixels)?,
            mime_type: "image/jpeg".to_owned(),
        })
    }
}

fn write_jpeg(pixels: &Pixels) -> Result<Vec<u8>, KupferdrahtError> {
    let failed = |e: String| KupferdrahtError::Encoding(format!("write avatar JPEG: {e}"));
    let width = u16::try_from(pixels.width).map_err(|e| failed(e.to_string()))?;
    let height = u16::try_from(pixels.height).map_err(|e| failed(e.to_string()))?;
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY)
        .encode(&pixels.rgba, width, height, jpeg_encoder::ColorType::Rgba)
        .map_err(|e| failed(e.to_string()))?;
    Ok(out)
}

fn write_png(pixels: &Pixels) -> Result<Vec<u8>, KupferdrahtError> {
    let failed =
        |e: png::EncodingError| KupferdrahtError::Encoding(format!("write avatar PNG: {e}"));
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, pixels.width, pixels.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(failed)?;
    writer.write_image_data(&pixels.rgba).map_err(failed)?;
    writer.finish().map_err(failed)?;
    Ok(out)
}
