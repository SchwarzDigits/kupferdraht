// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 144 — every avatar is written anew (TDD red).
//!
//! A crafted image attacks the decoder that renders it, and avatars are chosen
//! freely by every user in a shared room. Kupferdraht decodes each one with Rust
//! decoders, under limits, and hands on an image it wrote from the pixels. These
//! are the known inputs: what must not survive, what is refused before it
//! allocates, and that nothing malformed panics, since a panic ends the browser's
//! wasm instance.

use kupferdraht::avatar::{AVATAR_MAX_INPUT_BYTES, rewrite_image};
use kupferdraht::{Avatar, AvatarSize, KupferdrahtError};

#[path = "common/images.rs"]
mod images;

use images::{BLUE, GREEN, RED, WHITE, close, pixel, read_jpeg, read_png};

fn thumbnail(input: &[u8]) -> Result<Avatar, KupferdrahtError> {
    rewrite_image(input, AvatarSize::Thumbnail)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn a_jpeg_comes_out_written_anew() {
    let input = images::flat_jpeg(64, 64);

    let output = thumbnail(&input).expect("a JPEG is an image");

    assert_eq!(output.mime_type, "image/jpeg");
    assert_ne!(output.bytes, input, "not the bytes that came in");
    let (w, h, rgb) = read_jpeg(&output.bytes);
    assert_eq!((w, h), (64, 64));
    assert!(close(pixel(&rgb, w, 32, 32), RED), "the same picture");
}

#[test]
fn data_behind_the_end_marker_does_not_survive() {
    let mut input = images::flat_jpeg(64, 64);
    input.extend_from_slice(b"<html><script>alert(1)</script></html>");

    let output = thumbnail(&input).expect("the image part decodes");

    assert!(!contains(&output.bytes, b"<script>"));
}

/// An image and an HTML document at once: it stays an image, the HTML goes.
#[test]
fn html_in_a_comment_does_not_survive() {
    let input = images::with_comment(&images::flat_jpeg(64, 64), b"<script>alert(1)</script>");

    let output = thumbnail(&input).expect("a valid JPEG");

    assert!(!contains(&output.bytes, b"<script>"));
}

/// A PNG written by the same encoder comes out byte for byte the same, which
/// is no failure: what counts is that nothing foreign comes along.
#[test]
fn what_a_png_carries_besides_its_pixels_does_not_survive() {
    let mut input = images::with_text_chunk(&images::transparent_png(16, 16), b"<script>chunk");
    input.extend_from_slice(b"<script>trailing");

    let output = thumbnail(&input).expect("the image part decodes");

    assert_eq!(output.mime_type, "image/png");
    assert!(!contains(&output.bytes, b"<script>"));
}

#[test]
fn exif_does_not_survive() {
    let input = images::with_raw_exif(
        &images::flat_jpeg(64, 64),
        b"MM\0*GPS-POSITION-OF-THE-PHONE",
    );

    let output = thumbnail(&input).expect("a valid JPEG");

    assert!(!contains(&output.bytes, b"GPS-POSITION"));
    assert!(!contains(&output.bytes, b"Exif"), "no EXIF segment at all");
}

/// The four corner colours: top left, top right, bottom left, bottom right.
type Corners = [[u8; 3]; 4];
/// `(orientation, (width, height), corners)`.
type Orientation = (u16, (u32, u32), Corners);

/// Of a 64 × 32 image of red, green over blue, white.
const ORIENTATIONS: [Orientation; 8] = [
    (1, (64, 32), [RED, GREEN, BLUE, WHITE]),
    (2, (64, 32), [GREEN, RED, WHITE, BLUE]),
    (3, (64, 32), [WHITE, BLUE, GREEN, RED]),
    (4, (64, 32), [BLUE, WHITE, RED, GREEN]),
    (5, (32, 64), [RED, BLUE, GREEN, WHITE]),
    (6, (32, 64), [BLUE, RED, WHITE, GREEN]),
    (7, (32, 64), [WHITE, GREEN, BLUE, RED]),
    (8, (32, 64), [GREEN, WHITE, RED, BLUE]),
];

#[test]
fn the_exif_orientation_is_applied_in_either_byte_order() {
    let source = images::quadrant_jpeg(64, 32);
    for (orientation, (w, h), [tl, tr, bl, br]) in ORIENTATIONS {
        for big_endian in [false, true] {
            let input = images::with_orientation(&source, orientation, big_endian);

            let output = thumbnail(&input).expect("a valid JPEG");

            let (out_w, out_h, rgb) = read_jpeg(&output.bytes);
            let label = format!("orientation {orientation}, big endian {big_endian}");
            assert_eq!((out_w, out_h), (w, h), "{label}: dimensions");
            let (right, bottom) = (out_w - 4, out_h - 4);
            assert!(close(pixel(&rgb, out_w, 3, 3), tl), "{label}: top left");
            assert!(
                close(pixel(&rgb, out_w, right, 3), tr),
                "{label}: top right"
            );
            assert!(
                close(pixel(&rgb, out_w, 3, bottom), bl),
                "{label}: bottom left"
            );
            assert!(
                close(pixel(&rgb, out_w, right, bottom), br),
                "{label}: bottom right"
            );
        }
    }
}

/// A broken EXIF is no reason to lose the avatar: the picture stays as stored.
#[test]
fn a_broken_orientation_leaves_the_image_as_stored() {
    let source = images::quadrant_jpeg(64, 32);
    let broken: [&[u8]; 5] = [
        b"",
        b"II*\0",
        b"II*\0\xFF\xFF\xFF\xFF",
        b"XX*\0\x08\0\0\0",
        b"II*\0\x08\0\0\0\xFF\xFF",
    ];
    for payload in broken {
        let output = thumbnail(&images::with_raw_exif(&source, payload)).expect("a valid JPEG");
        let (w, h, rgb) = read_jpeg(&output.bytes);
        assert_eq!((w, h), (64, 32));
        assert!(close(pixel(&rgb, w, 3, 3), RED));
    }
    for value in [0, 9, 0xFFFF] {
        let output =
            thumbnail(&images::with_orientation(&source, value, false)).expect("a valid JPEG");
        assert_eq!(
            read_jpeg(&output.bytes).0,
            64,
            "orientation {value} is no orientation"
        );
    }
}

#[test]
fn what_is_no_image_is_refused() {
    let inputs: [(&str, &[u8]); 5] = [
        ("nothing", b""),
        (
            "svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
        ),
        (
            "html",
            b"<!DOCTYPE html><html><script>alert(1)</script></html>",
        ),
        (
            "a JPEG's magic and then HTML",
            b"\xFF\xD8\xFF\xE0<html><script>alert(1)</script></html>",
        ),
        ("bmp", b"BM\0\0\0\0\0\0\0\0"),
    ];
    for (label, input) in inputs {
        let result = thumbnail(input);
        assert!(
            matches!(result, Err(KupferdrahtError::InvalidInput(_))),
            "{label}: {result:?}"
        );
    }
}

#[test]
fn a_file_over_the_cap_is_refused() {
    let mut input = images::flat_jpeg(8, 8);
    input.resize(AVATAR_MAX_INPUT_BYTES + 1, 0);

    let result = thumbnail(&input);

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "{result:?}"
    );
}

