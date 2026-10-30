// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

use std::io::Result;

fn main() -> Result<()> {
    // Step 20a: UniFFI uses proc-macros (`setup_scaffolding!` in lib.rs),
    // so there is no UDL scaffolding step here — the FFI surface is
    // generated from `#[uniffi::export]` / `#[derive(uniffi::…)]`.

    // Step 4: compile Wire's GenericMessage proto subset into Rust.
    // The generated module lands in `OUT_DIR/kupferdraht.messages.rs` and
    // is `include!`'d from `src/wire/proto.rs`.
    //
    // `protoc-bin-vendored` ships a `protoc` binary so the build
    // works on Android / iOS / CI hosts without a system install
    // (CLAUDE.md: "protoc is bundled so no system dependency").
    println!("cargo:rerun-if-changed=proto/generic_message.proto");
    let protoc = protoc_bin_vendored::protoc_bin_path()
        .expect("vendored protoc binary must exist for the host triple");
    prost_build::Config::new()
        .protoc_executable(protoc)
        .compile_protos(&["proto/generic_message.proto"], &["proto/"])?;
    Ok(())
}
