# Step 36 — Streaming file send

> **Status:** MERGED to `main` — `9877c27`, PR #31 (squash-merge, no
> separate merge commit), 2026-06-18. Scope: streaming (callback-driven)
> `send_file` + a fix to the path variant's encrypt-staging directory so it
> also works on Android; surfaced by the natrium-ion file-send analysis.
> No dated live-smoke record in this file — its own TDD section lists unit
> tests only.
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 36" — current.
>
> **Branch / commit prefix:** `step/36-*`, `feat(step-36): …`.

---

## Step 36 — Streaming file send

Add `Timeline::send_file_stream(source, filename, mime)`: send a file from a
foreign-supplied byte stream instead of a filesystem path. Keep the existing
path-based `send_file`, but fix its encrypt-staging directory so it works on
Android too.

### Why

`send_file` takes a `&Path`. On Android the document/photo picker yields a
`content://` URI, not a filesystem path, so the app must first copy the stream
to a temp file just to obtain a path. A streaming `AssetSource` callback lets
the app feed bytes directly — no temp copy — and keeps Kupferdraht's chunked,
memory-bounded encrypt pipeline (plaintext never fully in RAM).

Separately, the path variant currently stages the encrypted tempfile under
`dirs::cache_dir()` (`asset_send.rs`), which returns `None` on Android → the
send fails with `Environment("no OS cache directory available")`. The receive
path already uses the client's `base_dir`-derived dir; send should too.

### Why a tempfile is still needed (not pure single-pass)

Wire's `/v14/assets` upload requires a `Content-MD5` header computed over the
**ciphertext**, so Kupferdraht must produce the full ciphertext before the upload
starts. The ciphertext is therefore staged (a tempfile on disk — memory-safe).
Streaming removes the *plaintext* temp copy and keeps plaintext out of RAM; the
ciphertext tempfile stays. That tempfile is what the `base_dir` fix is about.

### Public API

```rust
/// Foreign-implemented byte source for streaming uploads.
#[uniffi::export(with_foreign)]
pub trait AssetSource: Send + Sync {
    /// Read up to `max_len` bytes. Return an empty Vec at EOF. The same
    /// `max_len` Kupferdraht uses for its encrypt chunking (STREAM_CHUNK, 64 KiB).
    fn read_chunk(&self, max_len: u32) -> Result<Vec<u8>, KupferdrahtError>;
}

impl Timeline {
    pub async fn send_file_stream(
        &self,
        source: Arc<dyn AssetSource>,
        filename: String,
        mime_type: String,
    ) -> Result<MessageHandle, KupferdrahtError>;
}
```

Path-based `Timeline::send_file(path, mime)` is unchanged.

### Implementation

- `encryption/asset_crypto.rs`: extract `encrypt_reader<R: Read>(plaintext, dir)`
  from `encrypt_file`; the plaintext byte count comes from a `CountingReader`
  wrapper (so the stream source needs no `size()` — the count feeds
  `EncryptedAsset.plaintext_size`). `encrypt_file` becomes
  `encrypt_reader(BufReader(File::open(path)), dir)`.
- `timeline/asset_send.rs`:
  - `AssetSource` trait + an `AssetSourceReader: std::io::Read` adapter that
    pulls `STREAM_CHUNK`-sized chunks from the callback (empty chunk = EOF).
  - `Client::asset_staging_dir()` → `base/accounts/<id>/assets/staging` (from
    `KupferdrahtHome`), falling back to `dirs::cache_dir()` only for the no-`base_dir`
    in-memory test client. Replaces `ciphertext_tempfile_dir()`; both send paths
    use it. Bonus: same filesystem as the final asset cache, so the
    tempfile→cache move is a cheap rename.
  - Extract `finish_asset_send(room_id, group_id, encrypted, mime, filename)`
    (md5 → upload → proto → MLS send → persist → emit) shared by both entry
    points; `upload_asset_blob` takes `filename: Option<&str>` instead of a path.
  - `send_file_stream`: policy-gate → resolve group → `encrypt_reader(source)`
    → `finish_asset_send(..., Some(filename))`.
- `ffi/timeline.rs`: `Timeline::send_file_stream` forwards to the core.

Chunking matches Kupferdraht's existing encrypt streaming (`STREAM_CHUNK = 64 KiB`).
`read_chunk` is a synchronous foreign call made during the (already synchronous)
encrypt step, consistent with the current path-based send.

### TDD

`encryption/asset_crypto.rs`:
- `encrypt_reader` over the same bytes yields the same `plaintext_size` /
  `ciphertext_size` / round-trips to the same plaintext as `encrypt_file`
- empty input encrypts (IV-only blob) and decrypts back to empty

`timeline/asset_send.rs`:
- `AssetSourceReader` reassembles chunked reads and stops at the empty-chunk EOF
- a source that errors mid-stream surfaces the error (no partial send)
