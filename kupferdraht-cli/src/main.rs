// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![deny(warnings)]
#![deny(clippy::all)]

mod backend;
mod cli;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::cli::read_password;

#[derive(Parser)]
#[command(
    name = "kupferdraht-cli",
    about = "Kupferdraht command-line client — MLS messaging against a self-hosted wire-server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum TaskCommand {
    /// Create and send a new task (status starts at `open`). Step 60a.
    Add {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Task headline (required, non-empty).
        #[arg(long)]
        title: String,
        /// Free-text description; links are plain text inside it.
        #[arg(long)]
        description: Option<String>,
        /// Qualified user id (`<uuid>@<domain>`) of the assignee.
        #[arg(long)]
        assignee: Option<String>,
        /// Due date, ISO 8601: `yyyy-mm-dd` (due by end of that local
        /// day), `yyyy-mm-ddThh:mm[:ss]` (local time), or RFC 3339
        /// with offset/`Z`.
        #[arg(long, conflicts_with = "due_ms")]
        due: Option<String>,
        /// Due date as raw Unix milliseconds (machine escape hatch).
        #[arg(long)]
        due_ms: Option<i64>,
        /// Task priority.
        #[arg(long, value_enum)]
        priority: Option<cli::task::PriorityArg>,
        /// Send the bare payload without the human-readable `Text`
        /// fallback (mode a; official Wire clients then ignore the
        /// message entirely). Without either fallback flag the default
        /// derives from the room's task-list marker (ADR-0020).
        #[arg(long, conflicts_with = "fallback")]
        no_fallback: bool,
        /// Force the human-readable `Text` fallback on (mode c) even
        /// in a marked task-list room.
        #[arg(long)]
        fallback: bool,
    },
    /// List folded tasks: per room (`--id`), or cross-room via
    /// `--mine` (assigned to me, open/in-progress) or `--delegated`
    /// (created by me, assigned to someone else). Steps 60b/60c.
    List {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long, conflicts_with_all = ["mine", "delegated"])]
        id: Option<String>,
        /// Cross-room: tasks assigned to me, still open/in-progress.
        #[arg(long, conflicts_with = "delegated")]
        mine: bool,
        /// Cross-room: tasks I created and assigned to someone else.
        #[arg(long)]
        delegated: bool,
    },
    /// Move a task in the room's manual order: a fractional sort key
    /// between the `--after`/`--before` neighbors, sent as an ordinary
    /// full-state update. Step 60c.
    Move {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// The task's stable `task_id` (from `task list`).
        #[arg(long)]
        task: String,
        /// Place the task after this neighbor `task_id`.
        #[arg(long)]
        after: Option<String>,
        /// Place the task before this neighbor `task_id`.
        #[arg(long)]
        before: Option<String>,
        /// Send the bare payload without the Text fallback (mode a).
        /// Without either fallback flag the default derives from the
        /// room's task-list marker (ADR-0020).
        #[arg(long, conflicts_with = "fallback")]
        no_fallback: bool,
        /// Force the Text fallback on (mode c) even in a marked room.
        #[arg(long)]
        fallback: bool,
    },
    /// Mark a task done (status = done). Step 60b.
    Done {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// The task's stable `task_id` (from `task list`).
        #[arg(long)]
        task: String,
        /// Send the bare payload without the Text fallback (mode a).
        /// Without either fallback flag the default derives from the
        /// room's task-list marker (ADR-0020).
        #[arg(long, conflicts_with = "fallback")]
        no_fallback: bool,
        /// Force the Text fallback on (mode c) even in a marked room.
        #[arg(long)]
        fallback: bool,
    },
    /// Re-assign a task to another member. Step 60b.
    Assign {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// The task's stable `task_id` (from `task list`).
        #[arg(long)]
        task: String,
        /// Qualified user id (`<uuid>@<domain>`) of the new assignee.
        #[arg(short = 'u', long)]
        user: String,
        /// Send the bare payload without the Text fallback (mode a).
        /// Without either fallback flag the default derives from the
        /// room's task-list marker (ADR-0020).
        #[arg(long, conflicts_with = "fallback")]
        no_fallback: bool,
        /// Force the Text fallback on (mode c) even in a marked room.
        #[arg(long)]
        fallback: bool,
    },
    /// Edit task fields via a set-only patch (at least one field
    /// required; optional fields can be set, not cleared). Step 60b.
    Edit {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// The task's stable `task_id` (from `task list`).
        #[arg(long)]
        task: String,
        /// New task headline.
        #[arg(long)]
        title: Option<String>,
        /// New free-text description.
        #[arg(long)]
        description: Option<String>,
        /// Qualified user id (`<uuid>@<domain>`) of the assignee.
        #[arg(long)]
        assignee: Option<String>,
        /// Due date, ISO 8601: `yyyy-mm-dd` (due by end of that local
        /// day), `yyyy-mm-ddThh:mm[:ss]` (local time), or RFC 3339
        /// with offset/`Z`.
        #[arg(long, conflicts_with = "due_ms")]
        due: Option<String>,
        /// Due date as raw Unix milliseconds (machine escape hatch).
        #[arg(long)]
        due_ms: Option<i64>,
        /// Task priority.
        #[arg(long, value_enum)]
        priority: Option<cli::task::PriorityArg>,
        /// Task status (open | in-progress | done | cancelled).
        #[arg(long, value_enum)]
        status: Option<cli::task::StatusArg>,
        /// Send the bare payload without the Text fallback (mode a).
        /// Without either fallback flag the default derives from the
        /// room's task-list marker (ADR-0020).
        #[arg(long, conflicts_with = "fallback")]
        no_fallback: bool,
        /// Force the Text fallback on (mode c) even in a marked room.
        #[arg(long)]
        fallback: bool,
    },
    /// Set or clear the room's task-list marker (ADR-0020): any
    /// member may flip it; folded into `Room::is_task_list()` and
    /// re-broadcast to invitees. Official clients see nothing.
    /// Step 75.
    Mark {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Clear the marker instead of setting it.
        #[arg(long)]
        clear: bool,
    },
}