/// Refused from the header: a 65 535 × 65 535 RGBA image would ask for 17 GiB.
#[test]
fn declared_dimensions_over_the_limit_are_refused_before_allocating() {
    let jpeg = images::flat_jpeg(8, 8);
    let inputs = [
        ("jpeg", images::jpeg_declaring(&jpeg, 65_535, 65_535)),
        ("jpeg, wide", images::jpeg_declaring(&jpeg, 4097, 8)),
        ("png", images::png_declaring(65_535, 65_535)),
        ("png, tall", images::png_declaring(8, 4097)),
        ("gif", images::gif_declaring(65_535, 65_535)),
        ("webp", images::webp_declaring(16_384, 16_384)),
    ];
    for (label, input) in inputs {
        let result = thumbnail(&input);
        assert!(
            matches!(result, Err(KupferdrahtError::InvalidInput(_))),
            "{label}: {result:?}"
        );
    }
}

#[test]
fn an_opaque_image_becomes_a_jpeg_and_a_transparent_one_a_png() {
    let inputs = [
        ("opaque png", images::opaque_png(32, 32), "image/jpeg"),
        ("opaque webp", images::opaque_webp(32, 32), "image/jpeg"),
        (
            "transparent png",
            images::transparent_png(32, 32),
            "image/png",
        ),
        (
            "transparent webp",
            images::transparent_webp(32, 32),
            "image/png",
        ),
    ];
    for (label, input, mime) in inputs {
        let output = thumbnail(&input).expect(label);
        assert_eq!(output.mime_type, mime, "{label}");
        if mime == "image/png" {
            assert_eq!(read_png(&output.bytes), (32, 32, true), "{label}");
        } else {
            let (w, h, rgb) = read_jpeg(&output.bytes);
            assert_eq!((w, h), (32, 32), "{label}");
            assert!(close(pixel(&rgb, w, 16, 16), RED), "{label}: the picture");
        }
    }
}

