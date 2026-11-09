// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 144 — the image rewrite in the browser (TDD red): the wasm build has
//! no SIMD paths at all, and the decoders must hold there as natively. The
//! download and the views need a logged-in client the browser suite has not;
//! they are pinned natively (`step_144_avatars`).
//!
//! Run: `scripts/wasm-test.sh` (headless Firefox).
#![cfg(target_arch = "wasm32")]

use kupferdraht::AvatarSize;
use kupferdraht::avatar::rewrite_image;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

#[path = "common/images.rs"]
mod images;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn each_format_comes_out_as_a_written_image() {
    let inputs = [
        (images::flat_jpeg(64, 64), "image/jpeg"),
        (images::opaque_png(32, 32), "image/jpeg"),
        (images::transparent_png(32, 32), "image/png"),
        (
            images::animated_gif(32, 32, images::RED, images::BLUE),
            "image/jpeg",
        ),
        (images::opaque_webp(32, 32), "image/jpeg"),
        (images::transparent_webp(32, 32), "image/png"),
    ];
    for (input, mime) in inputs {
        let output = rewrite_image(&input, AvatarSize::Thumbnail).expect("an image");
        assert_eq!(output.mime_type, mime);
        let width = if mime == "image/png" {
            images::read_png(&output.bytes).0
        } else {
            images::read_jpeg(&output.bytes).0
        };
        assert!(width > 0, "the output reads back");
    }
}

#[wasm_bindgen_test]
fn the_exif_orientation_is_applied() {
    let input = images::with_orientation(&images::quadrant_jpeg(64, 32), 6, false);

    let output = rewrite_image(&input, AvatarSize::Thumbnail).expect("a JPEG");

    let (w, h, rgb) = images::read_jpeg(&output.bytes);
    assert_eq!((w, h), (32, 64));
    assert!(images::close(images::pixel(&rgb, w, 3, 3), images::BLUE));
}

#[wasm_bindgen_test]
fn an_image_over_its_target_size_is_scaled_down() {
    let output =
        rewrite_image(&images::flat_jpeg(600, 300), AvatarSize::Thumbnail).expect("a JPEG");

    let (w, h, _) = images::read_jpeg(&output.bytes);
    assert_eq!((w, h), (280, 140));
}

#[wasm_bindgen_test]
fn what_is_no_image_or_over_the_limits_is_refused() {
    let jpeg = images::flat_jpeg(8, 8);
    let inputs = [
        b"<svg><script>alert(1)</script></svg>".to_vec(),
        images::jpeg_declaring(&jpeg, 65_535, 65_535),
        images::png_declaring(65_535, 65_535),
        images::gif_declaring(65_535, 65_535),
    ];
    for input in inputs {
        assert!(rewrite_image(&input, AvatarSize::Thumbnail).is_err());
    }
}
