// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — the wire contract between thin tabs and the leader.
//
// Everything crosses `BroadcastChannel`s by structured clone: requests,
// events and the leader announcement on the origin-wide REQUEST_CHANNEL,
// responses on the requesting tab's own channel (`responseChannelName`).
// The method table below is the JS twin of the FFI-parity rule: it lists
// every public method of the wasm `Kupferdraht` / `PendingLogin` / `Client` /
// `Recovery` / `SyncHandle` classes except the leader-only `Kupferdraht.open`, with its
// failover class — `kupferdraht-core/tests/step_94_tab_proxy_parity.rs` fails
// on any drift between this table, `proxy.js`, `index.d.ts` and the
// `#[wasm_bindgen]` surface.

/** Envelope version; a leader ignores requests of another version. */
export const PROTOCOL_VERSION = 1;
/** Requests, events and leader announcements — one channel per origin. */
export const REQUEST_CHANNEL = 'kupferdraht:tabs';
/** Responses go to the requesting tab only. */
export function responseChannelName(tabId) {
  return `${REQUEST_CHANNEL}/${tabId}`;
}
/** The ADR-0023 lease; the lock queue on it is the election. */
export const LEASE_NAME = 'kupferdraht:instance';
/** The request target that means "the leader's `Kupferdraht` instance". */
export const KUPFERDRAHT_TARGET = 'kupferdraht';
export const DEFAULT_HEARTBEAT_MS = 2000;
/** Missed heartbeats before the host raises `leader-unresponsive` (a hint). */
export const MISSED_HEARTBEATS = 3;
/** How long a call waits for a leader to announce itself before `NoLeader`. */
export const DEFAULT_LEADER_WAIT_MS = 10000;
/** A candidate whose bootstrap failed re-enters the queue after this. */
export const DEFAULT_RETRY_MS = 5000;

/**
 * `[class, method, kind, returns]` — `kind` is the failover class:
 * `idempotent` calls are re-issued to the new leader, `mutation` calls in
 * flight reject with `LeaderChanged` and are never retried, `subscription`
 * calls are re-established. `returns` names the wrapper class a call
 * hands back (the leader returns a handle; the proxy wraps it).
 */
export const METHODS = Object.freeze([
  ["Kupferdraht", "login", "mutation", "PendingLogin"],
  ["Kupferdraht", "initiateSso", "idempotent", null],
  ["Kupferdraht", "initiateSsoByEmail", "idempotent", null],
  ["Kupferdraht", "finalizeSso", "mutation", "PendingLogin"],
  ["Kupferdraht", "resume", "idempotent", "Client"],
  ["Kupferdraht", "accounts", "idempotent", null],
  ["Kupferdraht", "validateNewPin", "idempotent", null],
  ["PendingLogin", "registerDevice", "mutation", "Client"],
  ["PendingLogin", "recover", "mutation", "Client"],
  ["PendingLogin", "devices", "idempotent", null],
  ["PendingLogin", "deleteDevice", "mutation", null],
  ["PendingLogin", "cancel", "mutation", null],
  ["Client", "storageEngine", "idempotent", null],
  ["Client", "pushState", "idempotent", null],
  ["Client", "startSync", "subscription", "SyncHandle"],
  ["Client", "startTyping", "mutation", null],
  ["Client", "stopTyping", "mutation", null],
  ["Client", "syncOnce", "idempotent", null],
  ["Client", "rooms", "idempotent", null],
  ["Client", "sendText", "mutation", null],
  ["Client", "messages", "idempotent", null],
  ["Client", "sendKnock", "mutation", null],
  ["Client", "sendLocation", "mutation", null],
  ["Client", "sendReply", "mutation", null],
  ["Client", "toggleReaction", "mutation", null],
  ["Client", "editMessage", "mutation", null],
  ["Client", "redact", "mutation", null],
  ["Client", "markAsRead", "mutation", null],
  ["Client", "createRoom", "mutation", null],
  ["Client", "createDm", "mutation", null],
  ["Client", "members", "idempotent", null],
  ["Client", "roomDevices", "idempotent", null],
  ["Client", "ownDeviceThumbprint", "idempotent", null],
  ["Client", "invite", "mutation", null],
  ["Client", "kick", "mutation", null],
  ["Client", "renameRoom", "mutation", null],
  ["Client", "deleteRoom", "mutation", null],
  ["Client", "roomAccess", "idempotent", null],
  ["Client", "setRoomAccess", "mutation", null],
  ["Client", "allowGuests", "mutation", null],
  ["Client", "createJoinLink", "mutation", null],
  ["Client", "revokeJoinLink", "mutation", null],
  ["Client", "joinViaLink", "mutation", null],
  ["Client", "leave", "mutation", null],
  ["Client", "setDisplayName", "mutation", null],
  ["Client", "setHandle", "mutation", null],
  ["Client", "updateEmail", "mutation", null],
  ["Client", "devices", "idempotent", null],
  ["Client", "deleteDevice", "mutation", null],
  ["Client", "featureConfigs", "idempotent", null],
  ["Client", "fileSharing", "idempotent", null],
  ["Client", "logout", "mutation", null],
  ["Client", "sendFile", "mutation", null],
  ["Client", "downloadAsset", "idempotent", null],
  ["Client", "recovery", "idempotent", "Recovery"],
  ["Recovery", "state", "idempotent", null],
  ["Recovery", "enable", "mutation", null],
  ["Recovery", "changePin", "mutation", null],
  ["Recovery", "verifyPin", "idempotent", null],
  ["SyncHandle", "stop", "idempotent", null],
]);

