# Step 144 — Avatars: show every user's and the team's, set the own

**Status: commissioned 2026-11-05** (from the browser embedder's
requirements for a work-queue client, R2 and R3, measured there against
`4f30f02` and re-checked against `main` `9e1a11c`). Taken with Volker on
commissioning:

- reading and setting in this one step (the embedder ranked setting
  lower);
- matrix-rust-sdk's naming — `avatar`, sizes `Thumbnail` / `Full`
  (browser `"thumbnail"` / `"full"`) instead of Wire's `preview` /
  `complete`;
- keys from the stored member list and the own profile, one
  `list-users` for a user in no room;
- an in-memory cache of 4 MiB (8 MiB the ceiling if 4 proves too small,
  never 16) and a size cap per image;
- **no foreign image bytes reach the embedder:** Kupferdraht decodes
  every avatar, downloaded or set, and writes a new image from its
  pixels — JPEG, PNG where the image is transparent; inputs JPEG, PNG,
  GIF and WebP; `upload_avatar` takes one image and makes both sizes;
- the team's icon (R3) in this step, as `team_avatar()`: the same
  pipeline and cache, its key from Step 143's team request.

After Step 143, whose V33 also touches `self_user`.

## Problem

1. **No user has a picture.** The embedder shows initials where it wants
   the own avatar (account row) and the other member's (room list,
   room header).
2. **The keys arrive and are dropped.** `POST /list-users` and
   `GET /self` carry `assets`, a list of `UserAsset { key, size:
   "preview" | "complete", type: "image" }`; `wire::dto::UserProfile`
   and `SelfUser` ignore it. `backfill_user_profiles`
   (`client/sync.rs`) already resolves every non-self member through
   `list-users` on each room-list refresh, so member keys would be as
   fresh as member names are today.
3. **The download exists, for encrypted files only.**
   `GET /v14/assets/{domain}/{key}` answers `200` or `302` (followed by
   hand since Step 141 natively, by `fetch` in the browser); avatars are
   public assets, so no `Asset-Token` and no decryption. The Swagger
   says the `200` comes as `application/octet-stream`.
4. **The own avatar cannot be set.** `PUT /self` sends only `name`
   (`update_self_profile_name`); uploads always go out `public=false`,
   `retention=persistent` (ADR-0011).
5. **An image is an attack vector for its decoder.** A crafted file
   with a valid header can corrupt the memory of the decoder that
   renders it (libwebp, CVE-2023-4863, exploited in the wild). Avatars
   are chosen freely by every user in a shared room — in the embedder's
   client, external people — and would be decoded by the browser or the
   platform the embedder runs on, with the session in reach. A file
   that is image and HTML at once, or an SVG, runs as code where it is
   opened as a document.
6. **The team's icon is not known either.** `GET /teams/{tid}` (Step
   143) carries `icon`: the asset key of the image set in the team
   settings, or `"default"` for none (Swagger `Icon`); Step 143 reads
   only `name`.

## Scope

1. **Reading.**
   - `UserProfile` and `SelfUser` read `assets`; a `UserAsset` with an
     unknown `size` or a `type` other than `image` is skipped.
   - `Client::avatar(&UserId, AvatarSize) -> Result<Option<Avatar>>`,
     `Avatar { bytes, mime_type }`, `AvatarSize::{Thumbnail, Full}`
     (`preview` / `complete` on the wire). The key comes from the own
     state for the own user, from the stored member list for a member,
     else from one `list-users` lookup; no key → `Ok(None)`.
   - The asset domain is the user's domain (`UserAsset` has none; phase
     1 confirms it live).
   - `Client::avatar_key(&UserId) -> Result<Option<String>>`, the
     version marker: the thumbnail's key, the full image's when a
     profile has only that. A method, not a getter: the own key changes
     with `upload_avatar`, and the tabs snapshot getters into the handle.
   - `RoomMember::avatar_key` (FFI record field), `members()` entries
     `avatarKey` in the browser.
   - `Client::team_avatar() -> Result<Option<Avatar>>`: the icon of the
     account's team, its key the `icon` of Step 143's team request,
     stored with the team so a resumed client has it. `None` without a
     team, for `"default"`, and when the team request failed at login.
     One size, scaled down to the full size's target if larger. As of
     the last login, like the team's name.
