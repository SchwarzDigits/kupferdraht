// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — hand-written types for kupferdraht-tabs. The method
// names and getters mirror the wasm `Kupferdraht` / `PendingLogin` / `Client` /
// `Recovery` / `SyncHandle` classes (`docs/bindings.md`, "The WASM / web surface");
// `kupferdraht-core/tests/step_94_tab_proxy_parity.rs` fails on any drift.

/** How a call behaves when the leader changes while it is in flight. */
export type FailoverKind = 'idempotent' | 'mutation' | 'subscription';
export type WrapperClass = 'PendingLogin' | 'Client' | 'Recovery' | 'SyncHandle';

/** `[class, method, kind, returns]` — the wasm surface minus the leader-only `Kupferdraht.open`. */
export const METHODS: ReadonlyArray<readonly [string, string, FailoverKind, WrapperClass | null]>;
/** `[class, getter]` — snapshotted into a handle when the leader returns it. */
export const GETTERS: ReadonlyArray<readonly [string, string]>;
/** Events the leader broadcasts: the wasm `startSync` callbacks plus the per-pass summary. */
export const LEADER_EVENTS: ReadonlyArray<'message' | 'typing' | 'expired' | 'sync' | 'devices' | 'storage'>;
export const PROTOCOL_VERSION: number;
export const REQUEST_CHANNEL: string;
export const LEASE_NAME: string;
export function responseChannelName(tabId: string): string;

/** A mutation was in flight when the leader changed; it was not retried. */
export class LeaderChangedError extends Error {
  readonly name: 'LeaderChanged';
  readonly method: string;
}
/** No leader announced itself within the wait. */
export class NoLeaderError extends Error {
  readonly name: 'NoLeader';
  readonly method: string;
}

export interface TabHostOptions {
  /** Constructs this tab's dedicated module worker — a script that calls `serveTab`. */
  worker: () => Worker;
  /** Defaults to `crypto.randomUUID()`. */
  tabId?: string;
  /** Leader heartbeat period; default 2000 ms. */
  heartbeatMs?: number;
  /** How long a call waits for a leader before `NoLeaderError`; default 10 000 ms. */
  leaderWaitMs?: number;
}

export interface LeaderChangedDetail { leaderTabId: string; epoch: number; isSelf: boolean }
export interface ErrorDetail { name: string; message: string }

/**
 * What a call rejects with: `name` is the `KupferdrahtError` variant (Step 110).
 * The recovery errors carry their detail (ADR-0035), each only when known.
 */
export interface KupferdrahtError extends Error {
  /** `InvalidRecoveryData`: not recovery data, an unsupported version, another account's data, or a replaced device's (Step 120). */
  readonly reason?: 'notRecoveryData' | 'unsupported' | 'otherAccount' | 'oldDevice';
  /** `WrongPin`: the attempts the PIN service has left; absent for a PIN checked locally (`changePin`). */
  readonly attemptsRemaining?: number;
  /** `TooManyAttempts`: seconds until the PIN service takes attempts again; `RateLimited`: seconds the backend asks to wait (Step 122). */
  readonly retryAfter?: number;
}

export type TabState = 'electing' | 'leader' | 'thin';

export interface TabHostEventMap {
  'leader-changed': CustomEvent<LeaderChangedDetail>;
  'leader-unresponsive': CustomEvent<{ leaderTabId: string | null; epoch: number }>;
  /**
   * `"tab"`: this tab led and another tab took over. `"device"` (another
   * browser took the account's databases over) / `"access"` (the storage no
   * longer admits the user): the leader's instance stopped for every tab;
   * the next leader serves again, and a `resume` meets `HeldElsewhere` /
   * `StorageAccessDenied` (Step 111c).
   */
  'evicted': CustomEvent<{ epoch: number; reason: 'tab' | 'device' | 'access' }>;
  'error': CustomEvent<ErrorDetail>;
  'message': CustomEvent<MessageEvent_>;
  'typing': CustomEvent<TypingEvent_>;
  'expired': CustomEvent<null>;
  'sync': CustomEvent<SyncSummary>;
  /** Step 113 (ADR-0038): a client's storage state changed — `"reconnecting"` while the storage server is unreachable. */
  'storage': CustomEvent<'online' | 'reconnecting'>;
}

