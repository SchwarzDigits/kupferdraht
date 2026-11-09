// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Decode an image file under limits. The magic bytes pick the decoder; the
//! dimensions are checked from the header, before anything is allocated for the
//! pixels. Of an animated image the first frame.

use std::io::Cursor;
use std::num::NonZeroU64;

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

use super::{AVATAR_MAX_INPUT_BYTES, Pixels, exif};
use crate::error::KupferdrahtError;

/// The longest side a decoder accepts.
const MAX_SIDE: u32 = 4096;
/// A cap on a decoder's allocations: the largest image as RGBA, and its buffers.
const MAX_DECODER_BYTES: usize = 96 * 1024 * 1024;

fn refused(what: &str) -> KupferdrahtError {
    KupferdrahtError::InvalidInput(format!("avatar image: {what}"))
}

pub(super) fn decode(input: &[u8]) -> Result<Pixels, KupferdrahtError> {
    if input.len() > AVATAR_MAX_INPUT_BYTES {
        return Err(refused("over the size limit"));
    }
    match input {
        [0xFF, 0xD8, 0xFF, ..] => jpeg(input),
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => png(input),
        [b'G', b'I', b'F', b'8', ..] => gif(input),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => webp(input),
        _ => Err(refused("no accepted image format")),
    }
}

fn within_limits(width: u32, height: u32) -> Result<(), KupferdrahtError> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(refused("dimensions over the limit"));
    }
    Ok(())
}

fn jpeg(input: &[u8]) -> Result<Pixels, KupferdrahtError> {
    let max_side = MAX_SIDE as usize;
    let options = DecoderOptions::default()
        .set_max_width(max_side)
        .set_max_height(max_side)
        .jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(input), options);
    decoder
        .decode_headers()
        .map_err(|_| refused("an unreadable JPEG"))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| refused("an unreadable JPEG"))?;
    let width = u32::try_from(width).map_err(|_| refused("dimensions over the limit"))?;
    let height = u32::try_from(height).map_err(|_| refused("dimensions over the limit"))?;
    within_limits(width, height)?;
    let orientation = decoder.exif().and_then(|bytes| exif::orientation(bytes));
    let rgb = decoder
        .decode()
        .map_err(|_| refused("an unreadable JPEG"))?;
    let pixels =
        Pixels::from_rgb(width, height, &rgb).ok_or_else(|| refused("an unreadable JPEG"))?;
    Ok(pixels.oriented(orientation.unwrap_or(1)))
}

fn png(input: &[u8]) -> Result<Pixels, KupferdrahtError> {
    let unreadable = || refused("an unreadable PNG");
    let limits = png::Limits {
        bytes: MAX_DECODER_BYTES,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(input), limits);
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().map_err(|_| unreadable())?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    within_limits(width, height)?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or_else(unreadable)?];
    let frame = reader.next_frame(&mut buffer).map_err(|_| unreadable())?;
    buffer.truncate(frame.buffer_size());
    let rgba = match frame.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::GrayscaleAlpha => buffer
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|[gray, alpha]| [*gray, *gray, *gray, *alpha])
            .collect(),
        _ => return Err(unreadable()),
    };
    Pixels::new(width, height, rgba).ok_or_else(unreadable)
}

fn gif(input: &[u8]) -> Result<Pixels, KupferdrahtError> {
    let unreadable = || refused("an unreadable GIF");
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    if let Some(limit) = NonZeroU64::new(MAX_DECODER_BYTES as u64) {
        options.set_memory_limit(gif::MemoryLimit::Bytes(limit));
    }
    let mut decoder = options
        .read_info(Cursor::new(input))
        .map_err(|_| unreadable())?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    within_limits(width, height)?;
    let frame = decoder
        .read_next_frame()
        .map_err(|_| unreadable())?
        .ok_or_else(unreadable)?;
    let mut canvas = vec![0; width as usize * height as usize * 4];
    draw_frame(&mut canvas, width as usize, height as usize, frame);
    Pixels::new(width, height, canvas).ok_or_else(unreadable)
}

/// A GIF's first frame on its canvas, cut to it: a frame may be smaller than
/// the canvas, offset from its corner, and reach past its edge.
fn draw_frame(canvas: &mut [u8], width: usize, height: usize, frame: &gif::Frame<'_>) {
    let (frame_w, left, top) = (
        usize::from(frame.width),
        usize::from(frame.left),
        usize::from(frame.top),
    );
    if frame_w == 0 || left >= width {
        return;
    }
    let visible = frame_w.min(width - left) * 4;
    for (row, line) in frame.buffer.chunks_exact(frame_w * 4).enumerate() {
        let y = row + top;
        if y >= height {
            break;
        }
        let at = (y * width + left) * 4;
        canvas[at..at + visible].copy_from_slice(&line[..visible]);
    }
}

fn webp(input: &[u8]) -> Result<Pixels, KupferdrahtError> {
    let unreadable = || refused("an unreadable WebP");
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(input)).map_err(|_| unreadable())?;
    let (width, height) = decoder.dimensions();
    within_limits(width, height)?;
    decoder.set_memory_limit(MAX_DECODER_BYTES);
    let mut buffer = vec![0; decoder.output_buffer_size().ok_or_else(unreadable)?];
    decoder.read_image(&mut buffer).map_err(|_| unreadable())?;
    let pixels = if decoder.has_alpha() {
        Pixels::new(width, height, buffer)
    } else {
        Pixels::from_rgb(width, height, &buffer)
    };
    pixels.ok_or_else(unreadable)
}