/** `[class, getter]` — snapshotted into the handle when the leader returns it. */
export const GETTERS = Object.freeze([
  ["Kupferdraht", "evicted"],
  ["Kupferdraht", "homeId"],
  ["Kupferdraht", "cookieLabel"],
  ["PendingLogin", "userId"],
  ["PendingLogin", "email"],
  ["PendingLogin", "keys"],
  ["PendingLogin", "recoveryExpected"],
  ["Client", "userId"],
  ["Client", "displayName"],
  ["Client", "email"],
  ["Client", "handle"],
  ["Client", "deviceReplaced"],
  ["Client", "storageState"],
]);

/** Events the leader broadcasts (the wasm `startSync` callbacks + the per-pass summary). */
export const LEADER_EVENTS = Object.freeze(['message', 'typing', 'expired', 'sync', 'devices', 'storage']);

/** `class → method → { kind, returns }`, derived from METHODS. */
export function methodTable() {
  const table = new Map();
  for (const [cls, method, kind, returns] of METHODS) {
    if (!table.has(cls)) table.set(cls, new Map());
    table.get(cls).set(method, { kind, returns });
  }
  return table;
}

/** `class → getter[]`, derived from GETTERS. */
export function getterTable() {
  const table = new Map();
  for (const [cls, getter] of GETTERS) {
    if (!table.has(cls)) table.set(cls, []);
    table.get(cls).push(getter);
  }
  return table;
}

/** Thrown for a mutation that was in flight when the leader changed. */
export class LeaderChangedError extends Error {
  constructor(method) {
    super(`kupferdraht-tabs: the leader changed while ${method} was in flight; the call was not retried `
      + '(a mutation must never reach the backend twice) — check the timeline, then retry deliberately');
    this.name = 'LeaderChanged';
    this.method = method;
  }
}

/** Thrown when no leader announced itself within the wait. */
export class NoLeaderError extends Error {
  constructor(method, waitMs) {
    super(`kupferdraht-tabs: no leader tab answered within ${waitMs} ms (${method}) — the origin has no open Kupferdraht instance yet`);
    this.name = 'NoLeader';
    this.method = method;
  }
}

/** What a recovery error carries beyond its name (ADR-0035), when known. */
const ERROR_DETAILS = Object.freeze(['reason', 'attemptsRemaining', 'retryAfter']);

/** Rebuild an error the leader reported, keeping its name, message and detail. */
export function remoteError({ name, message, ...detail }) {
  const err = new Error(message);
  err.name = name || 'Error';
  for (const key of ERROR_DETAILS) if (detail[key] !== undefined) err[key] = detail[key];
  return err;
}

/** The leader's or the wrapper's error, as a clonable record. */
export function describeError(err) {
  const record = { name: (err && err.name) || 'Error', message: String((err && err.message) ?? err) };
  for (const key of ERROR_DETAILS) if (err && err[key] !== undefined) record[key] = err[key];
  return record;
}