export class TabHost extends EventTarget {
  readonly tabId: string;
  readonly state: TabState;
  readonly leaderTabId: string | null;
  readonly epoch: number;
  /** The thin-client face of the leader's `Kupferdraht`. */
  readonly kupferdraht: KupferdrahtProxy;
  /** The ADR-0023 override for a hung leader: this tab steals the lease now. */
  takeOver(): void;
  /** Terminate this tab's candidate (a leader releases the lease) and stop following. */
  close(): void;
  addEventListener<K extends keyof TabHostEventMap>(type: K, listener: (ev: TabHostEventMap[K]) => void, options?: boolean | AddEventListenerOptions): void;
  addEventListener(type: string, listener: EventListenerOrEventListenerObject, options?: boolean | AddEventListenerOptions): void;
  removeEventListener<K extends keyof TabHostEventMap>(type: K, listener: (ev: TabHostEventMap[K]) => void, options?: boolean | EventListenerOptions): void;
  removeEventListener(type: string, listener: EventListenerOrEventListenerObject, options?: boolean | EventListenerOptions): void;
}
export function createTabHost(options: TabHostOptions): TabHost;

export interface ServeTabOptions {
  /** Resolves to the wasm `Kupferdraht` class after `init()` of the wasm package. */
  load: () => Promise<{ open: (options: unknown, onEvicted?: (reason: 'tab' | 'device' | 'access') => void) => Promise<unknown> }>;
  /** Passed to `Kupferdraht.open`; dev proxies only. */
  permitHttp?: boolean;
  /** Passed to `Kupferdraht.open` (Step 111c): the local mode when absent, or the account databases on a storage server. */
  storage?: { mode: 'local' } | { mode: 'server'; url: string; tokenExchangeUrl: string; pinServiceUrl?: string };
  /** Passed to `Kupferdraht.open` (Step 117): the least severe level `onLog` receives; default `'warn'`. */
  logLevel?: 'error' | 'warn' | 'info' | 'debug' | 'trace';
  /** Passed to `Kupferdraht.open` (Step 117): Kupferdraht's log events, in the leader's worker. Kupferdraht logs nowhere else. */
  onLog?: (record: LogRecord) => void;
  heartbeatMs?: number;
  /** Pause before a failed bootstrap re-enters the queue; default 5000 ms. */
  retryMs?: number;
}
/** One log event of Kupferdraht (Step 117). */
export interface LogRecord { level: 'error' | 'warn' | 'info' | 'debug' | 'trace'; target: string; message: string }
/** Worker side: queue for the lease, lead on the grant, serve every tab. */
export function serveTab(options: ServeTabOptions): void;

// ── payloads (the wasm view objects, camelCase) ────────────────────────────
export interface AccountEntry { accountId: string; server: string; handle: string | null; displayName: string | null; lastActiveMs: number }
export interface StorageEngineInfo { engine: string; version: string; cipher: string; plaintextHeaderSize: number; location: 'server' | 'local' }
export interface SyncSummary { roomsUpdated: number; eventsDispatched: number }
export interface Room { roomId: string; name: string }
export interface MessageHandle { messageId: string; sentAtMs: number }
export interface Reaction { emoji: string; count: number; isSelf: boolean }
export type DeliveryStatus = 'sending' | 'submitted' | 'received' | 'failed';
export interface AssetMeta { filename: string | null; mime: string; size: number }
export interface TimelineItem {
  itemId: string; senderUserId: string; sentAtMs: number; kind: string; body: string | null;
  replyTo: string | null; reactions: Reaction[]; edited: boolean; redacted: boolean;
  asset: AssetMeta | null; expiresAtMs: number | null; deliveryStatus: DeliveryStatus;
  /** Set for `kind === 'knock'`. */
  hotKnock: boolean | null;
  /** Set for `kind === 'location'`. */
  location: { latitude: number; longitude: number; name: string | null; zoom: number | null } | null;
}
export interface Member { userId: string; isSelf: boolean; displayName: string | null; handle: string | null; role: string }
export interface MessageEvent_ {
  roomId: string; senderUserId: string; messageId: string; body: string | null;
  replyToId: string | null; receivedAtMs: number; expiresAtMs: number | null;
}
export interface TypingEvent_ { roomId: string; typingUsers: string[] }
/** A server-administered feature config (Step 98d); `configJson` stays raw. */
export interface FeatureConfig { name: string; status: string | null; configJson: string; lastSyncedAtMs: number }
/** The team's file-sharing policy (Step 98d). */
export interface FileSharing { policy: 'disabled' | 'enabled' }
/** A conversation's access modes and roles (Step 98b). */
export interface RoomAccess { modes: string[]; roles: string[] }
/** A conversation's reusable guest link (Step 98b). */
export interface JoinLink { url: string; passwordProtected: boolean }
/** A registered device, field for field `core::DeviceInfo` (Step 98a). */
export interface DeviceInfo {
  deviceId: string; class: string | null; label: string | null; model: string | null;
  registeredAt: string | null; lastActive: string | null; isSelf: boolean;
}
/** One MLS leaf of a room (Step 95d); `thumbprint` is Wire's "MLS Thumbprint". */
export interface RoomDevice {
  userId: string | null; deviceId: string | null;
  credential: 'basic' | 'x509'; thumbprint: string | null;
}
/** A room's device set changed against the last persisted set (Step 95d). */
export interface DevicesChange { roomId: string; added: RoomDevice[]; removed: RoomDevice[] }