#[test]
fn of_an_animated_gif_the_first_frame_is_taken() {
    let input = images::animated_gif(32, 32, RED, BLUE);

    let output = thumbnail(&input).expect("a GIF");

    assert_eq!(output.mime_type, "image/jpeg");
    let (w, _, rgb) = read_jpeg(&output.bytes);
    assert!(
        close(pixel(&rgb, w, 16, 16), RED),
        "the first frame, not the second"
    );
}

#[test]
fn an_image_over_its_target_size_is_scaled_down_and_keeps_its_ratio() {
    let wide = images::flat_jpeg(600, 300);
    let (w, h, _) = read_jpeg(&thumbnail(&wide).expect("wide").bytes);
    assert_eq!(
        (w, h),
        (280, 140),
        "the longer side to the thumbnail's target"
    );

    let (w, h, _) = read_jpeg(&rewrite_image(&wide, AvatarSize::Full).expect("full").bytes);
    assert_eq!(
        (w, h),
        (600, 300),
        "under the full size's target it keeps its size"
    );

    let large = images::flat_jpeg(300, 2000);
    let (w, h, _) = read_jpeg(&rewrite_image(&large, AvatarSize::Full).expect("tall").bytes);
    assert_eq!(
        (w, h),
        (217, 1448),
        "the longer side to the full size's target"
    );
}

#[test]
fn scaling_a_transparent_image_keeps_its_transparency() {
    let input = images::transparent_png(600, 600);

    let output = thumbnail(&input).expect("a PNG");

    assert_eq!(output.mime_type, "image/png");
    assert_eq!(read_png(&output.bytes).0, 280);
}

/// A panic ends the wasm instance, so a crafted avatar must not cause one: cut
/// and flipped bytes of each format come back as an image or an error.
#[test]
fn malformed_images_do_not_panic() {
    let seeds = [
        images::quadrant_jpeg(32, 32),
        images::with_orientation(&images::quadrant_jpeg(32, 32), 6, false),
        images::transparent_png(16, 16),
        images::animated_gif(16, 16, RED, GREEN),
        images::transparent_webp(16, 16),
    ];
    let mut state = 0x9E37_79B9_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as usize
    };
    for seed in &seeds {
        for cut in 0..seed.len().min(64) {
            let _ = thumbnail(&seed[..cut]);
        }
        for _ in 0..300 {
            let mut input = seed.clone();
            for _ in 0..1 + next() % 8 {
                let at = next() % input.len();
                input[at] = (next() & 0xFF) as u8;
            }
            let _ = thumbnail(&input);
        }
    }
}