2. **Setting.**
   - `Client::upload_avatar(image)` and `Client::remove_avatar()`
     (matrix-sdk-ffi's names on `Client`). Kupferdraht decodes the one image,
     scales it to the thumbnail and the full size (aspect ratio kept;
     cropping is the embedder's), writes both anew (scope 3), then two
     uploads with `public=true`, `retention=eternal` and the written
     image's MIME type on the part, then `PUT /self` with `assets`
     naming both keys. `remove_avatar` sends `assets: []`.
   - The own keys live in the client's state, not in the frozen
     `Account` snapshot: `upload_avatar` / `remove_avatar` replace them
     and persist them, so `avatar_key` and `avatar` for the own user
     answer the new state at once. The replaced assets stay on the
     server (Wire's clients do not delete them either).
3. **Every image is written anew.** No byte of a downloaded or a given
   image passes through:
   - **Decode** with Rust decoders (the `image` crate with the format
     features `jpeg`, `png`, `gif`, `webp` only), under limits: 4 MiB
     per input file (a larger download fails `BackendRejected` without
     buffering it whole), 4096 × 4096 pixels, a cap on the decoder's
     allocations (decompression bombs); of an animated GIF or WebP the
     first frame. The magic bytes pick the decoder; anything else (SVG,
     HTML, …) is "no avatar" with a `warn!` on download, `InvalidInput`
     on set.
   - **Write** a new image from the pixels: JPEG, or PNG when the image
     uses transparency (an alpha below 255 anywhere). Metadata (EXIF,
     the GPS position of a phone photo among it), trailing data and any
     structure aimed at a decoder are gone by construction. A downloaded
     avatar keeps its dimensions unless it exceeds the target size of
     its kind, then it is scaled down.
   - `Avatar::mime_type` is `image/jpeg` or `image/png`, from what
     Kupferdraht wrote, never from the server or the bytes received.
4. **Cache.** In memory, per `Client`, keyed by asset key and size (a
   changed avatar has a new key, so nothing is invalidated, only
   evicted), holding the written images, bounded to 4 MiB in total,
   least recently used out first; an image over 1 MiB is served but not
   cached. No disk cache (Step 84's cache policy stays untouched); an
   embedder that wants persistence keys its own store by `avatarKey`.
5. **Storage.** Migration V34 (the next free number when the step
   starts): `avatar_thumbnail_key` and `avatar_full_key` on
   `room_members` and on `self_user`, `team_avatar_key` on `self_user`
   (Step 143's team request reads `icon` from here on); every writer of the two tables
   carries them (`replace_rooms`, the member merge of `sync_members`,
   the login persistence, the profile update), resume reads the own
   ones back.
6. **Public surface, in the same commit.** FFI: `avatar`, `avatar_key`,
   `upload_avatar`, `remove_avatar`, `team_avatar`, the `Avatar` record and the
   `AvatarSize` enum, `RoomMember.avatar_key`. wasm `Client`:
   `avatar(userId, size) → Promise<{ bytes: Uint8Array, mimeType } |
   undefined>`, `avatarKey(userId)`, `uploadAvatar(image)`,
   `removeAvatar()`, `teamAvatar()`, `avatarKey` on `members()`.
   `kupferdraht-tabs`: the five calls in `METHODS` (`avatar`,
   `avatarKey` and `teamAvatar` idempotent,
   `uploadAvatar` and `removeAvatar` mutations), `proxy.js`, `index.d.ts`.
7. **CLI.** `avatar get --user <id> [--size thumbnail|full] --out
   <file>`, `avatar get --team --out <file>`, `avatar upload <file>`,
   `avatar remove`; `room-members` prints
   each member's `avatarKey`.
8. **Docs:** `docs/library.md`, `docs/bindings.md` (with the rule for
   embedders: show an avatar as `<img>` or on a canvas, never as a
   document, `iframe` or `object`), `docs/cli.md`,
   `docs/wire/api-notes.md` (the `assets` field, the public upload, the
   live observations of phase 1), `docs/api-mapping-matrix-rust-sdk.md`
   (`upload_avatar` / `remove_avatar` Aligned with matrix-sdk-ffi's
   `Client::upload_avatar(mime_type, data)` / `remove_avatar()` — here
   without the MIME type, which Kupferdraht sniffs; `avatar` Drift to
   `Account::get_avatar` / `RoomMember::avatar` with a `MediaFormat` —
   here on `Client` by user id, `Full` for `MediaFormat::File`;
   `avatar_key` Drift to `avatar_url` / `Room::member_avatar_url` — a
   Wire asset key, no URL; `team_avatar` as Wire extension —
   matrix-rust-sdk has no team, the nearest is a space's avatar), `docs/ARCHITECTURE.md` §7 (V34) and the module that writes
   images, `docs/THREAT_MODEL.md` (minor version: the re-encoding, the
   decoders now in Kupferdraht's process, the residual risks below),
   `THIRD-PARTY-NOTICES` and `docs/THIRD_PARTY_LICENSES.html` (the
   `image` crate and its format crates), `README.md` ("What it does"),
   `CHANGELOG.md` `[Unreleased]` under Added.

## Residual risks (THREAT_MODEL)

- **The decoders run in Kupferdraht's process now.** Rust rules out
  most memory errors, but some decoders take `unsafe` paths for speed
  (SIMD); phase 1 checks the chosen crates and turns such paths off
  where a feature allows it.
- **A decoder panic.** A malformed image is far likelier to panic a
  Rust decoder than to run code. In the browser a panic ends the wasm
  instance: a crafted avatar could crash the client, not take it over.
  The limits narrow it; nothing rules it out.
- **The image written by Kupferdraht is still decoded by the platform**
  — but it comes from a known encoder and carries no crafted structure.

## Out of scope

- **Live profile updates.** Wire sends `user.update` with `assets`; a
  member's key refreshes with the next room-list refresh, the own with
  `upload_avatar` or the next login (an avatar changed on another device
  of the account shows at the next login — the embedder accepts it).
- **Images sent as files** (Step 11) still reach the embedder as the
  sender's bytes; the same decoder risk applies where an embedder
  previews them. A written preview beside the original would be a step
  of its own.
- Room avatars (Wire conversations have none; open.md's parity list).
- Deleting replaced avatar assets on the server; a disk cache;
  cropping or any editing beyond decode, scale and write.

## Phase-1 questions

1. **Live probes on the wire-server-rs stack:** a public upload as a
   team member — does the backend demand `convId` / `filename` /
   `filetype` when asset audit logging is on, as for file uploads? —,
   `PUT /self` with `assets` and with `assets: []`, the download of a
   public asset by another team member and by its owner (domain = the
   user's domain, `302` or `200`), and what `list-users` returns for
   `assets` afterwards; a team icon set by the owner (`PUT
   /teams/{tid}` with `icon` after a public upload), downloaded by a
   member, and what `icon_key` carries beside `icon`. Recorded in
   `docs/wire/api-notes.md`.
2. **Sizes and quality:** the dimensions and the JPEG quality of the
   `preview` and `complete` images Wire's own clients upload — they set
   Kupferdraht's target sizes and quality; the caps (4 MiB per input,
   4096 × 4096, 1 MiB per cache entry) stand unless an observed avatar
   exceeds them.
3. **The decoder crates:** the `image` version and its format crates,
   their `unsafe` (`cargo geiger`) and the features that avoid it, their
   fuzzing; `cargo deny` with the new licenses; the growth of the wasm
   binary and the time to decode and write a 4096 × 4096 image in the
   browser.

## TDD sketch

- httpmock, reading: a member with `assets` → `avatar_key` and both
  sizes; a member without → `None`; the own user from `/self`; a user in
  no room → one `list-users`, then the download; the `302` hop without
  credentials (Step 141's path); an answer over 4 MiB → error; the
  second call for the same key and size makes no HTTP request; an image
  over 1 MiB is fetched again.
- Writing anew, known-input tests: the output decodes and is not the
  input; a JPEG with data after its end marker and one with EXIF GPS →
  neither survives; an SVG, an HTML file and a JPEG/HTML polyglot →
  `None` on download, `InvalidInput` on set; a small file declaring
  65 535 × 65 535 pixels → refused before allocation; an opaque PNG →
  JPEG, a transparent PNG → PNG; an animated GIF → its first frame; an
  avatar larger than its target size → scaled down.
- httpmock, team: an `icon` key → the image, written anew; `"default"`
  → `None` without a download; a personal account → `None` without a
  request; a team request that failed at login → `None`; a resumed
  client keeps the key; a second call is served from the cache.
- httpmock, setting: one image → two `POST /assets` with `public=true`,
  `retention=eternal` and the written MIME type, the two sizes as
  produced, then `PUT /self` with both keys and sizes; a non-image or an
  oversize image → `InvalidInput` and no request; afterwards
  `avatar_key(self)` is the new key, and a resumed client reads it from
  the store; `remove_avatar` → `PUT /self` with `assets: []`, then
  `None`.
- Storage: V34 over a V33 store; `migration_immutability` pins it; the
  member keys survive `replace_rooms` and a member merge.
- FFI and tabs: the record fields and the four calls in the parity
  tests (`step_94_tab_proxy_parity`); wasm32 `check` and `clippy`.
- Smoke: `kupferdraht-smoke.sh --throwaway` 18/18, then `avatar upload` for
  the owner of the throwaway team with a phone photo carrying EXIF,
  `avatar get` of the owner by the member (both sizes: images, no EXIF),
  `avatar remove`; a team icon set by the owner (`curl`, no Kupferdraht
  API sets it), `avatar get --team` by the member.