// ── the proxies (same names as the wasm classes) ──────────────────────────
export class KupferdrahtProxy {
  /** Always `false` on a proxy; a tab's own eviction is the host's `evicted` event. */
  readonly evicted: boolean;
  /** The leader's home id (Step 93); `undefined` until a leader has announced itself. */
  readonly homeId: string | undefined;
  /** The leader's login cookie label (Step 93); `undefined` until a leader has announced itself. */
  readonly cookieLabel: string | undefined;
  login(apiBaseUrl: string, websocketUrl: string, title: string, email: string, password: string, secondFactorCode?: string, deviceLabel?: string): Promise<PendingLoginProxy>;
  initiateSso(apiBaseUrl: string, code: string, successRedirect: string, errorRedirect: string): Promise<string>;
  initiateSsoByEmail(apiBaseUrl: string, email: string, successRedirect: string, errorRedirect: string): Promise<string>;
  finalizeSso(apiBaseUrl: string, websocketUrl: string, title: string, ssoCookie: string, deviceLabel?: string): Promise<PendingLoginProxy>;
  /** `takeOver` takes the account's databases over from another browser that holds them (`HeldElsewhere` otherwise; Step 111c). */
  resume(accountId: string, options?: { takeOver?: boolean }): Promise<ClientProxy>;
  accounts(): Promise<AccountEntry[]>;
  /**
   * Whether `pin` may become a recovery PIN (ADR-0035): at least 6 characters after Unicode NFC, any characters,
   * no upper limit a user would reach; `"tooCommon"` refuses trivial PINs such as `123456`. Offer a text field.
   */
  validateNewPin(pin: string): Promise<'ok' | 'tooShort' | 'tooCommon'>;
}

export class PendingLoginProxy {
  readonly userId: string;
  readonly email: string;
  /** Whether this browser holds the account's keys (ADR-0035); a snapshot taken at login. */
  readonly keys: 'present' | 'missing';
  /** With `keys` `"missing"`: a browser device of the account is registered, so recovery data exists somewhere. */
  readonly recoveryExpected: boolean;
  /**
   * Opens the account's databases (the login opened none). With `keys` `"missing"` only `{ newSetup: true }`
   * proceeds (a first setup); otherwise `RecoveryDataRequired`. `takeOver` takes the databases over from another
   * browser that holds them (`HeldElsewhere` otherwise; Step 111c).
   */
  registerDevice(options?: { newSetup?: boolean; takeOver?: boolean }): Promise<ClientProxy>;
  /**
   * With `keys` `"missing"`: restore the account from recovery data, then register as `registerDevice` does.
   * A failure before the keys are recovered (`WrongPin`, `TooManyAttempts`, `InvalidRecoveryData`,
   * `ServiceUnavailable`) leaves nothing; after it the keys stay — on `HeldElsewhere` continue with
   * `registerDevice({ takeOver: true })`, no second PIN attempt.
   */
  recover(data: Uint8Array, pin: string, options?: { takeOver?: boolean }): Promise<ClientProxy>;
  /** Device-limit recovery: list, free a slot, then `registerDevice` again. */
  devices(): Promise<DeviceInfo[]>;
  deleteDevice(deviceId: string): Promise<void>;
  /** Abandon the login; a first setup it began leaves nothing behind. */
  cancel(): Promise<void>;
}

