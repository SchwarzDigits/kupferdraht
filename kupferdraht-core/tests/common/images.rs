// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Images built in memory for Step 144's tests, and the means to read an
//! output back, shared by the native tests and, through `#[path]`, the browser
//! test. Built with the same format crates the code under test reads with.
#![allow(dead_code)]

use std::io::Cursor;

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

pub const RED: [u8; 3] = [230, 20, 20];
pub const GREEN: [u8; 3] = [20, 200, 20];
pub const BLUE: [u8; 3] = [20, 20, 230];
pub const WHITE: [u8; 3] = [250, 250, 250];

pub fn jpeg(w: u16, h: u16, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(usize::from(w) * usize::from(h) * 3);
    for y in 0..u32::from(h) {
        for x in 0..u32::from(w) {
            rgb.extend_from_slice(&pixel(x, y));
        }
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 95)
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .expect("fixture jpeg");
    out
}

/// A flat JPEG of one colour.
pub fn flat_jpeg(w: u16, h: u16) -> Vec<u8> {
    jpeg(w, h, |_, _| RED)
}

/// Four quadrants: red, green over blue, white. Orientation shows in them.
pub fn quadrant_jpeg(w: u16, h: u16) -> Vec<u8> {
    let (mid_x, mid_y) = (u32::from(w) / 2, u32::from(h) / 2);
    jpeg(w, h, move |x, y| match (x < mid_x, y < mid_y) {
        (true, true) => RED,
        (false, true) => GREEN,
        (true, false) => BLUE,
        (false, false) => WHITE,
    })
}

/// A JPEG of noise: large on disk, and written anew about as large.
pub fn noise_jpeg(w: u16, h: u16) -> Vec<u8> {
    let state = std::cell::Cell::new(0x1234_5678_u32);
    let mut rgb = Vec::with_capacity(usize::from(w) * usize::from(h) * 3);
    for _ in 0..(u32::from(w) * u32::from(h) * 3) {
        let mut s = state.get();
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        state.set(s);
        rgb.push(s.to_le_bytes()[0]);
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 80)
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .expect("fixture jpeg");
    out
}

fn png(w: u32, h: u32, color: png::ColorType, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("fixture png header")
        .write_image_data(pixels)
        .expect("fixture png data");
    out
}

pub fn opaque_png(w: u32, h: u32) -> Vec<u8> {
    let pixels: Vec<u8> = RED
        .iter()
        .copied()
        .cycle()
        .take((w * h * 3) as usize)
        .collect();
    png(w, h, png::ColorType::Rgb, &pixels)
}

/// Opaque red with one transparent pixel, so nothing but an alpha check finds it.
pub fn transparent_png(w: u32, h: u32) -> Vec<u8> {
    let mut pixels: Vec<u8> = [RED[0], RED[1], RED[2], 255]
        .iter()
        .copied()
        .cycle()
        .take((w * h * 4) as usize)
        .collect();
    pixels[3] = 0;
    png(w, h, png::ColorType::Rgba, &pixels)
}

/// Two frames: `first` then `second`, one colour each.
pub fn animated_gif(w: u16, h: u16, first: [u8; 3], second: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, w, h, &[]).expect("fixture gif");
        let pixels = vec![0_u8; usize::from(w) * usize::from(h)];
        for colour in [first, second] {
            let frame =
                gif::Frame::from_palette_pixels(w, h, pixels.clone(), colour.to_vec(), None);
            encoder.write_frame(&frame).expect("fixture gif frame");
        }
    }
    out
}

pub fn opaque_webp(w: u32, h: u32) -> Vec<u8> {
    let pixels: Vec<u8> = RED
        .iter()
        .copied()
        .cycle()
        .take((w * h * 3) as usize)
        .collect();
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(Cursor::new(&mut out))
        .encode(&pixels, w, h, image_webp::ColorType::Rgb8)
        .expect("fixture webp");
    out
}

pub fn transparent_webp(w: u32, h: u32) -> Vec<u8> {
    let mut pixels: Vec<u8> = [RED[0], RED[1], RED[2], 255]
        .iter()
        .copied()
        .cycle()
        .take((w * h * 4) as usize)
        .collect();
    pixels[3] = 0;
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(Cursor::new(&mut out))
        .encode(&pixels, w, h, image_webp::ColorType::Rgba8)
        .expect("fixture webp");
    out
}

