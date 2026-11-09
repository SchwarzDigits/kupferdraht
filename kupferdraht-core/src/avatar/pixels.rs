// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Decoded pixels: what is left of an image once its file is gone.

use super::{Avatar, AvatarSize, encode};
use crate::error::KupferdrahtError;

/// An image as RGBA, row by row. Its dimensions are positive and within the
/// decoders' limits, and `rgba` holds exactly `width * height * 4` bytes.
#[derive(Debug, Clone)]
pub(crate) struct Pixels {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

impl Pixels {
    /// `rgba` as an image, if its length fits the dimensions.
    pub(super) fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        (rgba.len() == width as usize * height as usize * 4).then_some(Self {
            width,
            height,
            rgba,
        })
    }

    /// RGB bytes as an opaque image, if their length fits the dimensions.
    pub(super) fn from_rgb(width: u32, height: u32, rgb: &[u8]) -> Option<Self> {
        let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
        for [r, g, b] in rgb.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[*r, *g, *b, 255]);
        }
        Self::new(width, height, rgba)
    }

    /// Whether any pixel is less than opaque.
    pub(super) fn is_transparent(&self) -> bool {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] != 255)
    }

    /// The image as an EXIF orientation says to show it: 1 to 8, the rows and
    /// columns of a phone that held the camera on its side.
    pub(super) fn oriented(self, orientation: u16) -> Self {
        if !(2..=8).contains(&orientation) {
            return self;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let (out_w, out_h) = if orientation >= 5 { (h, w) } else { (w, h) };
        let mut rgba = vec![0; self.rgba.len()];
        for (at, pixel) in self.rgba.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (at % w, at / w);
            let (out_x, out_y) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                _ => (y, w - 1 - x),
            };
            let out = (out_y * out_w + out_x) * 4;
            rgba[out..out + 4].copy_from_slice(pixel);
        }
        Self {
            width: u32::try_from(out_w).unwrap_or(self.height),
            height: u32::try_from(out_h).unwrap_or(self.width),
            rgba,
        }
    }

    /// Write a new image for `size`, scaled down if it exceeds the target.
    pub(crate) fn write_as(&self, size: AvatarSize) -> Result<Avatar, KupferdrahtError> {
        let target = size.target_pixels();
        if self.width <= target && self.height <= target {
            return encode::write(self);
        }
        encode::write(&self.scaled_to(target))
    }

    /// The image with its longer side at `target`, each new pixel the average
    /// of those it covers. The colours are weighted by their alpha, or a
    /// transparent pixel's hidden colour would bleed into its neighbours.
    fn scaled_to(&self, target: u32) -> Self {
        debug_assert!(target > 0, "a target size is positive");
        let (w, h) = if self.width >= self.height {
            (target, (self.height * target / self.width).max(1))
        } else {
            ((self.width * target / self.height).max(1), target)
        };
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h {
            let rows = span(y, h, self.height);
            for x in 0..w {
                rgba.extend_from_slice(&self.average(span(x, w, self.width), rows));
            }
        }
        Self {
            width: w,
            height: h,
            rgba,
        }
    }

    fn average(&self, columns: (u32, u32), rows: (u32, u32)) -> [u8; 4] {
        let mut sum = [0_u64; 3];
        let mut alpha = 0_u64;
        let stride = self.width as usize * 4;
        for row in rows.0..rows.1 {
            let start = row as usize * stride;
            let line = &self.rgba[start + columns.0 as usize * 4..start + columns.1 as usize * 4];
            for pixel in line.as_chunks::<4>().0 {
                let a = u64::from(pixel[3]);
                for (total, channel) in sum.iter_mut().zip(pixel) {
                    *total += u64::from(*channel) * a;
                }
                alpha += a;
            }
        }
        if alpha == 0 {
            return [0; 4];
        }
        let count = u64::from(columns.1 - columns.0) * u64::from(rows.1 - rows.0);
        [
            byte(sum[0] / alpha),
            byte(sum[1] / alpha),
            byte(sum[2] / alpha),
            byte(alpha / count),
        ]
    }
}

/// The source rows or columns new row or column `index` covers, at least one.
fn span(index: u32, out: u32, source: u32) -> (u32, u32) {
    let start = index * source / out;
    let end = ((index + 1) * source / out).max(start + 1);
    (start, end.min(source))
}

fn byte(value: u64) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}
