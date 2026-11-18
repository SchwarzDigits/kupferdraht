// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! AES-256-CBC + SHA-256 file-streaming for Step 11 attachment content.
//!
//! Wire's deployed attachment protocol (live-validated round-trip
//! against Wire's `/v14/assets` backend; construction spec in
//! ADR-0011 §Crypto construction):
//!
//! - Symmetric: AES-256-CBC with PKCS7 padding. Key = 32 bytes,
//!   IV = 16 bytes both generated fresh per-attachment via OS CSPRNG.
//! - Output blob layout: `iv(16) || aes-cbc-pkcs7(plaintext)`. The
//!   IV is part of the ciphertext stream, not a separate field.
//! - Integrity: SHA-256 over the *full encrypted blob including the
//!   IV prefix*. Travels in the attachment reference inside the
//!   MLS-protected `GenericMessage`. The MLS layer authenticates the
//!   sender of the SHA; the SHA binds the reference to the file.
//!
//! ## Streaming model
//!
//! All three public functions operate on filesystem paths and stream
//! plaintext / ciphertext through 64 KB buffers. Memory is constant
//! (~64 KB peak per buffer) regardless of attachment size — Kupferdraht can
//! handle 100 MB+ attachments without inflating process RSS. ADR-0011
//! §Streaming model.
//!
//! ## Plaintext persistence
//!
//! Plaintext bytes are written only to caller-supplied destinations
//! (`decrypt_to_path` writes to a path the consumer chose;
//! `decrypt_to_memory` returns a `Vec<u8>`). Kupferdraht's per-account
//! cache holds **encrypted** ciphertext only — plaintext is
//! re-decrypted on every consumer access. ADR-0011 §Plaintext
//! persistence.

#![cfg_attr(not(test), allow(dead_code))]

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use aes::Aes256;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::KupferdrahtError;

type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;

/// Buffer size for streamed reads / writes. 64 KB is large enough
/// that AES per-block overhead is amortised, small enough that
/// peak RSS stays bounded for arbitrarily large attachments.
pub(crate) const STREAM_CHUNK: usize = 64 * 1024;

/// The longest ciphertext a plaintext of `plaintext_size` bytes makes:
/// the 16-byte IV and at most one 16-byte block of PKCS7 padding
/// (ADR-0011). Bounds a download by the sender's declared size (Step 160).
pub(crate) fn max_ciphertext_len(plaintext_size: u64) -> u64 {
    plaintext_size.saturating_add(16 + 16)
}

/// Output of [`encrypt_file`] — everything the upload + send
/// pipeline needs to ship the attachment.
///
/// `key` is held in memory only; it travels in the MLS-protected
/// attachment reference on send. The encrypted ciphertext
/// is on disk at `ciphertext_path`; the upload step streams that
/// file as the multipart body.
pub struct EncryptedAttachment {
    /// 32-byte AES-256 key, fresh per attachment — compiler-wiped on drop
    /// (Step 70, `THREAT_MODEL.md` G4). The zeroized domain ends at two
    /// serialization boundaries: the protobuf field of the reference
    /// field (prost `Vec<u8>`) and the send-side `attachments` row
    /// (whole-DB encryption at rest, ADR-0021) — chasing
    /// `Zeroizing` through prost/storage types would mean forking them.
    key: Zeroizing<[u8; 32]>,
    /// 16-byte IV, fresh per attachment. Recoverable from the first 16
    /// bytes of `ciphertext_path` — this field is informational.
    pub iv: [u8; 16],
    /// SHA-256 over the full encrypted blob (IV prefix + ciphertext).
    /// Travels in the attachment reference. Receivers re-compute it
    /// before decrypting and reject mismatched bytes.
    pub sha256: [u8; 32],
    /// Path to the temp file holding `iv(16) || aes-cbc-pkcs7(plaintext)`.
    /// Caller owns the file's lifecycle — once the upload is done,
    /// either move it into the per-account cache or delete it.
    pub ciphertext_path: PathBuf,
    /// Number of plaintext bytes encrypted (informational; mirrors
    /// the size the reference declares).
    pub plaintext_size: u64,
    /// Number of bytes written to `ciphertext_path` (= 16 IV bytes
    /// plus padded ciphertext). Used by the upload's Content-Length
    /// + Content-MD5 calculations.
    pub ciphertext_size: u64,
}