export class ClientProxy {
  readonly userId: string | undefined;
  readonly displayName: string;
  /** The account's email address; `undefined` when it has none on record. */
  readonly email: string | undefined;
  /** The account's handle; `undefined` when it has none on record (Step 125). */
  readonly handle: string | undefined;
  /** The recorded device was gone, so this login registered a new one (older messages are gone). */
  readonly deviceReplaced: boolean;
  /** Snapshot when the handle was returned; `startSync`'s `onStorage` reports each change (Step 113). */
  readonly storageState: 'online' | 'reconnecting';
  storageEngine(): Promise<StorageEngineInfo | undefined>;
  /** Whether sync runs on push: an open socket, polling without one, or no sync loop running. */
  pushState(): Promise<'connected' | 'polling' | 'stopped'>;
  setDisplayName(name: string): Promise<void>;
  setHandle(handle: string): Promise<void>;
  /** Wire mails a verification link; the address changes once confirmed. */
  updateEmail(email: string): Promise<void>;
  devices(): Promise<DeviceInfo[]>;
  /** Wire requires the account password to re-authenticate. */
  deleteDevice(deviceId: string, password: string): Promise<void>;
  renameRoom(roomId: string, name: string): Promise<Room>;
  /** Team-admin delete for everyone; an already-gone room counts as success. */
  deleteRoom(roomId: string): Promise<void>;
  roomAccess(roomId: string): Promise<RoomAccess>;
  setRoomAccess(roomId: string, modes: string[], roles: string[]): Promise<Room>;
  /** Prerequisite for `createJoinLink` — a fresh room permits no codes. */
  allowGuests(roomId: string): Promise<Room>;
  createJoinLink(roomId: string, password?: string): Promise<JoinLink>;
  revokeJoinLink(roomId: string): Promise<void>;
  /** An MLS external commit — `code`/`key` come from the link's query; the link names the conversation. */
  joinViaLink(code: string, key: string, password?: string): Promise<Room>;
  /** A subscription to the leader's one sync loop; callbacks run in this tab. */
  startSync(onMessage: (m: MessageEvent_) => void, onTyping?: (t: TypingEvent_) => void, onExpired?: () => void, onSync?: (s: SyncSummary) => void, onDevicesChanged?: (c: DevicesChange) => void, onStorage?: (state: 'online' | 'reconnecting') => void): SyncHandleProxy;
  startTyping(roomId: string): Promise<void>;
  stopTyping(roomId: string): Promise<void>;
  syncOnce(): Promise<SyncSummary>;
  rooms(): Promise<Room[]>;
  sendText(roomId: string, body: string): Promise<MessageHandle>;
  /** `before` is a message id: only items strictly older than it are returned. */
  messages(roomId: string, limit?: number, before?: string): Promise<TimelineItem[]>;
  sendKnock(roomId: string, hotKnock: boolean): Promise<MessageHandle>;
  sendLocation(roomId: string, latitude: number, longitude: number, name?: string, zoom?: number): Promise<MessageHandle>;
  sendReply(roomId: string, parentId: string, body: string): Promise<MessageHandle>;
  toggleReaction(roomId: string, itemId: string, emoji: string): Promise<MessageHandle>;
  editMessage(roomId: string, itemId: string, newBody: string): Promise<MessageHandle>;
  redact(roomId: string, itemId: string): Promise<MessageHandle>;
  markAsRead(roomId: string, itemId: string): Promise<void>;
  createRoom(name: string): Promise<Room>;
  createDm(peer: string): Promise<Room>;
  members(roomId: string): Promise<Member[]>;
  roomDevices(roomId: string): Promise<RoomDevice[]>;
  ownDeviceThumbprint(): Promise<string>;
  invite(roomId: string, userIds: string[]): Promise<Room>;
  kick(roomId: string, userIds: string[]): Promise<Room>;
  leave(roomId: string): Promise<void>;
  featureConfigs(): Promise<FeatureConfig[]>;
  fileSharing(): Promise<FileSharing>;
  /**
   * Without options the keys stay, as natively. `purgeDevice` deletes the device at the backend (with the password)
   * and, in the server mode, its databases there; `purgeLocal` forgets the account in this browser (keys, local
   * databases, registry entry — recovery data still restores it). After a purge the client is closed.
   */
  logout(options?: { purgeDevice?: boolean; password?: string; purgeLocal?: boolean }): Promise<void>;
  sendFile(roomId: string, bytes: Uint8Array, filename: string, mimeType: string): Promise<MessageHandle>;
  downloadAsset(roomId: string, itemId: string): Promise<Uint8Array>;
  /** The account's recovery data (ADR-0035). */
  recovery(): Promise<RecoveryProxy>;
}

/** `exportedAt`: when the current recovery data was made (milliseconds since the epoch). */
export type RecoveryState = { state: 'disabled' } | { state: 'enabled'; exportedAt: number };

/** The account's recovery data (Step 112, ADR-0035); the local mode, or no `pinServiceUrl`, has none. */
export class RecoveryProxy {
  state(): Promise<RecoveryState>;
  /** The first recovery data — only while disabled; the PIN must pass `validateNewPin`. */
  enable(pin: string): Promise<Uint8Array>;
  /** New recovery data under `newPin`; `currentPin` is checked locally (`WrongPin`, no attempt spent). */
  changePin(currentPin: string, newPin: string): Promise<Uint8Array>;
  /** Whether `pin` is the recovery PIN — local, no attempt spent. */
  verifyPin(pin: string): Promise<boolean>;
}

export class SyncHandleProxy {
  /** Ends this tab's subscription; the leader stops its loop when the last subscriber stops. */
  stop(): void;
}

/** Leader side, exported for tooling; `serveTab` calls it on the grant. */
export function createLeaderDispatcher(kupferdraht: unknown, options: { tabId: string; epoch: number; heartbeatMs?: number }): { tabId: string; epoch: number; stop(): void };