#[derive(Subcommand)]
enum SsoCommand {
    /// Build the interactive SSO initiate URL for a known SSO code.
    ///
    /// Validates the code against the backend
    /// (`HEAD /sso/initiate-login/{code}`) and prints the URL to open in a
    /// browser. The post-IdP redirect carries the `zuid` cookie that
    /// `sso finalize` (Step 17b) will consume.
    Initiate {
        /// Base https URL of a self-hosted Wire-compatible backend.
        #[arg(long)]
        api_url: String,
        /// WebSocket URL (wss://…); derived from --api-url when omitted.
        #[arg(long)]
        ws_url: Option<String>,
        /// The SSO code (idp identifier). Provide this or --email.
        #[arg(
            short = 'c',
            long,
            required_unless_present = "email",
            conflicts_with = "email"
        )]
        code: Option<String>,
        /// Discover the SSO code from the backend default
        /// (`GET /sso/settings`) for this email instead of passing --code.
        #[arg(short = 'e', long)]
        email: Option<String>,
        /// Where the backend redirects after a successful IdP round-trip
        /// (carries the `zuid` cookie for `sso finalize`). Any URL works;
        /// the default is a scheme no installed app claims, so the browser
        /// shows the redirect instead of handing it to another client.
        #[arg(long, default_value = "kupferdraht://sso/done")]
        success_redirect: String,
        /// Where the backend redirects on IdP failure.
        #[arg(long, default_value = "kupferdraht://sso/error")]
        error_redirect: String,
    },
    /// Complete SSO login from the `zuid` cookie captured off the redirect.
    ///
    /// After `sso initiate`, the browser round-trip redirects to
    /// `success_redirect` carrying the `zuid` refresh cookie. Pass that value
    /// here to mint the session, register this device, and persist the account
    /// (listable via `accounts`, resumable via `resume`).
    Finalize {
        /// Base https URL of a self-hosted Wire-compatible backend.
        #[arg(long)]
        api_url: String,
        /// WebSocket URL (wss://…); derived from --api-url when omitted.
        #[arg(long)]
        ws_url: Option<String>,
        /// The `zuid` cookie value captured from the finalize-login redirect.
        #[arg(long)]
        cookie: String,
        /// On the 7-device cap, delete this device id to free a slot and retry.
        #[arg(long)]
        delete_device: Option<String>,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate with email + password and persist the session.
    ///
    /// Password is read interactively (TTY prompt, no echo) by default.
    /// Pass `--password-stdin` to read from stdin instead — useful for
    /// piping from a password manager or for non-interactive CI runs.
    ///
    /// There is deliberately no `--password` flag: secrets in argv leak
    /// into `ps`, shell history, and process listings.
    Login {
        /// Base https URL of a self-hosted Wire-compatible backend,
        /// e.g. https://nginz-https.example.org (backend-spec §20:
        /// changing the base URL is all a client needs).
        #[arg(long)]
        api_url: String,
        /// WebSocket URL (wss://…) of that backend; derived from
        /// --api-url by the nginz naming convention when omitted.
        #[arg(long)]
        ws_url: Option<String>,
        #[arg(short = 'e', long)]
        email: String,
        /// Read the password from stdin (non-interactive). Without this
        /// flag the CLI prompts on the TTY.
        #[arg(long)]
        password_stdin: bool,
        /// On `too-many-clients` (the 7-device cap), delete this device
        /// id to free a slot and retry the login. Without it, hitting the
        /// limit just prints the registered devices. Step 24.
        #[arg(long)]
        delete_device: Option<String>,
    },
    /// Create a fresh account on the configured backend.
    ///
    /// POSTs `/register` against the chosen backend and prints the
    /// pending user-id plus the email Wire will deliver the activation
    /// code to. Run `kupferdraht-cli activate` once the email arrives to
    /// complete the loop.
    ///
    /// This registers a real account on the backend you name; use a
    /// test instance. The CLI does not enforce this — the operator
    /// does.
    ///
    /// Password is read interactively (TTY prompt, no echo) by
    /// default; pass `--password-stdin` to read from stdin. No
    /// `--password` flag exists — secrets in argv leak.
    Register {
        /// Base https URL of a self-hosted Wire-compatible backend,
        /// e.g. https://nginz-https.example.org (backend-spec §20:
        /// changing the base URL is all a client needs).
        #[arg(long)]
        api_url: String,
        /// WebSocket URL (wss://…) of that backend; derived from
        /// --api-url by the nginz naming convention when omitted.
        #[arg(long)]
        ws_url: Option<String>,
        #[arg(short = 'e', long)]
        email: String,
        #[arg(short = 'n', long)]
        name: String,
        /// Read the password from stdin (non-interactive).
        #[arg(long)]
        password_stdin: bool,
    },
    /// SSO login (Step 17β, interactive / SP-driven).
    Sso {
        #[command(subcommand)]
        action: SsoCommand,
    },
    /// Activate a pending account with the email-delivered code.
    ///
    /// POSTs `/activate` against the chosen backend, then drives the
    /// same login pipeline `kupferdraht-cli login` runs — registers the
    /// device, rebinds the access token, uploads MLS key packages,
    /// and persists the full session state to `kupferdraht.db`. After
    /// this command succeeds, the user can immediately run `rooms`,
    /// `sync`, `send`, etc.
    ///
    /// `--email` must be supplied explicitly because the CLI does not
    /// persist any state between `register` and `activate` invocations
    /// (PendingClient is in-memory only).
    Activate {
        /// Base https URL of a self-hosted Wire-compatible backend,
        /// e.g. https://nginz-https.example.org (backend-spec §20:
        /// changing the base URL is all a client needs).
        #[arg(long)]
        api_url: String,
        /// WebSocket URL (wss://…) of that backend; derived from
        /// --api-url by the nginz naming convention when omitted.
        #[arg(long)]
        ws_url: Option<String>,
        #[arg(short = 'e', long)]
        email: String,
        #[arg(short = 'c', long)]
        code: String,
        /// Read the password from stdin (non-interactive).
        #[arg(long)]
        password_stdin: bool,
    },
    /// Print the most recently persisted session.
    Status,
    /// List the accounts registered on this device (no unlock needed).
    Accounts,
    /// Refresh the rooms snapshot in `kupferdraht.db` from a persisted
    /// session — no password required.
    ///
    /// Loads the latest session, server config, device, self-user,
    /// and backend removal key from `kupferdraht.db`, refreshes the
    /// access token via `POST /v14/access?client_id=<id>` using the
    /// persisted `zuid` refresh cookie, then drives
    /// `Client::sync_metadata` through
    /// `POST /v14/conversations/list-ids` +
    /// `POST /v14/conversations/list` + `POST /v14/list-users`.
    ///
    /// Requires a prior `kupferdraht-cli login`. Exits non-zero if no
    /// persisted session exists.
    Sync,
    /// Lightweight metadata-only sync — refreshes the local rooms
    /// snapshot from `POST /v14/conversations/list-ids` +
    /// `POST /v14/conversations/list` + `POST /v14/list-users`
    /// **without** draining `/v14/notifications`. Use over `sync`
    /// when you want a cheap freshness check and don't need the
    /// per-event decrypt loop to run (battery, latency, or
    /// background-startup scenarios on future mobile bindings).
    /// Library entry point: `Client::sync_metadata`.
    SyncMetadata,
    /// List the rooms previously written to `kupferdraht.db` by `sync`.
    /// Reads from the local store only — no network, no
    /// authentication. Empty when `sync` has not run.
    Rooms,
    /// Join a single existing MLS group via external commit.
    ///
    /// Fetches the group's current `GroupInfo` from Wire, runs
    /// openmls's external-commit builder locally, and uploads the
    /// resulting commit bundle to `POST /v14/mls/commit-bundles`.
    /// Requires a prior `kupferdraht-cli login` and a recent `kupferdraht-cli
    /// sync` so the target room id is in the cache.
    RoomJoin {
        /// Qualified room id (`<uuid>@<domain>`) as printed by
        /// `kupferdraht-cli rooms`.
        #[arg(long)]
        id: String,
    },
    /// Join every room the local device is not yet a member of via
    /// external commit. Skips rooms whose MLS group is already in the
    /// local openmls storage — no extra network traffic. Prints a
    /// summary and a per-room failure list.
    RoomsJoinAll,
    /// Re-join every room whose persisted MLS leaf belongs to a prior
    /// `device_id` of the same user. Use after a fresh `login` to
    /// recover access to rooms created or joined under an older
    /// device. Skips rooms whose leaf already matches the current
    /// device. Prints a summary and a per-room failure list. Step 7e.
    RejoinAll,
    /// List the members of a room as derived from the local MLS
    /// group's leaves (post-`room-join`). Resolves display name /
    /// handle for every non-self leaf via `POST /v14/list-users`.
    RoomMembers {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// List the devices of a room — one per leaf of the local MLS group,
    /// with the MLS thumbprint to compare against the "MLS Thumbprint" an
    /// official Wire app shows in the device details. `*` marks this
    /// device. Step 95d.
    RoomDevices {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Encrypt a plain-text `GenericMessage` and POST it to
    /// `/v14/mls/messages`. The room must have a local MLS group
    /// (run `room-join` first if Kupferdraht did not create the group).
    /// Persists a `timeline_events` row with
    /// `delivery_status='submitted'`.
    Send {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Message body to send.
        #[arg(short = 'm', long)]
        message: String,
        /// Send as a self-deleting message with this timer in seconds
        /// (1 … 31536000): official clients show a countdown and delete
        /// their copy; Kupferdraht's own copy self-redacts too (Step 89).
        /// `@handle` mentions are not parsed in this mode.
        #[arg(long, value_name = "SECS")]
        ephemeral: Option<u64>,
    },
    /// Re-drive a failed send (a `delivery_status='failed'` row —
    /// the upload never reached the backend) from its persisted row
    /// under the SAME message_id: text incl. reply / mentions,
    /// ephemeral text (re-wrapped with its timer), knock, location,
    /// and files (the blob is already uploaded; only the MLS
    /// reference travels again). Task / case / marker snapshots are
    /// refused — re-send the current state instead. Step 79.
    ///
    /// One row via `--message` (room auto-resolved from the id when
    /// `--id` is omitted), or the whole room oldest-first via
    /// `--id <room> --all` — that pass stops at the first hard
    /// error and leaves the rest failed for the next run.
    Resend {
        /// Qualified room id (`<uuid>@<domain>`). Required with
        /// `--all`; optional with `--message` (auto-resolved from
        /// the message id against the local timeline cache).
        #[arg(long)]
        id: Option<String>,
        /// `message_id` of the failed row (from `failed-sends` /
        /// `timeline`, marked `✗`).
        #[arg(
            long = "message",
            conflicts_with = "all",
            required_unless_present = "all"
        )]
        message: Option<String>,
        /// Re-drive every failed send of the room, oldest first.
        #[arg(long, requires = "id")]
        all: bool,
    },
    /// List a room's failed sends (`delivery_status='failed'`),
    /// oldest first — the rows `resend` can re-drive. Step 79.
    FailedSends {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Long-running receive loop — Kupferdraht's canonical continuous-
    /// receive surface (Step 5d). Holds the push socket and drains
    /// `GET /v14/notifications` whenever it rings (`--transport
    /// polling`: drains in a loop, sleeping `--idle-backoff` seconds
    /// between empty drains), dispatching events through the
    /// registered handler. Modelled on
    /// `matrix-rust-sdk::Client::sync()`. **Blocks** until Ctrl-C.
    SyncStream {
        /// Seconds to sleep when a drain returns zero
        /// notifications (polling), or the push pace — at most one
        /// connect, and one drain without a socket, per pace
        /// (websocket). Unset: the library's default, 10 s for both
        /// (Step 118).
        #[arg(long)]
        idle_backoff: Option<u64>,
        /// Receive transport: `websocket` (the default since Step 118,
        /// ADR-0033) holds the push socket — its frames trigger the
        /// REST drain; `polling` drains GET /v14/notifications in a
        /// loop.
        #[arg(long, default_value = "websocket")]
        transport: String,
    },
    /// Create a new MLS conversation (Step 6a, zero-invitee). The
    /// group is registered server-side via `POST /v14/conversations`
    /// and the local MLS state is built at epoch 0 with Wire's
    /// backend removal key in the `external_senders` extension.
    /// Print the qualified `room_id` so subsequent `send` /
    /// `room-join` / `room-members` calls can target it.
    RoomCreate {
        /// Room name (1–256 characters per Wire's schema).
        #[arg(short = 'n', long)]
        name: String,
    },
    /// Invite one or more users into an existing room (Step 6b).
    /// Claims one KeyPackage per device for each invitee, builds a
    /// single Add-commit, uploads it to `/v14/mls/commit-bundles`,
    /// and re-syncs the room metadata. The first invite into a
    /// freshly-created (epoch 0) room also unlocks `send` against
    /// it (CHANGELOG.md §25).
    RoomInvite {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// One or more qualified user ids (`<uuid>@<domain>`),
        /// repeatable.
        #[arg(short = 'u', long = "user", required = true, num_args = 1..)]
        users: Vec<String>,
    },
    /// Remove one or more users from an existing room (Step 6d).
    /// Builds a single Remove-commit covering every leaf belonging
    /// to the listed users (one user can have up to 7 device
    /// leaves), uploads it to `/v14/mls/commit-bundles`, and
    /// re-syncs the room metadata. Self-kick is rejected.
    RoomKick {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// One or more qualified user ids (`<uuid>@<domain>`),
        /// repeatable.
        #[arg(short = 'u', long = "user", required = true, num_args = 1..)]
        users: Vec<String>,
    },
    /// Create — and establish — the MLS 1:1 with the named peer
    /// (Steps 14a + 68). `GET /v14/one2one-conversations/{d}/{u}`
    /// reserves Wire's first-class 1:1 (idempotent server-side —
    /// calling against an existing 1:1 returns it). epoch 0 → claim
    /// the peer's key packages + upload the establishing Add-commit
    /// (only then does wire-server list the conversation); epoch > 0
    /// → external-commit join; local group present → resume. Runs
    /// `sync_metadata` and prints the resulting qualified `room_id`.
    /// matrix-rust-sdk analog: `Client::create_dm`.
    DmCreate {
        /// Qualified peer user id (`<uuid>@<domain>`).
        #[arg(long)]
        peer: String,
    },
    /// Look up the cached 1:1 conversation with the named peer
    /// (Step 14a). Strict local-cache lookup — no network, no
    /// authentication. Prints the room id when one is on file,
    /// exits with status 2 otherwise (mirrors `room` /
    /// `room-members` on a cache miss). matrix-rust-sdk analog:
    /// `Client::get_dm_room` (renamed per finding §41).
    DmShow {
        /// Qualified peer user id (`<uuid>@<domain>`).
        #[arg(long)]
        peer: String,
    },
    /// "Look up first, create if missing" convenience for 1:1
    /// conversations (Step 14a). Cache hit prints the existing
    /// room id without any HTTP traffic; cache miss falls through
    /// to `dm-create`. Concurrent invocations against the same
    /// peer de-duplicate the POST. Kupferdraht-specific (no
    /// matrix-rust-sdk analog — see finding §42).
    DmWith {
        /// Qualified peer user id (`<uuid>@<domain>`).
        #[arg(long)]
        peer: String,
    },
    /// Look up the caller's self conversation — Wire's notes-to-self
    /// room, an MLS group of the user's own devices (Step 86). Strict
    /// local-cache lookup: no network, no authentication. Prints
    /// `room_id` + `epoch` when the last sync's snapshot carries it,
    /// exits with status 2 otherwise (run `sync` first). Epoch 0 means
    /// nobody established the group yet — `rooms-join-all` (or
    /// `room-join --id <room_id>`) establishes it from this device.
    /// No matrix-rust-sdk analog.
    SelfRoom,
    /// List every device registered for the authenticated user
    /// (Step 1d). Wire enforces a 7-device cap; this is the
    /// CLI-driven escape hatch when the cap is hit and
    /// `POST /v14/clients` (login) starts failing.
    Devices,
    /// Show this device's server-side MLS key-package pool (Step 63).
    /// Peers claim one package per invite; an empty pool means the
    /// device can no longer be added to new conversations. The sync
    /// loop tops the pool up automatically once per day.
    KeyPackages {
        /// Force a top-up now instead of waiting for the daily
        /// sync-loop check (no-op while the pool is healthy).
        #[arg(long)]
        replenish: bool,
    },
    /// Revoke a registered device by its `client_id` (Step 1d).
    /// Wire requires the user's password; pass it via stdin
    /// (`--password-stdin`) or be prompted interactively.
    /// Refuses to delete the *current* device — use `logout` for
    /// that once Step 6e ships.
    DeviceDelete {
        /// Wire `client_id` (hexadecimal) of the device to delete,
        /// as printed by `kupferdraht-cli devices`.
        #[arg(long)]
        id: String,
        /// Read the password from stdin (non-interactive). Without
        /// this flag the CLI prompts on the TTY.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Rename a registered device (Step 1d). Updates the
    /// user-visible label shown in Wire's settings UI.
    DeviceRename {
        /// Wire `client_id` of the device to rename, as printed
        /// by `kupferdraht-cli devices`.
        #[arg(long)]
        id: String,
        /// New label.
        #[arg(short = 'l', long)]
        label: String,
    },
    /// Update the authenticated user's profile (Step 2g). At least
    /// one of `--display-name` or `--handle` must be supplied.
    /// `--display-name` patches Wire's `PUT /v{N}/self`;
    /// `--handle` patches `PUT /v{N}/self/handle`. Both are sent
    /// independently when both are supplied.
    ProfileSet {
        /// New display name (Wire's `name` field).
        #[arg(long)]
        display_name: Option<String>,
        /// New handle (Wire's `handle` field). Wire enforces global
        /// uniqueness; collisions surface as `BackendRejected`.
        #[arg(long)]
        handle: Option<String>,
    },
    /// Request a change of the account email (Step 98a). Patches Wire's
    /// `PUT /v{N}/access/self/email`; Wire mails a verification link and the
    /// address only changes once that link is followed — so a success
    /// here means "mail sent", not "email changed".
    EmailUpdate {
        /// The new email address.
        #[arg(long)]
        email: String,
    },
    /// Change the account password (Step 27). Prompts for the current
    /// and new password on the TTY (no passwords in argv). On success
    /// Wire invalidates every device's session, so all devices must
    /// re-login; registered devices and MLS group state survive.
    ChangePassword,
    /// Rename a conversation server-side (Step 2g). Sends
    /// `PUT /v{N}/conversations/{domain}/{id}/name`. Wire emits a
    /// `conversation.rename` event to all members.
    RoomRename {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// New conversation name.
        #[arg(short = 'n', long)]
        name: String,
    },
    /// Leave a room (Step 6e-1). Emits a self-Remove MLS commit to
    /// Wire and discards the local openmls group state. After
    /// success the room no longer appears in `rooms` and `send`
    /// against it fails.
    RoomLeave {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Delete a team conversation for everyone (Step 23, team-admin
    /// action — distinct from `room-leave`). `DELETE /v{N}/teams/{tid}/
    /// conversations/{cid}`; the owning team id is read from the
    /// conversation's `team` field. Only valid for team conversations —
    /// a 1:1 / non-team group is rejected (leave it instead).
    RoomDelete {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Mark a message as read (Step 6-II-f). Wire emits
    /// `Confirmation { type: READ, first_message_id }`. Recipients
    /// stamp the `(message_id, reader_user_id)` tuple into
    /// `read_receipts`; Wire web shows a "read" indicator.
    ///
    /// `--id` is optional: when omitted, the CLI looks up the
    /// owning room from the local `timeline_events` table. Pass
    /// it explicitly when the message has not been synced locally
    /// or when the lookup is ambiguous.
    MarkRead {
        /// Qualified room id (`<uuid>@<domain>`). Optional —
        /// auto-resolved from `--message` against the local
        /// timeline cache when omitted.
        #[arg(long)]
        id: Option<String>,
        /// `message_id` of the message to confirm as read.
        #[arg(long = "message")]
        message: String,
    },
    /// Toggle a single emoji reaction on (Step 6-II-e). If the
    /// emoji is already in the caller's reaction set on the
    /// target, it is removed; otherwise added. Wire emits a
    /// fresh `Reaction` event carrying the caller's complete
    /// post-toggle set as a comma-separated string.
    ///
    /// `--id` is optional — see `mark-read` for the resolution
    /// rule.
    React {
        /// Qualified room id (`<uuid>@<domain>`). Optional —
        /// auto-resolved from `--message` against the local
        /// timeline cache when omitted.
        #[arg(long)]
        id: Option<String>,
        /// `message_id` of the message to react to.
        #[arg(long = "message")]
        message: String,
        /// Emoji to toggle (e.g. `👍`, `❤️`).
        #[arg(long = "emoji")]
        emoji: String,
    },
    /// Delete-for-everyone a previously-sent text message (Step
    /// 6-II-d). Wire-side emits a `MessageDelete { message_id }`
    /// inside a fresh `GenericMessage`. Receivers tombstone the
    /// row; the body is hidden but the row itself stays so reply
    /// chains remain renderable.
    ///
    /// `--id` is optional — see `mark-read` for the resolution
    /// rule.
    DeleteMessage {
        /// Qualified room id (`<uuid>@<domain>`). Optional —
        /// auto-resolved from `--message` against the local
        /// timeline cache when omitted.
        #[arg(long)]
        id: Option<String>,
        /// `message_id` of the original message being retired
        /// (UUID from prior `send` / `reply` / `timeline` output).
        #[arg(long = "message")]
        message: String,
    },
    /// Edit a previously-sent text message (Step 6-II-c). Wire-side
    /// emits a `MessageEdit { replacing_message_id, text { new_body } }`
    /// inside a regular `GenericMessage`. Receivers replace the
    /// original message_id's body with the new one.
    ///
    /// `--id` is optional — see `mark-read` for the resolution
    /// rule.
    Edit {
        /// Qualified room id (`<uuid>@<domain>`). Optional —
        /// auto-resolved from `--message` against the local
        /// timeline cache when omitted.
        #[arg(long)]
        id: Option<String>,
        /// `message_id` of the original message being replaced
        /// (UUID from prior `send` / `reply` / `timeline` output).
        #[arg(long = "message")]
        message: String,
        /// New body.
        #[arg(short = 'm', long = "new-body")]
        new_body: String,
    },
    /// Send a text reply that quotes a previous message (Step
    /// 6-II-b). Wire-side this emits a `Text.quote.quoted_message_id`
    /// inside a regular `GenericMessage`; receivers thread it as
    /// a quote / reply.
    ///
    /// `--id` is optional — when omitted the CLI looks up the
    /// parent message's room via `--to` against the local
    /// timeline cache.
    Reply {
        /// Qualified room id (`<uuid>@<domain>`). Optional —
        /// auto-resolved from `--to` against the local timeline
        /// cache when omitted.
        #[arg(long)]
        id: Option<String>,
        /// `message_id` of the parent message being quoted (UUID
        /// from prior `send` / `timeline` output).
        #[arg(long = "to")]
        to: String,
        /// Reply body.
        #[arg(short = 'm', long)]
        message: String,
    },
    /// Print persisted timeline rows for a room (Step 6-II-a).
    /// Reads from `timeline_events` only — no network call. Use
    /// after `sync-fetch` / `sync-stream` so the receive-path has
    /// populated the rows first.
    Timeline {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Maximum number of items to print (omitted = no limit).
        #[arg(long)]
        limit: Option<u32>,
        /// Cursor: only items strictly older than this `message_id`
        /// are printed.
        #[arg(long)]
        before: Option<String>,
    },
    /// Sign out of the backend (Step 6e-2). Revokes the `zuid` refresh
    /// cookie server-side via `POST /v{N}/access/logout`. With
    /// `--purge-device`, **also** issues
    /// `DELETE /v{N}/clients/{device_id}` to remove the registered
    /// device from Wire entirely (frees a slot in the 7-device cap)
    /// — Wire requires the user's password for that step.
    ///
    /// Local DB cleanup: with `--purge-device`, the CLI also wipes
    /// `~/.kupferdraht/` so the next `login` starts from a clean slate.
    /// Without `--purge-device`, the local DB is preserved and the
    /// next `login` reuses the existing device via the Step 7e
    /// reuse path.
    Logout {
        /// Also delete this device from Wire's clients list.
        /// Requires the user's password (prompt or
        /// `--password-stdin`). Wipes `~/.kupferdraht/` afterwards.
        #[arg(long)]
        purge_device: bool,
        /// Read the password from stdin (only relevant with
        /// `--purge-device`).
        #[arg(long)]
        password_stdin: bool,
    },
    /// Issue (or fetch the existing) reusable guest-link for a room
    /// via `POST /v{N}/conversations/{cnv}/code` (Step 8). Prints
    /// the link the backend issues
    /// (`https://account.<operator>/conversation-join/?key=…&code=…`)
    /// plus whether the link is password-protected. Pass `-p` to require
    /// a password on redeem; the password is read interactively
    /// (TTY, no echo) — there is deliberately no `--password` flag
    /// to keep secrets out of `ps`.
    JoinLinkGenerate {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Issue a password-protected link. The password is read on
        /// the TTY (no echo) and must be ≥ 8 characters per Wire's
        /// schema.
        #[arg(short = 'p', long)]
        password: bool,
    },
    /// Revoke the room's current guest link via
    /// `DELETE /v{N}/conversations/{cnv}/code` (Step 8). Idempotent:
    /// revoking a non-existent code is a no-op.
    JoinLinkRevoke {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Fetch the room's current guest link via
    /// `GET /v{N}/conversations/{cnv}/code` (Step 8). Prints
    /// `(no link issued)` when none is on file.
    JoinLinkCurrent {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Display the current `access` + `access_role` arrays for a
    /// conversation (Step 8b). Re-fetches via
    /// `POST /v{N}/conversations/list` so the output reflects
    /// Wire's live state, not the local sync snapshot.
    RoomAccessShow {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Replace `access` + `access_role` for a conversation
    /// (Step 8b). `--modes` is comma-separated (`invite`, `code`,
    /// `link`, `private`); `--roles` is comma-separated
    /// (`team_member`, `non_team_member`, `guest`, `service`).
    RoomAccessSet {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Comma-separated access modes (`invite,code` is the
        /// canonical guest-link config).
        #[arg(long)]
        modes: String,
        /// Comma-separated access roles
        /// (`team_member,non_team_member,guest` for guest links).
        #[arg(long)]
        roles: String,
    },
    /// Convenience over `room-access set`: opens the conversation
    /// up for guest-link joins via the canonical
    /// `access=["invite","code"]`,
    /// `access_role=["team_member","non_team_member","guest"]`
    /// configuration. Step 8b. Most Kupferdraht-created conversations
    /// default to `access=["invite"]`; calling this once is the
    /// prerequisite for `join-link-generate`.
    RoomAccessAllowGuests {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Redeem a guest link and MLS-join the conversation it names
    /// (Steps 8, 122). Looks the conversation up by the link
    /// (`GET /v{N}/conversations/join`), joins the caller as a
    /// Wire-conversation member (`POST /v{N}/conversations/join`), then
    /// runs `sync_metadata` and the Step-3 external-commit join on that
    /// room. The password (if the link is password-protected) is read on
    /// the TTY (no echo).
    JoinViaLink {
        /// The link's `code` query parameter.
        #[arg(long)]
        code: String,
        /// The link's `key` query parameter.
        #[arg(long)]
        key: String,
        /// Supply the link's password. Reads from the TTY (no echo).
        #[arg(short = 'p', long)]
        password: bool,
    },
    /// List Wire's team-feature configs as persisted at the last
    /// successful login or `feature-config.update` event (Step 10c).
    /// Reads from the local store only — no network call. With
    /// `--name <key>` prints one entry's full detail (status,
    /// `last_synced_at_ms`, raw `config_json`); without flags lists
    /// every persisted entry one per line. Empty when login has not
    /// yet populated the V18 `feature_configs` table.
    Features {
        /// Show only the feature config matching this Wire key
        /// (`fileSharing`, `mls`, `mlsE2EId`, …).
        #[arg(long)]
        name: Option<String>,
    },
    /// List the authenticated user's properties as persisted at the
    /// last login or `user.properties-set` event (Step 10c). Reads
    /// from the local store only — no network call. With
    /// `--key <name>` prints one property's full detail (value,
    /// `last_synced_at_ms`); without flags lists every persisted
    /// property one per line.
    Properties {
        /// Show only the property matching this Wire key
        /// (`WIRE_RECEIPT_MODE`, `labels`, …).
        #[arg(long)]
        key: Option<String>,
    },
    /// Encrypt + upload + MLS-send a file as an Asset content
    /// variant (Step 11h, ADR-0011). The team's `fileSharing`
    /// policy from Step 10e gates the call before any
    /// network traffic — `Disabled` rejects with InvalidInput;
    /// `Enabled` proceeds.
    SendFile {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Path to the plaintext file to send. Streamed through
        /// AES-256-CBC + SHA-256; memory profile stays under
        /// ~200 KB regardless of file size.
        #[arg(long)]
        path: PathBuf,
        /// Mime type for the asset (e.g. `image/png`,
        /// `application/pdf`). Embedded into
        /// `Asset.Original.mime_type`.
        #[arg(long)]
        mime: String,
    },
    /// Send a `Knock` (nudge) to a room (Step 26). Wire-side emits a
    /// `GenericMessage { knock { hot_knock } }`; receivers surface it
    /// as a poke with no body.
    Knock {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Send a "hot" (double-tap) knock instead of a plain one.
        #[arg(long)]
        hot: bool,
    },
    /// Share a `Location` (map point) with a room (Step 26). Wire-side
    /// emits a `GenericMessage { location { longitude, latitude, name?,
    /// zoom? } }`.
    Location {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Latitude in decimal degrees.
        #[arg(long)]
        lat: f32,
        /// Longitude in decimal degrees.
        #[arg(long)]
        lon: f32,
        /// Optional human-readable place label.
        #[arg(long)]
        name: Option<String>,
        /// Optional map zoom level.
        #[arg(long)]
        zoom: Option<i32>,
    },
    /// Tasks / todos (Step 60a, ADR-0014). Wire-side emits a
    /// `GenericMessage` carrying a `TaskPayload` in field 262000 (outside
    /// the content oneof), optionally alongside a human-readable `Text`
    /// fallback official Wire clients render.
    Task {
        #[command(subcommand)]
        action: TaskCommand,
    },
    /// Create or update the room's case (Step 72c,
    /// ADR-0018): patches the folded state and sends the resulting
    /// full CasePayload snapshot (no text fallback; requires the
    /// caller to be a conversation administrator). Unset flags keep
    /// their current values.
    CaseSet {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
        /// Case identity — honored only when creating.
        #[arg(long = "case-ref")]
        case_ref: Option<String>,
        /// File reference / id in the leading external system.
        #[arg(long = "external-ref")]
        external_ref: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long = "case-type")]
        case_type: Option<String>,
        /// open|active|waiting|resolved|closed|cancelled
        #[arg(long)]
        state: Option<String>,
        /// Free-text sub-state.
        #[arg(long)]
        detail: Option<String>,
        /// Qualified user id of the assignee.
        #[arg(long)]
        assignee: Option<String>,
        /// Deadline, milliseconds since Unix epoch.
        #[arg(long = "due-ms")]
        due_ms: Option<i64>,
        /// low|normal|high|urgent
        #[arg(long)]
        priority: Option<String>,
        /// Tenant or public authority.
        #[arg(long)]
        authority: Option<String>,
        /// Display-only extras, KEY=VALUE, repeatable. Replaces the
        /// whole extensions map when given.
        #[arg(long = "ext", num_args = 0..)]
        ext: Vec<String>,
    },
    /// Print the folded case state of a room (Step 72c) — the latest
    /// admin-authorized snapshot plus display-only extensions.
    CaseShow {
        /// Qualified room id (`<uuid>@<domain>`).
        #[arg(long)]
        id: String,
    },
    /// Download + decrypt an inbound asset that has been
    /// dispatched into the local store (Step 11h). Verifies
    /// SHA-256 against the value carried inside the MLS-protected
    /// `Asset.RemoteData.sha256`; rejects tampered ciphertext
    /// with `IntegrityCheck`.
    ///
    /// `--id` is optional — without it the CLI looks up the
    /// owning room from local `timeline_events` (same resolver
    /// as `mark-read` / `react` / etc.). With `--save-to <path>`
    /// the plaintext is streamed to the filesystem; without it
    /// the command prints the asset's metadata only (filename,
    /// mime, size, sender, cache state).
    DownloadAsset {
        /// `message_id` of the asset message.
        #[arg(long = "message")]
        message: String,
        /// Optional explicit room id. Auto-resolved from
        /// `--message` against the local timeline cache when
        /// omitted.
        #[arg(long)]
        id: Option<String>,
        /// Filesystem path to write the decrypted plaintext to.
        /// Without this flag, the command prints metadata only
        /// (useful for inspecting an asset before deciding to
        /// download multi-megabyte content).
        #[arg(long = "save-to")]
        save_to: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    // Local-offset lookup must precede the runtime: `time` refuses
    // the Unix env read once worker threads exist.
    cli::init_local_offset();
    async_main()
}

#[tokio::main]
async fn async_main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Login {
            api_url,
            ws_url,
            email,
            password_stdin,
            delete_device,
        } => {
            let password = match read_password(password_stdin) {
                Ok(p) if !p.is_empty() => p,
                Ok(_) => {
                    eprintln!("kupferdraht-cli: empty password");
                    return ExitCode::from(2);
                }
                Err(e) => {
                    eprintln!("kupferdraht-cli: read password: {e}");
                    return ExitCode::from(2);
                }
            };
            match backend::resolve_server(&api_url, ws_url.as_deref()) {
                Ok(server) => cli::auth::login(server, email, password, delete_device).await,
                Err(e) => Err(e),
            }
        }
        Command::Register {
            api_url,
            ws_url,
            email,
            name,
            password_stdin,
        } => {
            let password = match read_password(password_stdin) {
                Ok(p) if !p.is_empty() => p,
                Ok(_) => {
                    eprintln!("kupferdraht-cli: empty password");
                    return ExitCode::from(2);
                }
                Err(e) => {
                    eprintln!("kupferdraht-cli: read password: {e}");
                    return ExitCode::from(2);
                }
            };
            match backend::resolve_server(&api_url, ws_url.as_deref()) {
                Ok(server) => cli::auth::register(server, email, name, password).await,
                Err(e) => Err(e),
            }
        }
        Command::Activate {
            api_url,
            ws_url,
            email,
            code,
            password_stdin,
        } => {
            let password = match read_password(password_stdin) {
                Ok(p) if !p.is_empty() => p,
                Ok(_) => {
                    eprintln!("kupferdraht-cli: empty password");
                    return ExitCode::from(2);
                }
                Err(e) => {
                    eprintln!("kupferdraht-cli: read password: {e}");
                    return ExitCode::from(2);
                }
            };
            match backend::resolve_server(&api_url, ws_url.as_deref()) {
                Ok(server) => cli::auth::activate(server, email, code, password).await,
                Err(e) => Err(e),
            }
        }
        Command::Sso { action } => match action {
            SsoCommand::Initiate {
                api_url,
                ws_url,
                code,
                email,
                success_redirect,
                error_redirect,
            } => match backend::resolve_server(&api_url, ws_url.as_deref()) {
                Ok(server) => {
                    cli::auth::sso_initiate(server, code, email, success_redirect, error_redirect)
                        .await
                }
                Err(e) => Err(e),
            },
            SsoCommand::Finalize {
                api_url,
                ws_url,
                cookie,
                delete_device,
            } => match backend::resolve_server(&api_url, ws_url.as_deref()) {
                Ok(server) => cli::auth::sso_finalize(server, cookie, delete_device).await,
                Err(e) => Err(e),
            },
        },
        Command::Status => cli::auth::status().await,
        Command::Accounts => cli::auth::accounts().await,
        Command::Sync => cli::sync::sync().await,
        Command::SyncMetadata => cli::sync::sync_metadata().await,
        Command::Rooms => cli::rooms::rooms().await,
        Command::RoomJoin { id } => cli::rooms::room_join(id).await,
        Command::RoomsJoinAll => cli::rooms::rooms_join_all().await,
        Command::RejoinAll => cli::rooms::rejoin_all().await,
        Command::RoomMembers { id } => cli::rooms::room_members(id).await,
        Command::RoomDevices { id } => cli::rooms::room_devices(id).await,
        Command::Send {
            id,
            message,
            ephemeral,
        } => cli::timeline::send(id, message, ephemeral).await,
        Command::Reply { id, to, message } => cli::timeline::reply(id, to, message).await,
        Command::Resend { id, message, all } => cli::resend::resend(id, message, all).await,
        Command::FailedSends { id } => cli::resend::failed_sends(id).await,
        Command::Edit {
            id,
            message,
            new_body,
        } => cli::timeline::edit(id, message, new_body).await,
        Command::DeleteMessage { id, message } => cli::timeline::delete_message(id, message).await,
        Command::React { id, message, emoji } => cli::timeline::react(id, message, emoji).await,
        Command::MarkRead { id, message } => cli::timeline::mark_read(id, message).await,
        Command::SyncStream {
            idle_backoff,
            transport,
        } => cli::sync::sync_stream(idle_backoff, transport).await,
        Command::RoomCreate { name } => cli::rooms::room_create(name).await,
        Command::RoomInvite { id, users } => cli::rooms::room_invite(id, users).await,
        Command::RoomKick { id, users } => cli::rooms::room_kick(id, users).await,
        Command::DmCreate { peer } => cli::rooms::dm_create(peer).await,
        Command::DmShow { peer } => cli::rooms::dm_show(peer).await,
        Command::DmWith { peer } => cli::rooms::dm_with(peer).await,
        Command::SelfRoom => cli::rooms::self_room().await,
        Command::Devices => cli::devices::devices().await,
        Command::KeyPackages { replenish } => cli::devices::key_packages(replenish).await,
        Command::DeviceDelete { id, password_stdin } => {
            cli::devices::device_delete(id, password_stdin).await
        }
        Command::DeviceRename { id, label } => cli::devices::device_rename(id, label).await,
        Command::ProfileSet {
            display_name,
            handle,
        } => cli::config::profile_set(display_name, handle).await,
        Command::EmailUpdate { email } => cli::config::email_update(email).await,
        Command::ChangePassword => cli::auth::change_password().await,
        Command::RoomRename { id, name } => cli::rooms::room_rename(id, name).await,
        Command::RoomLeave { id } => cli::rooms::room_leave(id).await,
        Command::RoomDelete { id } => cli::rooms::room_delete(id).await,
        Command::Timeline { id, limit, before } => cli::timeline::timeline(id, limit, before).await,
        Command::Logout {
            purge_device,
            password_stdin,
        } => cli::auth::logout(purge_device, password_stdin).await,
        Command::JoinLinkGenerate { id, password } => {
            let pw = if password {
                match read_password(false) {
                    Ok(p) if !p.is_empty() => Some(p),
                    Ok(_) => {
                        eprintln!("kupferdraht-cli: empty password");
                        return ExitCode::from(2);
                    }
                    Err(e) => {
                        eprintln!("kupferdraht-cli: read password: {e}");
                        return ExitCode::from(2);
                    }
                }
            } else {
                None
            };
            cli::access::join_link_generate(id, pw).await
        }
        Command::JoinLinkRevoke { id } => cli::access::join_link_revoke(id).await,
        Command::JoinLinkCurrent { id } => cli::access::join_link_current(id).await,
        Command::RoomAccessShow { id } => cli::access::room_access_show(id).await,
        Command::RoomAccessSet { id, modes, roles } => {
            cli::access::room_access_set(id, modes, roles).await
        }
        Command::RoomAccessAllowGuests { id } => cli::access::room_access_allow_guests(id).await,
        Command::JoinViaLink {
            code,
            key,
            password,
        } => {
            let pw = if password {
                match read_password(false) {
                    Ok(p) if !p.is_empty() => Some(p),
                    Ok(_) => {
                        eprintln!("kupferdraht-cli: empty password");
                        return ExitCode::from(2);
                    }
                    Err(e) => {
                        eprintln!("kupferdraht-cli: read password: {e}");
                        return ExitCode::from(2);
                    }
                }
            } else {
                None
            };
            cli::access::join_via_link(code, key, pw).await
        }
        Command::Features { name } => cli::config::features(name).await,
        Command::Properties { key } => cli::config::properties(key).await,
        Command::SendFile { id, path, mime } => cli::assets::send_file(id, path, mime).await,
        Command::Knock { id, hot } => cli::timeline::knock(id, hot).await,
        Command::CaseSet {
            id,
            case_ref,
            external_ref,
            title,
            case_type,
            state,
            detail,
            assignee,
            due_ms,
            priority,
            authority,
            ext,
        } => {
            cli::case::case_set(cli::case::CaseSetArgs {
                room_id: id,
                case_ref,
                external_ref,
                title,
                case_type,
                state,
                detail,
                assignee,
                due_ms,
                priority,
                authority,
                ext,
            })
            .await
        }
        Command::CaseShow { id } => cli::case::case_show(id).await,
        Command::Task { action } => match action {
            TaskCommand::Add {
                id,
                title,
                description,
                assignee,
                due,
                due_ms,
                priority,
                no_fallback,
                fallback,
            } => match cli::resolve_due(due, due_ms) {
                Err(e) => Err(e),
                Ok(due_at_ms) => {
                    let draft = kupferdraht::timeline::TaskDraft {
                        title,
                        description,
                        assignee,
                        due_at_ms,
                        priority: priority.map(cli::task::PriorityArg::into_core),
                    };
                    cli::task::add(id, draft, cli::task::fallback_mode(fallback, no_fallback)).await
                }
            },
            TaskCommand::List {
                id,
                mine,
                delegated,
            } => match (id, mine, delegated) {
                (Some(id), _, _) => cli::task::list(id).await,
                (None, true, _) => cli::task::list_mine().await,
                (None, _, true) => cli::task::list_delegated().await,
                (None, false, false) => Err(kupferdraht::KupferdrahtError::InvalidInput(
                    "task list: pass --id <room>, --mine, or --delegated".into(),
                )),
            },
            TaskCommand::Move {
                id,
                task,
                after,
                before,
                no_fallback,
                fallback,
            } => {
                cli::task::move_task(
                    id,
                    task,
                    after,
                    before,
                    cli::task::fallback_mode(fallback, no_fallback),
                )
                .await
            }
            TaskCommand::Done {
                id,
                task,
                no_fallback,
                fallback,
            } => cli::task::done(id, task, cli::task::fallback_mode(fallback, no_fallback)).await,
            TaskCommand::Assign {
                id,
                task,
                user,
                no_fallback,
                fallback,
            } => {
                cli::task::assign(
                    id,
                    task,
                    user,
                    cli::task::fallback_mode(fallback, no_fallback),
                )
                .await
            }
            TaskCommand::Edit {
                id,
                task,
                title,
                description,
                assignee,
                due,
                due_ms,
                priority,
                status,
                no_fallback,
                fallback,
            } => match cli::resolve_due(due, due_ms) {
                Err(e) => Err(e),
                Ok(due_at_ms) => {
                    let patch = kupferdraht::timeline::TaskPatch {
                        title,
                        description,
                        assignee,
                        due_at_ms,
                        priority: priority.map(cli::task::PriorityArg::into_core),
                        status: status.map(cli::task::StatusArg::into_core),
                        sort_key: None,
                    };
                    cli::task::edit(
                        id,
                        task,
                        patch,
                        cli::task::fallback_mode(fallback, no_fallback),
                    )
                    .await
                }
            },
            TaskCommand::Mark { id, clear } => cli::task::mark(id, clear).await,
        },
        Command::Location {
            id,
            lat,
            lon,
            name,
            zoom,
        } => cli::timeline::location(id, lat, lon, name, zoom).await,
        Command::DownloadAsset {
            message,
            id,
            save_to,
        } => cli::assets::download_asset(message, id, save_to).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kupferdraht-cli error: {e}");
            ExitCode::from(cli::exit_code_for(&e))
        }
    }
}
