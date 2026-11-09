// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The one thing read from an image's EXIF: which way up it was taken. Without
//! it a phone's portrait photo lies on its side once the metadata is gone.
//! Everything here is the sender's data, so a broken EXIF answers `None`.

/// The orientation tag of the first image directory, TIFF's `0x0112`.
const ORIENTATION_TAG: u16 = 0x0112;
/// TIFF's `SHORT`, the tag's only valid type.
const SHORT: u16 = 3;
/// The size of one directory entry.
const ENTRY_BYTES: usize = 12;

/// The EXIF orientation, 1 to 8. `exif` starts at the TIFF header or at the
/// `Exif\0\0` before it.
pub(super) fn orientation(exif: &[u8]) -> Option<u16> {
    let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let big_endian = match tiff.get(..2)? {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    if read_u16(tiff, 2, big_endian)? != 42 {
        return None;
    }
    let directory = usize::try_from(read_u32(tiff, 4, big_endian)?).ok()?;
    let count = usize::from(read_u16(tiff, directory, big_endian)?);
    let entries = directory.checked_add(2)?;
    (0..count)
        .map_while(|index| {
            let entry = entries.checked_add(index.checked_mul(ENTRY_BYTES)?)?;
            read_u16(tiff, entry, big_endian).map(|tag| (entry, tag))
        })
        .find(|(_, tag)| *tag == ORIENTATION_TAG)
        .and_then(|(entry, _)| {
            let value = read_u16(tiff, entry.checked_add(8)?, big_endian)?;
            let valid = read_u16(tiff, entry.checked_add(2)?, big_endian)? == SHORT
                && read_u32(tiff, entry.checked_add(4)?, big_endian)? == 1
                && (1..=8).contains(&value);
            valid.then_some(value)
        })
}

fn read_u16(bytes: &[u8], at: usize, big_endian: bool) -> Option<u16> {
    let raw: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(if big_endian {
        u16::from_be_bytes(raw)
    } else {
        u16::from_le_bytes(raw)
    })
}

fn read_u32(bytes: &[u8], at: usize, big_endian: bool) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(if big_endian {
        u32::from_be_bytes(raw)
    } else {
        u32::from_le_bytes(raw)
    })
}