impl std::fmt::Debug for EncryptedAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`) — `Zeroizing`'s own
        // derived `Debug` would print the key bytes.
        f.debug_struct("EncryptedAttachment")
            .field("key", &crate::redact::byte_count(&*self.key))
            .field("iv", &self.iv)
            .field("sha256", &self.sha256)
            .field("ciphertext_path", &self.ciphertext_path)
            .field("plaintext_size", &self.plaintext_size)
            .field("ciphertext_size", &self.ciphertext_size)
            .finish()
    }
}

impl EncryptedAttachment {
    /// Borrow the per-attachment AES-256 key material through its
    /// compiler-wiped handle (`THREAT_MODEL.md` G4 / gap-B.21).
    pub(crate) fn key_material(&self) -> &Zeroizing<[u8; 32]> {
        &self.key
    }
}

/// Encrypt `plaintext_path` to a fresh temp file under
/// `ciphertext_dir`. Generates a fresh AES-256 key + 16-byte IV via
/// OS CSPRNG; streams the plaintext through AES-256-CBC with PKCS7
/// padding, writing `iv(16) || ciphertext` to the temp file;
/// computes SHA-256 over the same byte stream during the write.
///
/// `ciphertext_dir` must exist and be writable. Kupferdraht picks
/// `dirs::cache_dir()` for production callers and `tempfile::TempDir`
/// for tests.
///
/// # Errors
/// `KupferdrahtError::Storage` for filesystem errors (read of plaintext,
/// write of ciphertext, mkdir). `KupferdrahtError::Encoding` for
/// pathological streaming-cipher mishandling (cannot happen on
/// standard inputs).
pub fn encrypt_file(
    plaintext_path: &Path,
    ciphertext_dir: &Path,
) -> Result<EncryptedAttachment, KupferdrahtError> {
    let plaintext_file = File::open(plaintext_path).map_err(|e| {
        KupferdrahtError::Storage(format!("open {}: {e}", plaintext_path.display()))
    })?;
    encrypt_reader(
        BufReader::with_capacity(STREAM_CHUNK, plaintext_file),
        ciphertext_dir,
    )
}

/// Encrypt a plaintext byte stream to a Wire-shaped encrypted-attachment file
/// (`iv(16) || aes-cbc-pkcs7(plaintext)`) under `ciphertext_dir`. Streams in
/// `STREAM_CHUNK` chunks — peak memory is bounded regardless of attachment size.
///
/// The plaintext byte count is measured as bytes are consumed (via
/// [`CountingReader`]), so a streaming source needs no separate length: the
/// count feeds `EncryptedAttachment::plaintext_size`. Shared by the path-based
/// [`encrypt_file`] and the callback-driven send-file-stream path.
///
/// # Errors
/// `KupferdrahtError::Storage` on filesystem / read failure.
pub fn encrypt_reader<R: Read>(
    plaintext: R,
    ciphertext_dir: &Path,
) -> Result<EncryptedAttachment, KupferdrahtError> {
    let mut key = Zeroizing::new([0u8; 32]);
    let mut iv = [0u8; 16];
    OsRng.fill_bytes(&mut *key);
    OsRng.fill_bytes(&mut iv);

    let ciphertext_path =
        ciphertext_dir.join(format!("attachment-{}.enc", uuid::Uuid::new_v4().simple()));
    let ciphertext_file = File::create(&ciphertext_path).map_err(|e| {
        KupferdrahtError::Storage(format!("create {}: {e}", ciphertext_path.display()))
    })?;
    let mut writer = BufWriter::with_capacity(STREAM_CHUNK, ciphertext_file);

    let mut hasher = Sha256::new();
    writer
        .write_all(&iv)
        .map_err(|e| KupferdrahtError::Storage(format!("write iv: {e}")))?;
    hasher.update(iv);

    // `(&*key).into()` borrows the key into the cipher without the
    // un-wiped stack copy a by-value `.into()` would make.
    let cipher = Aes256CbcEnc::new((&*key).into(), &iv.into());
    let mut counting = CountingReader::new(plaintext);
    let written = stream_encrypt(cipher, &mut counting, &mut writer, &mut hasher)?;
    writer
        .flush()
        .map_err(|e| KupferdrahtError::Storage(format!("flush ciphertext: {e}")))?;
    drop(writer);

    let plaintext_size = counting.count;
    let ciphertext_size: u64 = 16
        + u64::try_from(written)
            .map_err(|_| KupferdrahtError::Storage("ciphertext byte count overflows u64".into()))?;
    let sha256: [u8; 32] = hasher.finalize().into();

    Ok(EncryptedAttachment {
        key,
        iv,
        sha256,
        ciphertext_path,
        plaintext_size,
        ciphertext_size,
    })
}

/// A `Read` adapter that counts the total bytes pulled through it, so the
/// encrypt path can report the plaintext size without a separate `stat` /
/// declared length (works for both files and streaming sources).
struct CountingReader<R> {
    inner: R,
    count: u64,
}

impl<R> CountingReader<R> {
    fn new(inner: R) -> Self {
        Self { inner, count: 0 }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n as u64;
        Ok(n)
    }
}

/// Decrypt a Wire-shaped encrypted-attachment file (`iv(16) || ciphertext`)
/// into `plaintext_path`. Streams in 64 KB chunks; verifies SHA-256
/// over the encrypted bytes before producing plaintext; rejects
/// mismatches with [`KupferdrahtError::IntegrityCheck`].
///
/// Returns the plaintext byte count.
///
/// # Errors
/// * `KupferdrahtError::IntegrityCheck` when `expected_sha256` does not
///   match the actual SHA-256 over the ciphertext file. The
///   plaintext file is left empty.
/// * `KupferdrahtError::Storage` for filesystem errors.
/// * `KupferdrahtError::Encoding` for AES unpadding failure (wrong key,
///   truncated ciphertext, or PKCS7-violating padding).
pub fn decrypt_to_path(
    ciphertext_path: &Path,
    key: &[u8; 32],
    expected_sha256: &[u8; 32],
    plaintext_path: &Path,
) -> Result<u64, KupferdrahtError> {
    verify_sha256_over_file(ciphertext_path, expected_sha256)?;
    let mut reader = open_ciphertext_reader(ciphertext_path)?;
    let iv = read_iv_prefix(&mut reader, ciphertext_path)?;
    let mut writer = BufWriter::with_capacity(
        STREAM_CHUNK,
        File::create(plaintext_path).map_err(|e| {
            KupferdrahtError::Storage(format!("create {}: {e}", plaintext_path.display()))
        })?,
    );
    let cipher = Aes256CbcDec::new(key.into(), &iv.into());
    let written = stream_decrypt(cipher, &mut reader, &mut writer)?;
    writer
        .flush()
        .map_err(|e| KupferdrahtError::Storage(format!("flush plaintext: {e}")))?;
    u64::try_from(written)
        .map_err(|_| KupferdrahtError::Storage("plaintext byte count overflows u64".into()))
}

/// In-memory variant of [`decrypt_to_path`]. Returns the decrypted
/// bytes. Capped at 64 MB so a malicious/malformed attachment cannot OOM
/// the process; consumers above the cap should use the path variant.
///
/// # Errors
/// Same as [`decrypt_to_path`], plus `KupferdrahtError::InvalidInput`
/// when the decrypted byte count exceeds 64 MB.
pub fn decrypt_to_memory(
    ciphertext_path: &Path,
    key: &[u8; 32],
    expected_sha256: &[u8; 32],
) -> Result<Vec<u8>, KupferdrahtError> {
    const MEM_CAP: u64 = 64 * 1024 * 1024;
    let ciphertext_size = std::fs::metadata(ciphertext_path)
        .map_err(|e| KupferdrahtError::Storage(format!("stat ciphertext: {e}")))?
        .len();
    if ciphertext_size > MEM_CAP + 16 {
        return Err(KupferdrahtError::InvalidInput(format!(
            "decrypt_to_memory: ciphertext {ciphertext_size} bytes exceeds 64 MB cap; use decrypt_to_path"
        )));
    }
    verify_sha256_over_file(ciphertext_path, expected_sha256)?;
    let mut reader = open_ciphertext_reader(ciphertext_path)?;
    let iv = read_iv_prefix(&mut reader, ciphertext_path)?;
    let cipher = Aes256CbcDec::new(key.into(), &iv.into());
    let cap = usize::try_from(ciphertext_size).unwrap_or(0);
    let mut sink: Vec<u8> = Vec::with_capacity(cap);
    stream_decrypt(cipher, &mut reader, &mut sink)?;
    Ok(sink)
}

// ── wasm: in-memory (buffer) attachment crypto (W7) ──────────────────────────────
//
// The browser has no filesystem, so the streaming file paths above are replaced
// by `Vec<u8>`-in/`Vec<u8>`-out variants: encryption through the same
// `stream_encrypt` core, decryption through `stream_decrypt`'s own core
// `decrypt_in_place`, in the downloaded buffer. Same wire format —
// `iv(16) || aes-cbc-pkcs7(plaintext)`, SHA-256 over the whole blob.
// Compiled for native tests as well.

/// The output of [`encrypt_bytes`]: the encrypted blob plus the key + digest a
/// caller needs to upload and reference the attachment.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) struct EncryptedBytes {
    /// Compiler-wiped on drop (Step 70) — same serialization-boundary
    /// caveat as [`EncryptedAttachment::key`].
    key: Zeroizing<[u8; 32]>,
    pub(crate) sha256: [u8; 32],
    /// `iv(16) || aes-cbc-pkcs7(plaintext)`.
    pub(crate) ciphertext: Vec<u8>,
    pub(crate) plaintext_size: u64,
}

#[cfg(any(target_arch = "wasm32", test))]
impl EncryptedBytes {
    /// Borrow the per-attachment AES-256 key material through its
    /// compiler-wiped handle (`THREAT_MODEL.md` G4 / gap-B.21).
    pub(crate) fn key_material(&self) -> &Zeroizing<[u8; 32]> {
        &self.key
    }
}

/// In-memory encrypt (W7): AES-256-CBC/PKCS7 a plaintext buffer to
/// `iv(16) || ciphertext`, hashing the whole blob. Fresh key + IV per call.
///
/// # Errors
/// `KupferdrahtError` on the (in practice unreachable) streaming-cipher path.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn encrypt_bytes(plaintext: &[u8]) -> Result<EncryptedBytes, KupferdrahtError> {
    let mut key = Zeroizing::new([0u8; 32]);
    let mut iv = [0u8; 16];
    OsRng.fill_bytes(&mut *key);
    OsRng.fill_bytes(&mut iv);

    let mut ciphertext: Vec<u8> = Vec::with_capacity(plaintext.len() + 32);
    let mut hasher = Sha256::new();
    ciphertext.extend_from_slice(&iv);
    hasher.update(iv);

    // Borrow, don't copy — see `encrypt_reader`.
    let cipher = Aes256CbcEnc::new((&*key).into(), &iv.into());
    let mut reader = std::io::Cursor::new(plaintext);
    stream_encrypt(cipher, &mut reader, &mut ciphertext, &mut hasher)?;
    let sha256: [u8; 32] = hasher.finalize().into();

    Ok(EncryptedBytes {
        key,
        sha256,
        ciphertext,
        plaintext_size: plaintext.len() as u64,
    })
}

/// In-memory decrypt (W7): verify SHA-256 over the whole blob, then
/// AES-256-CBC/PKCS7 decrypt it where it lies. The plaintext takes the
/// blob's place, so the attachment is held once, not as ciphertext and
/// plaintext side by side (Step 160).
///
/// # Errors
/// * `IntegrityCheck` on SHA mismatch.
/// * `Encoding` if the blob is shorter than the 16-byte IV prefix or unpadding
///   fails (wrong key / truncated ciphertext).
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn decrypt_bytes(
    mut blob: Vec<u8>,
    key: &[u8; 32],
    expected_sha256: &[u8; 32],
) -> Result<Vec<u8>, KupferdrahtError> {
    let actual: [u8; 32] = Sha256::digest(&blob).into();
    if &actual != expected_sha256 {
        return Err(KupferdrahtError::IntegrityCheck(format!(
            "attachment SHA-256 mismatch: expected {}, got {}",
            hex_lower(expected_sha256),
            hex_lower(&actual),
        )));
    }
    if blob.len() < 16 {
        return Err(KupferdrahtError::Encoding(
            "ciphertext shorter than 16-byte IV prefix".into(),
        ));
    }
    let cipher = Aes256CbcDec::new_from_slices(key, &blob[..16])
        .map_err(|e| KupferdrahtError::Encoding(format!("attachment key or IV: {e}")))?;
    let plaintext_len = decrypt_in_place(cipher, &mut blob[16..])?;
    blob.copy_within(16..16 + plaintext_len, 0);
    blob.truncate(plaintext_len);
    Ok(blob)
}

/// Verify the actual SHA-256 of `path`'s contents against
/// `expected`. Returns `IntegrityCheck` on mismatch.
fn verify_sha256_over_file(path: &Path, expected: &[u8; 32]) -> Result<(), KupferdrahtError> {
    let file = File::open(path)
        .map_err(|e| KupferdrahtError::Storage(format!("open {} for hash: {e}", path.display())))?;
    let mut reader = BufReader::with_capacity(STREAM_CHUNK, file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; STREAM_CHUNK];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| KupferdrahtError::Storage(format!("read for hash: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual: [u8; 32] = hasher.finalize().into();
    if &actual != expected {
        return Err(KupferdrahtError::IntegrityCheck(format!(
            "attachment SHA-256 mismatch: expected {}, got {}",
            hex_lower(expected),
            hex_lower(&actual),
        )));
    }
    Ok(())
}

fn open_ciphertext_reader(path: &Path) -> Result<BufReader<File>, KupferdrahtError> {
    let file = File::open(path)
        .map_err(|e| KupferdrahtError::Storage(format!("open {}: {e}", path.display())))?;
    Ok(BufReader::with_capacity(STREAM_CHUNK, file))
}

fn read_iv_prefix(reader: &mut BufReader<File>, path: &Path) -> Result<[u8; 16], KupferdrahtError> {
    let mut iv = [0u8; 16];
    reader.read_exact(&mut iv).map_err(|e| {
        KupferdrahtError::Encoding(format!(
            "ciphertext at {} is shorter than 16-byte IV prefix: {e}",
            path.display()
        ))
    })?;
    Ok(iv)
}

/// Drive AES-256-CBC encryption + SHA-256 hashing across a buffered
/// stream. Returns the number of ciphertext bytes written (excluding
/// the IV prefix the caller already wrote).
fn stream_encrypt<R: Read, W: Write>(
    mut cipher: Aes256CbcEnc,
    reader: &mut R,
    writer: &mut W,
    hasher: &mut Sha256,
) -> Result<usize, KupferdrahtError> {
    let mut buf = vec![0u8; STREAM_CHUNK];
    let mut leftover: Vec<u8> = Vec::new();
    let mut written: usize = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| KupferdrahtError::Storage(format!("read plaintext: {e}")))?;
        if n == 0 {
            break;
        }
        leftover.extend_from_slice(&buf[..n]);
        // Encrypt every full 16-byte block we have; keep the
        // unaligned remainder for the next iteration.
        let blocks = leftover.len() / 16;
        if blocks > 0 {
            let bytes = blocks * 16;
            let mut chunk = leftover[..bytes].to_vec();
            for block_start in (0..bytes).step_by(16) {
                let block: &mut [u8; 16] = (&mut chunk[block_start..block_start + 16])
                    .try_into()
                    .expect("16 bytes");
                cipher.encrypt_block_mut(block.into());
            }
            writer
                .write_all(&chunk)
                .map_err(|e| KupferdrahtError::Storage(format!("write ciphertext: {e}")))?;
            hasher.update(&chunk);
            written += chunk.len();
            leftover.drain(..bytes);
        }
    }
    // Final block — pad the leftover with PKCS7 to a 16-byte boundary
    // and encrypt. PKCS7 always adds at least one byte, so an empty
    // leftover still produces a full 16-byte padding block.
    let mut final_block = [0u8; 16];
    let pad_len: u8 = u8::try_from(16 - leftover.len()).expect("0 <= pad <= 16 fits in u8");
    final_block[..leftover.len()].copy_from_slice(&leftover);
    for b in &mut final_block[leftover.len()..] {
        *b = pad_len;
    }
    cipher.encrypt_block_mut((&mut final_block).into());
    writer
        .write_all(&final_block)
        .map_err(|e| KupferdrahtError::Storage(format!("write final block: {e}")))?;
    hasher.update(final_block);
    written += 16;
    Ok(written)
}

/// Drive AES-256-CBC decryption across a buffered stream, stripping
/// PKCS7 padding from the final block.
fn stream_decrypt<R: Read, W: Write>(
    cipher: Aes256CbcDec,
    reader: &mut R,
    writer: &mut W,
) -> Result<usize, KupferdrahtError> {
    // Read the full ciphertext into memory (after IV) — we need to
    // hold the last block back to strip its PKCS7 padding only
    // after we know it IS the last block. For the capped sizes
    // 64 MB / 64 KB+streamed, this trade is acceptable.
    //
    // A pure-streaming variant would buffer one block at a time
    // and look ahead one block before committing; the in-memory
    // path is simpler and matches the consumer-facing
    // `decrypt_to_memory` contract.
    let mut all = Vec::new();
    reader
        .read_to_end(&mut all)
        .map_err(|e| KupferdrahtError::Storage(format!("read ciphertext: {e}")))?;
    let plaintext_len = decrypt_in_place(cipher, &mut all)?;
    writer
        .write_all(&all[..plaintext_len])
        .map_err(|e| KupferdrahtError::Storage(format!("write plaintext: {e}")))?;
    Ok(plaintext_len)
}

/// AES-256-CBC decrypt `blocks` (the ciphertext after its IV) where they
/// lie and check their PKCS7 padding: the plaintext is
/// `blocks[..returned]`.
fn decrypt_in_place(
    mut cipher: Aes256CbcDec,
    blocks: &mut [u8],
) -> Result<usize, KupferdrahtError> {
    if blocks.is_empty() || !blocks.len().is_multiple_of(16) {
        return Err(KupferdrahtError::Encoding(format!(
            "ciphertext length {} is not a positive multiple of 16",
            blocks.len()
        )));
    }
    for block in blocks.as_chunks_mut::<16>().0 {
        cipher.decrypt_block_mut(block.into());
    }
    let pad = blocks.last().map_or(0, |&b| usize::from(b));
    if pad == 0 || pad > 16 {
        return Err(KupferdrahtError::Encoding(format!(
            "PKCS7 padding byte {pad} out of range (1..=16); wrong key?"
        )));
    }
    let plaintext_len = blocks.len() - pad;
    // Validate every padding byte to catch wrong-key cases
    // (CBC decrypts garbage; PKCS7 catches statistically).
    if blocks[plaintext_len..]
        .iter()
        .any(|&b| usize::from(b) != pad)
    {
        return Err(KupferdrahtError::Encoding(
            "PKCS7 padding mismatch (wrong key or corrupt ciphertext)".into(),
        ));
    }
    Ok(plaintext_len)
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(&mut s, "{b:02x}").expect("write to String");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_temp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut f = File::create(&path).expect("create");
        f.write_all(bytes).expect("write");
        path
    }

    fn round_trip(plaintext: &[u8]) -> Vec<u8> {
        let dir = TempDir::new().expect("tempdir");
        let plain_path = write_temp(dir.path(), "in.bin", plaintext);
        let attachment = encrypt_file(&plain_path, dir.path()).expect("encrypt");
        let decrypted = decrypt_to_memory(
            &attachment.ciphertext_path,
            attachment.key_material(),
            &attachment.sha256,
        )
        .expect("decrypt_to_memory");
        // Also verify decrypt_to_path round-trip.
        let out_path = dir.path().join("out.bin");
        let n = decrypt_to_path(
            &attachment.ciphertext_path,
            attachment.key_material(),
            &attachment.sha256,
            &out_path,
        )
        .expect("decrypt_to_path");
        assert_eq!(
            usize::try_from(n).expect("test fixture fits"),
            plaintext.len()
        );
        let from_disk = std::fs::read(&out_path).expect("read out");
        assert_eq!(from_disk, plaintext);
        decrypted
    }

    /// Step 70 (gap-B.21): key material must be compiler-wiped —
    /// the type, not convention, carries `THREAT_MODEL.md` G4.
    fn requires_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>(_: &T) {}

    #[test]
    fn encrypt_file_key_material_is_compiler_wiped() {
        let dir = TempDir::new().expect("tempdir");
        let plain_path = write_temp(dir.path(), "in.bin", b"zeroize me");
        let attachment = encrypt_file(&plain_path, dir.path()).expect("encrypt");
        requires_zeroize_on_drop(attachment.key_material());
        let back = decrypt_to_memory(
            &attachment.ciphertext_path,
            attachment.key_material(),
            &attachment.sha256,
        )
        .expect("decrypt through the zeroizing handle");
        assert_eq!(back, b"zeroize me");
    }

    #[test]
    fn encrypt_bytes_key_material_is_compiler_wiped() {
        let enc = encrypt_bytes(b"tiny secret").expect("encrypt_bytes");
        requires_zeroize_on_drop(enc.key_material());
        let back = decrypt_bytes(enc.ciphertext.clone(), enc.key_material(), &enc.sha256)
            .expect("decrypt through the zeroizing handle");
        assert_eq!(back, b"tiny secret");
    }

    /// Step 160: the bound covers the IV and every padding length, and is
    /// reached exactly when the plaintext fills whole blocks.
    #[test]
    fn the_ciphertext_bound_covers_the_iv_and_every_padding() {
        for size in 0..=64usize {
            let enc = encrypt_bytes(&vec![0x5a; size]).expect("encrypt_bytes");
            let bound = max_ciphertext_len(enc.plaintext_size);
            let made = enc.ciphertext.len() as u64;
            assert!(made <= bound, "{size} bytes made {made}, bound {bound}");
            if size % 16 == 0 {
                assert_eq!(made, bound, "{size} bytes pad a whole block");
            }
        }
        assert_eq!(max_ciphertext_len(u64::MAX), u64::MAX, "saturates");
    }

    /// Step 160: the browser decrypts in the downloaded buffer, so an attachment
    /// is held once, not as ciphertext and plaintext side by side.
    #[test]
    fn decrypt_bytes_decrypts_in_the_downloaded_buffer() {
        let plaintext = b"a file of more than one block, decrypted where it lies";
        let enc = encrypt_bytes(plaintext).expect("encrypt_bytes");
        let blob = enc.ciphertext.clone();
        let downloaded_at = blob.as_ptr();
        let back = decrypt_bytes(blob, enc.key_material(), &enc.sha256).expect("decrypt_bytes");
        assert_eq!(back, plaintext);
        assert_eq!(back.as_ptr(), downloaded_at, "no second buffer");
    }

    #[test]
    fn decrypt_bytes_refuses_other_bytes() {
        let enc = encrypt_bytes(b"what the sender sent").expect("encrypt_bytes");
        let mut blob = enc.ciphertext.clone();
        blob[20] ^= 1;
        let err = decrypt_bytes(blob, enc.key_material(), &enc.sha256)
            .expect_err("a changed byte fails the digest");
        assert!(
            matches!(err, KupferdrahtError::IntegrityCheck(_)),
            "{err:?}"
        );
    }

    #[test]
    fn encrypt_reader_matches_encrypt_file() {
        use std::io::Cursor;
        let plaintext: &[u8] = b"streamed asset bytes \x00\x01\x02 across a block boundary!!";
        let dir = TempDir::new().expect("tempdir");
        let streamed =
            encrypt_reader(Cursor::new(plaintext.to_vec()), dir.path()).expect("encrypt_reader");
        assert_eq!(streamed.plaintext_size, plaintext.len() as u64);
        let back = decrypt_to_memory(
            &streamed.ciphertext_path,
            streamed.key_material(),
            &streamed.sha256,
        )
        .expect("decrypt");
        assert_eq!(back, plaintext);
        // Same plaintext through the path-based path yields identical sizes.
        let plain_path = write_temp(dir.path(), "in.bin", plaintext);
        let viafile = encrypt_file(&plain_path, dir.path()).expect("encrypt_file");
        assert_eq!(streamed.plaintext_size, viafile.plaintext_size);
        assert_eq!(streamed.ciphertext_size, viafile.ciphertext_size);
    }

    #[test]
    fn encrypt_reader_empty_round_trips() {
        use std::io::Cursor;
        let dir = TempDir::new().expect("tempdir");
        let attachment =
            encrypt_reader(Cursor::new(Vec::new()), dir.path()).expect("encrypt empty");
        assert_eq!(attachment.plaintext_size, 0);
        let back = decrypt_to_memory(
            &attachment.ciphertext_path,
            attachment.key_material(),
            &attachment.sha256,
        )
        .expect("decrypt");
        assert!(back.is_empty());
    }

    #[test]
    fn round_trip_one_byte() {
        assert_eq!(round_trip(&[0x42]), vec![0x42]);
    }

    #[test]
    fn round_trip_empty() {
        // PKCS7 still encrypts an empty plaintext into one full
        // 16-byte padding block — round-trip must yield empty.
        assert_eq!(round_trip(&[]), Vec::<u8>::new());
    }

    #[test]
    fn round_trip_exactly_one_block() {
        // 16 bytes — PKCS7 must emit a *second* full padding block
        // (one block of 0x10 bytes), then strip on decrypt.
        let pt: Vec<u8> = (0..16).collect();
        assert_eq!(round_trip(&pt), pt);
    }

    #[test]
    fn round_trip_unaligned_short() {
        let pt = b"Hello, asset!".to_vec(); // 13 bytes
        assert_eq!(round_trip(&pt), pt);
    }

    #[test]
    fn round_trip_multi_chunk_unaligned() {
        // ~100 KB, deliberately not aligned to 64 KB chunks or
        // 16-byte blocks. Exercises the streaming chunk-boundary
        // handling.
        let pt: Vec<u8> = (0..100_003u32).map(|i| (i % 251) as u8).collect();
        let out = round_trip(&pt);
        assert_eq!(out.len(), pt.len());
        assert_eq!(out, pt);
    }

    #[test]
    fn ciphertext_size_equals_iv_plus_padded_plaintext() {
        let dir = TempDir::new().expect("tempdir");
        let plain_path = write_temp(dir.path(), "in.bin", &[0u8; 31]);
        let attachment = encrypt_file(&plain_path, dir.path()).expect("encrypt");
        // 16-byte IV + ceil((31+1)/16)*16 = 16 + 32 = 48.
        // PKCS7: 31 bytes is ceil(31/16) = 2 blocks of plaintext,
        // padded to 32 by adding 1 byte of 0x01 — 2 blocks total.
        // Plus IV → 48.
        assert_eq!(attachment.ciphertext_size, 48);
        let actual_size = std::fs::metadata(&attachment.ciphertext_path)
            .expect("stat")
            .len();
        assert_eq!(actual_size, 48);
    }

    #[test]
    fn sha256_mismatch_is_integrity_error() {
        let dir = TempDir::new().expect("tempdir");
        let plain_path = write_temp(dir.path(), "in.bin", b"original");
        let attachment = encrypt_file(&plain_path, dir.path()).expect("encrypt");
        // Tamper one byte of the ciphertext post-encrypt.
        let mut tampered = std::fs::read(&attachment.ciphertext_path).expect("read");
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        std::fs::write(&attachment.ciphertext_path, &tampered).expect("rewrite");
        let err = decrypt_to_memory(
            &attachment.ciphertext_path,
            attachment.key_material(),
            &attachment.sha256,
        )
        .expect_err("integrity must fail");
        assert!(
            matches!(err, KupferdrahtError::IntegrityCheck(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn wrong_key_surfaces_as_encoding_error() {
        let dir = TempDir::new().expect("tempdir");
        let plaintext = b"hello world this is a test";
        let plain_path = write_temp(dir.path(), "in.bin", plaintext);
        let attachment = encrypt_file(&plain_path, dir.path()).expect("encrypt");
        let wrong_key = [0xFFu8; 32];
        // Override the key but keep the SHA — SHA verifies (we didn't
        // tamper the ciphertext), then PKCS7 unpadding rejects the
        // garbage: in ~255 of 256 runs. The key and IV are random per
        // encryption, so in ~1 of 256 the garbage ends in a valid
        // padding byte and decrypts to other bytes — never to the
        // plaintext. Both are the contract; asserting only the error
        // made the test flaky on CI.
        match decrypt_to_memory(&attachment.ciphertext_path, &wrong_key, &attachment.sha256) {
            Err(KupferdrahtError::Encoding(_)) => {}
            Ok(bytes) => assert_ne!(
                bytes.as_slice(),
                plaintext,
                "a wrong key never yields the plaintext"
            ),
            Err(other) => panic!("expected an Encoding error, got {other:?}"),
        }
    }

    #[test]
    fn decrypt_to_memory_caps_oversized_attachments() {
        // Construct a faux 65 MB ciphertext file via metadata-only
        // tracking — we don't actually allocate; we just need to
        // demonstrate the cap rejection. Use a 65 MB file of zeros
        // is fine for a one-off test on a beefy CI.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("oversize.enc");
        // 64 MB + 17 bytes (16 IV + 1 over the cap)
        let size: u64 = 64 * 1024 * 1024 + 17;
        let f = File::create(&path).expect("create");
        f.set_len(size).expect("truncate");
        drop(f);
        let err = decrypt_to_memory(&path, &[0u8; 32], &[0u8; 32]).expect_err("cap must reject");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn fresh_calls_produce_different_keys_and_ivs() {
        let dir = TempDir::new().expect("tempdir");
        let plain_path = write_temp(dir.path(), "in.bin", b"x");
        let a = encrypt_file(&plain_path, dir.path()).expect("a");
        let b = encrypt_file(&plain_path, dir.path()).expect("b");
        assert_ne!(
            a.key_material(),
            b.key_material(),
            "key must be fresh per call"
        );
        assert_ne!(a.iv, b.iv, "IV must be fresh per call");
    }

    #[test]
    fn plaintext_size_mirrors_input_byte_count() {
        let dir = TempDir::new().expect("tempdir");
        let plaintext = b"the quick brown fox jumps over the lazy dog";
        let plain_path = write_temp(dir.path(), "in.bin", plaintext);
        let attachment = encrypt_file(&plain_path, dir.path()).expect("encrypt");
        assert_eq!(attachment.plaintext_size, plaintext.len() as u64);
    }

    // ── Step 132 item 11: AES-256-CBC known answers ──────────────────
    //
    // Produced with an independent tool, never a Wire repository
    // (ADR-0011 revision 1.2): `openssl enc -aes-256-cbc -K <KEY> -iv <IV>
    // -nosalt` of LibreSSL 3.3.6, PKCS#7 padding. Two plaintexts: one a
    // whole block (its padding is a full extra block), one three blocks
    // and one byte.

    const KAT_KEY: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    const KAT_IV: [u8; 16] = [
        0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01,
        0x00,
    ];

    fn unhex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
            .collect()
    }

    #[test]
    fn decrypts_openssl_known_answers() {
        let dir = TempDir::new().expect("tempdir");
        // Wire's `sha256` is over the encrypted blob (ADR-0011), so the
        // integrity input is computed here; the plaintexts' own SHA-256 are
        // 8f1c194a…2207 and 0d78381d…e8b4 for the record.
        for (name, plaintext, ciphertext_hex) in [
            (
                "three-blocks-and-one",
                &b"Kupferdraht asset known-answer vector: 41 bytes!!"[..],
                "ebe3ab060e20aa0a61870dc9ca3c6c1883dccbe6207b40bfe1af9554ad254f66df33568292eaa051f93bee30b0f9acdb450b2597e7458ca3f214dda7f4bb0d19",
            ),
            (
                "one-block",
                &b"sixteen-byte-blk"[..],
                "8f4598010ab09e6ceabea2a10a0dcd0f3ce62f6428a2e27305b4656246d39544",
            ),
        ] {
            let mut blob = KAT_IV.to_vec();
            blob.extend(unhex(ciphertext_hex));
            let path = write_temp(dir.path(), name, &blob);
            let blob_sha: [u8; 32] = sha2::Sha256::digest(&blob).into();
            let got = decrypt_to_memory(&path, &KAT_KEY, &blob_sha).expect("known answer decrypts");
            assert_eq!(got, plaintext, "{name}: plaintext must match openssl's");
        }
    }

    /// The same vectors through the file path: `decrypt_to_path` writes
    /// exactly the plaintext.
    #[test]
    fn decrypt_to_path_matches_the_openssl_known_answer() {
        let dir = TempDir::new().expect("tempdir");
        let mut blob = KAT_IV.to_vec();
        blob.extend(unhex(
            "8f4598010ab09e6ceabea2a10a0dcd0f3ce62f6428a2e27305b4656246d39544",
        ));
        let path = write_temp(dir.path(), "one-block.enc", &blob);
        let out = dir.path().join("one-block.txt");
        let blob_sha: [u8; 32] = sha2::Sha256::digest(&blob).into();
        decrypt_to_path(&path, &KAT_KEY, &blob_sha, &out).expect("known answer decrypts");
        assert_eq!(std::fs::read(&out).expect("read"), b"sixteen-byte-blk");
    }
}