/// `segment` (marker and payload) right after a JPEG's start-of-image.
fn with_segment(jpeg: &[u8], marker: u8, payload: &[u8]) -> Vec<u8> {
    let length = u16::try_from(payload.len() + 2)
        .expect("segment fits")
        .to_be_bytes();
    let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&length);
    out.extend_from_slice(payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// The JPEG with `text` in a comment segment.
pub fn with_comment(jpeg: &[u8], text: &[u8]) -> Vec<u8> {
    with_segment(jpeg, 0xFE, text)
}

/// The JPEG with an APP1 Exif segment of one tag, orientation `value`.
pub fn with_orientation(jpeg: &[u8], value: u16, big_endian: bool) -> Vec<u8> {
    let u16b = |v: u16| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let u32b = |v: u32| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(if big_endian { b"MM" } else { b"II" });
    exif.extend_from_slice(&u16b(42));
    exif.extend_from_slice(&u32b(8));
    exif.extend_from_slice(&u16b(1));
    exif.extend_from_slice(&u16b(0x0112));
    exif.extend_from_slice(&u16b(3));
    exif.extend_from_slice(&u32b(1));
    exif.extend_from_slice(&u16b(value));
    exif.extend_from_slice(&[0, 0]);
    exif.extend_from_slice(&u32b(0));
    with_segment(jpeg, 0xE1, &exif)
}

/// The JPEG with an APP1 Exif segment holding `payload` after the header.
pub fn with_raw_exif(jpeg: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(payload);
    with_segment(jpeg, 0xE1, &exif)
}

/// The JPEG's start-of-frame patched to declare `w` × `h`.
pub fn jpeg_declaring(jpeg: &[u8], w: u16, h: u16) -> Vec<u8> {
    let mut out = jpeg.to_vec();
    let at = out
        .windows(2)
        .position(|pair| pair == [0xFF, 0xC0])
        .expect("baseline start of frame");
    out[at + 5..at + 7].copy_from_slice(&h.to_be_bytes());
    out[at + 7..at + 9].copy_from_slice(&w.to_be_bytes());
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A PNG of a signature and a header alone, declaring `w` × `h`, RGBA.
pub fn png_declaring(w: u32, h: u32) -> Vec<u8> {
    let mut chunk = b"IHDR".to_vec();
    chunk.extend_from_slice(&w.to_be_bytes());
    chunk.extend_from_slice(&h.to_be_bytes());
    chunk.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&13_u32.to_be_bytes());
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc32(&chunk).to_be_bytes());
    out
}

/// The PNG with a `tEXt` chunk of `text` right after its header.
pub fn with_text_chunk(png: &[u8], text: &[u8]) -> Vec<u8> {
    let mut chunk = b"tEXt".to_vec();
    chunk.extend_from_slice(b"Comment\0");
    chunk.extend_from_slice(text);
    let length = u32::try_from(chunk.len() - 4).expect("fits").to_be_bytes();
    let after_header = 8 + 25;
    let mut out = png[..after_header].to_vec();
    out.extend_from_slice(&length);
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc32(&chunk).to_be_bytes());
    out.extend_from_slice(&png[after_header..]);
    out
}

/// A GIF of a header alone, declaring `w` × `h`.
pub fn gif_declaring(w: u16, h: u16) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, b';']);
    out
}

/// A WebP of an extended header alone, declaring `w` × `h`.
pub fn webp_declaring(w: u32, h: u32) -> Vec<u8> {
    let mut chunk = b"VP8X".to_vec();
    chunk.extend_from_slice(&10_u32.to_le_bytes());
    chunk.extend_from_slice(&[0; 4]);
    chunk.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
    chunk.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&u32::try_from(chunk.len() + 4).expect("fits").to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&chunk);
    out
}

/// `(width, height, rgb)` of a JPEG.
pub fn read_jpeg(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let rgb = decoder.decode().expect("the output is a JPEG");
    let (w, h) = decoder.dimensions().expect("dimensions");
    (
        u32::try_from(w).expect("width"),
        u32::try_from(h).expect("height"),
        rgb,
    )
}

/// `(width, height)` of a PNG, and whether any pixel is transparent.
pub fn read_png(bytes: &[u8]) -> (u32, u32, bool) {
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .expect("the output is a PNG");
    let mut buffer = vec![0; reader.output_buffer_size().expect("size")];
    let info = reader.next_frame(&mut buffer).expect("the frame");
    assert_eq!(info.color_type, png::ColorType::Rgba, "written as RGBA");
    let transparent = buffer[..info.buffer_size()]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[3] != 255);
    (info.width, info.height, transparent)
}

/// The pixel at `(x, y)` of an RGB buffer of width `w`.
pub fn pixel(rgb: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let at = ((y * w + x) * 3) as usize;
    [rgb[at], rgb[at + 1], rgb[at + 2]]
}

/// Whether two colours match within what a lossy encoder moves them.
pub fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 40)
}
