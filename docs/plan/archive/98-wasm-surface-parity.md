# Step 98 — wasm: surface parity with the UniFFI binding

**Status: MERGED 2026-09-29** — Sven Braune, requested by the natrium web
SDK (gaps G2–G10). `90f3741`, `fe09e1c`, `f89dfb3` (98a–98d); review
fixes `470b8cc`, `a2aedb3`, `22bff32`. Merged to `main` as PR #76
(`5eea228`); CI green. Smoke 2026-09-29 through the natrium web SDK
showcase (see the end of this file). Scope shipped: session + device
admin, room admin + guest links, timeline cursor / delivery status /
reactions / knock / location, server-administered config on the wasm
surface. Numbered 98 in parallel with Step 98 (asset transfer timeouts,
PR #67, `98-asset-transfer-timeouts.md`); both keep their number, the
file names tell them apart. Archived with this header.

Four sub-steps on one branch, landing as one commit at the user's request:

| | subject | requested as |
|---|---|---|
| 98a | session + device admin | G2, G3, G4 |
| 98b | room admin + guest links | G5, G6 |
| 98c | timeline: cursor, delivery status, reactions, knock/location | G7, G8, G9 |
| 98d | server-administered config | G10 |

Deliberately **not** included: a browser WebSocket transport. That is not a
missing binding but real work, and stays `open.md` NG-14. Nor a log-level
hook (asked for as G11): the core has no logging API to bind, so it would be
new core work rather than a binding.

## Why

The wasm surface is a hand-written subset of the UniFFI one, and three
capability groups that exist in `kupferdraht-core` and in `ffi/` have no browser
binding. A browser embedder therefore cannot change a profile, list the
account's devices, or recover from Wire's seven-device cap.

The last of those is the sharp one: a user who hits the cap during login is
locked out with no way back, because `PendingLogin::devices` /
`delete_device` — the documented recovery path — stop at the wasm boundary.

Requested by the natrium web SDK (`docs/kupferdraht-wasm-gaps.md` in that repo,
gaps G2, G3, G4). Everything here is binding-only: no core change, no new
protocol work, each method delegating to the same call `ffi/` already wraps.

## 98a — surface added

`Client`:

| wasm (camelCase) | delegates to |
|---|---|
| `setDisplayName(name)` | `Client::set_display_name` (`ffi/admin.rs:114`) |
| `setHandle(handle)` | `Client::set_handle` (`:122`) |
| `updateEmail(email)` | `Client::update_email` (`:134`) |
| `devices()` | `Client::devices` (`:158`) |
| `deleteDevice(deviceId, password)` | `Client::delete_device` (`:166`) |

`PendingLogin`:

| wasm (camelCase) | delegates to |
|---|---|
| `devices()` | `PendingLogin::devices` (`ffi/manager.rs:237`) |
| `deleteDevice(deviceId)` | `PendingLogin::delete_device` (`:245`) |

`DeviceInfo` is projected into a serde view struct like the rest of the wasm
payloads (`DeviceInfoView`: `deviceId`, `label`, `model`, `deviceClass`,
`createdAtMs`), because the surface returns owned camelCase JS objects rather
than opaque handles.

## 98b — room admin + guest links

`Client.renameRoom(roomId, name)`, `deleteRoom(roomId)`,
`createJoinLink(roomId, password?)`, `revokeJoinLink(roomId)` and
`joinViaLink(code, key, domain, password?)`, delegating to the same
`Client` methods `ffi/room.rs` and `ffi/admin.rs` wrap. `JoinLink` is
projected as `{url, passwordProtected}`.

Plus `roomAccess(roomId)`, `setRoomAccess(roomId, modes, roles)` and
`allowGuests(roomId)`. Those are not decoration: a Kupferdraht-created conversation
defaults to `access=["invite"]`, and Wire answers `403 access-denied` to a code
request against it — so opening the access modes is the **prerequisite** for a
guest link, which the CLI has always said in `room-access-allow-guests`.
Binding `createJoinLink` without it produced a method that fails on every
fresh room, which is how the consumer found it.

`joinViaLink` is the one a consumer could not have worked around: redeeming a
guest link is an MLS external commit, so it has to come from Kupferdraht. Issuing
and revoking are plain REST and the natrium web SDK had bridged them; joining
it could not.

## 98c — timeline

- `messages(roomId, limit?, before?)` — the cursor `MessagesOptions` always
  had. Without it a browser could only read the newest page of a conversation.
- `TimelineItemView.deliveryStatus` — `"sending" | "submitted" | "received" |
  "failed"`, from the core's `DeliveryStatus`. A UI that cannot read it has to
  guess whether a message was sent, and guessing about delivery is worse than
  saying nothing.
- `ReactionView.isSelf` — the core aggregates `{emoji, senders}`; the view
  collapsed that to a count, which cannot express whether the local user
  reacted. The projection now takes the local user id, and degrades the flag
  to `false` rather than guessing when it is unknown.
- `sendKnock(roomId, hotKnock)` and `sendLocation(roomId, lat, lon, name?,
  zoom?)` — two of Wire's nine content variants could be received but not
  sent from a browser.

`TimelineItemView::from` became `TimelineItemView::project(item, me)`, since
the projection now needs the local user id.

## 98d — server-administered config

`Client.featureConfigs()` and `Client.fileSharing()`. `configJson` stays raw:
Wire's per-feature shapes differ and reshaping them in the binding would age
badly. `FileSharingView` keeps `allowedExtensions`, because
`FileSharingPolicy::EnabledSome` carries the allow-list and a flat boolean
would drop it.

## Phase 6 note — one open question for the gate

Phase 6 wants the CLI command that exercises the feature in the same change.
For a binding-only step that command should already exist, and for four of the
five `Client` methods it does:

| capability | CLI command |
|---|---|
| `set_display_name`, `set_handle` | `profile-set` |
| `devices` | `devices` |
| `delete_device` | `device-delete` |
| `PendingLogin` device recovery | `login --delete-device` (`cli/auth.rs`) |
| `update_email` | **none** |

`Client::update_email` exists in the core and in `ffi/admin.rs` but has no CLI
consumer, so binding it would put a capability on the browser surface that the
CLI has never exercised. Two ways out, decide at the approval gate:

1. add a small `email-update` command in this step, restoring phase 6, or
2. leave `updateEmail` out of the wasm surface until the CLI has it, and bind
   only the other four.

This step is written for (1); the test pins all five, so choosing (2) means
dropping one assertion.

## Retrospective (phase 11)

*Written after 98a; the notes below cover the whole step.*

**What 98b–d added to the picture.** Two things were nearly shipped half-done
and were caught by writing the test before the code:

- `sendKnock` / `sendLocation` went in, and only then did it become obvious
  that the *receive* path drops the data: `TimelineItemView` flattened both
  variants to a bare kind label, so a browser could send a location and never
  read one back. Fixed in the same sub-step (`hotKnock`, `location`), but it
  is a reminder that "bind the sender" is not the same as "support the
  feature".
- `ReactionView` could not gain `isSelf` without knowing who "self" is, which
  turned `TimelineItemView::from` into
  `TimelineItemView::project(item, me)`. Where the local id is unknown the
  flag degrades to `false` rather than guessing — the same rule the rest of
  this surface follows.

**The same mistake, twice.** Both slips in this step were of one kind:
binding the obvious method and missing what it depends on.

- `sendKnock` / `sendLocation` went in while the *receive* projection still
  flattened both variants away — send without read.
- `createJoinLink` went in without `allowGuests`, so it returned
  `403 access-denied` on every conversation Kupferdraht itself had created. Caught
  only when a human pressed the button; no test would have found it, because
  every test in this step asks "is the method bound?" and none asks "does it
  work on a fresh room?".

The lesson is not "write more surface tests" — they were green throughout. It
is that a binding step needs one question per capability that the source
cannot answer: *what does this call need to already be true?* For guest links
the CLI's own help text had the answer.

**Scope held.** G1 (browser WebSocket) and G11 (log level) were requested by
the consumer but are not binding gaps — one is transport work, the other has
no core API to bind — and both stayed out. The natrium web SDK covers G1 with
its own socket rather than waiting.

**What went well.** The step was genuinely binding-only: every method is one
delegation, no core change, and the existing `ffi/` wrappers told us the exact
signatures. The Step-94 parity test earned its keep — it named all seven
missing `kupferdraht-tabs` mirrors the moment the Rust side compiled, with no
thinking required.

**What was checked rather than assumed.** The plan first claimed the CLI
already exercised every capability. It does not: `Client::update_email` had no
command, so this step adds `email-update`. Had we not looked, we would have
put a capability on the browser surface that the CLI has never driven —
exactly what phase 6 exists to prevent.

**What surprised us.** `DeviceInfo` carries `class`, which needs no rename for
JS (a legal object key) but reads oddly next to `deviceId`. We kept the core
name rather than inventing `deviceClass`, so the browser sees the same record
as Kotlin and Swift. Worth a second opinion.

Also: the core already marks the current device via `is_self`, which means a
browser consumer no longer needs the workaround the natrium web SDK had grown
— matching `ownDeviceThumbprint()` against `roomDevices()` leaves — just to
find out which device it is.

**Debt / open.**

- `DeviceInfoView` duplicates `DeviceInfo` field for field. The alternative is
  serde derives on the core record (as some FFI-friendly types already do),
  which would delete this struct. Deliberately not done here: it touches a
  core type, and this step is binding-only.
- `Client.deleteDevice` takes the password as a plain `String` across the JS
  boundary. That matches the native surface, but on the web the value sits in
  a JS string the embedder owns; the zeroization the core applies internally
  cannot reach it. Not new to this step — it is the shape of every password
  argument on the wasm surface — but it should be written down somewhere.

**Gate note — not green on this machine, for reasons outside the step.**
`cargo clippy` fails with five errors in `storage/engine.rs`,
`timeline/ephemeral.rs` and `encryption/atomicity_tests.rs` — files this step
never touches. Verified pre-existing by stashing the change and re-running: the
same five fail on `main`. The cause is a newer toolchain (cargo 1.95) whose
`doc_markdown` and `Duration`-readability lints did not exist when those lines
were written. Clippy reports **nothing** in this step's files. Fixing the other
three is deliberately left out: drive-by edits to unrelated files in a shared
repo, mixed into an unrelated commit, are worse than the red.

The other four gates are green: `fmt` clean, 0 build warnings,
`cargo test --workspace --no-fail-fast` = 93 suites / 965 tests / 0 failures,
`cargo deny check` ok.

**Smoke (phase 9).** Live against the Schwarz dev backend through the natrium
web SDK and its showcase, in headless Firefox: `setDisplayName` round-trips
("Display name updated"), `devices()` lists the account's eight devices with
the current one flagged from `is_self`. The web SDK dropped its REST stand-ins
for these in the same session.
