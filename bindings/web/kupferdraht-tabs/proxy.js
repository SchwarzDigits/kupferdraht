// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — the thin-client side: `KupferdrahtProxy` and the handle
// proxies mirror the wasm `Kupferdraht` / `PendingLogin` / `Client` /
// `Recovery` / `SyncHandle` classes name for name; every call is a request
// to the leader over `TabTransport`. Failover (ADR-0026 L3): an idempotent
// call in flight is re-issued to the new leader (a `Client` handle is
// re-resumed by its `userId` first, a `Recovery` handle asks its client
// again), a mutation in flight rejects with
// `LeaderChangedError` and is never retried, a subscription is
// re-established. Getters are snapshots the leader took when it handed
// the wrapper out, so `userId` / `displayName` / `email` stay synchronous.
import {
  DEFAULT_LEADER_WAIT_MS, LeaderChangedError, NoLeaderError, PROTOCOL_VERSION, REQUEST_CHANNEL, KUPFERDRAHT_TARGET,
  methodTable, remoteError, responseChannelName,
} from './protocol.js';

const METHOD_SPEC = methodTable();

/** The channels, the pending calls and the leader identity of one tab. Internal to the host. */
export class TabTransport {
  /**
   * @param {{ tabId: string, leaderWaitMs?: number,
   *           onLeader: (leaderTabId: string, epoch: number, changed: boolean) => void,
   *           onEvent: (event: string, payload: unknown) => void,
   *           onEvicted?: (notice: { leaderTabId: string, epoch: number, reason: string }) => void }} options
   */
  constructor({ tabId, leaderWaitMs = DEFAULT_LEADER_WAIT_MS, onLeader, onEvent, onEvicted = () => {} }) {
    this.tabId = tabId;
    this.leaderWaitMs = leaderWaitMs;
    this.onLeader = onLeader;
    this.onEvent = onEvent;
    /** Step 111c: the leader's instance stopped for every tab (`"device"`, `"access"`). */
    this.onEvicted = onEvicted;
    /** @type {{ tabId: string, epoch: number, key: string } | null} */
    this.leader = null;
    /** The current leader's `Kupferdraht` getters (`homeId`, `cookieLabel`, …), from its announcement. */
    this.leaderGetters = {};
    /** @type {Map<string, object>} requestId → pending call */
    this.pending = new Map();
    /** @type {Set<object>} calls made before any leader is known */
    this.waiting = new Set();
    /** @type {Map<string, object>} subscription id → live `startSync` subscription */
    this.subscriptions = new Map();
    this.closed = false;
    this.requests = new BroadcastChannel(REQUEST_CHANNEL);
    this.requests.onmessage = (e) => this.#fromChannel(e.data);
    this.responses = new BroadcastChannel(responseChannelName(tabId));
    this.responses.onmessage = (e) => this.#onResponse(e.data);
    // Ask a leader that may already exist to announce itself now, not at its next beat.
    this.requests.postMessage({ kind: 'hello', v: PROTOCOL_VERSION, tabId });
  }

  /** The leader's identity as the failover key. */
  get leaderKey() { return this.leader ? this.leader.key : null; }

  /**
   * Issue `method` on `targetProxy` (a handle proxy, or null for the leader's `Kupferdraht`).
   * Resolves with the raw response value; the caller wraps handles.
   */
  call(targetProxy, cls, method, args) {
    const spec = METHOD_SPEC.get(cls)?.get(method);
    if (!spec) return Promise.reject(new TypeError(`kupferdraht-tabs: ${cls}.${method} is not on the wasm surface`));
    return new Promise((resolve, reject) => {
      const entry = { targetProxy, cls, method, args, kind: spec.kind, resolve, reject, requestId: null, leaderKey: null, timer: null };
      if (this.closed) { reject(closedError(method)); return; }
      if (!this.leader) this.#hold(entry);
      else this.#send(entry);
    });
  }

  /** A `startSync` subscription: callbacks stay here, the leader's loop is shared. */
  subscribe(subscription) {
    this.subscriptions.set(subscription.id, subscription);
    this.#establish(subscription);
  }

  /** Send the leader the `startSync` for a subscription; a leader change mid-way is re-tried by `#failOver`. */
  #establish(subscription) {
    this.call(subscription.clientProxy, 'Client', 'startSync', [])
      .then((handle) => { subscription.remoteHandle = handle; subscription.leaderKey = this.leaderKey; })
      .catch((err) => {
        if (err && err.name !== 'LeaderChanged') console.error('kupferdraht-tabs: startSync subscription failed', err);
      });
  }

  unsubscribe(subscription) {
    this.subscriptions.delete(subscription.id);
    if (subscription.remoteHandle && subscription.leaderKey === this.leaderKey) {
      this.#post(this.#envelope(subscription.remoteHandle.$handle, 'stop', []));
    }
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    for (const entry of this.pending.values()) entry.reject(closedError(entry.method));
    this.pending.clear();
    for (const entry of this.waiting) { clearTimeout(entry.timer); entry.reject(closedError(entry.method)); }
    this.waiting.clear();
    this.subscriptions.clear();
    this.requests.close();
    this.responses.close();
  }

  #hold(entry) {
    entry.timer = setTimeout(() => {
      this.waiting.delete(entry);
      entry.reject(new NoLeaderError(entry.method, this.leaderWaitMs));
    }, this.leaderWaitMs);
    this.waiting.add(entry);
  }

  async #send(entry) {
    try {
      const target = await this.#resolveTarget(entry.targetProxy);
      if (this.closed) { entry.reject(closedError(entry.method)); return; }
      entry.requestId = crypto.randomUUID();
      entry.leaderKey = this.leaderKey;
      this.pending.set(entry.requestId, entry);
      this.#post(this.#envelope(target, entry.method, entry.args, entry.requestId));
    } catch (err) {
      entry.reject(err);
    }
  }

  /** A handle from a previous leader is dead: one that can reopen itself does (`Client`, `Recovery`), anything else cannot. */
  async #resolveTarget(targetProxy) {
    if (!targetProxy) return KUPFERDRAHT_TARGET;
    if (targetProxy.leaderKey === this.leaderKey) return targetProxy.$handle;
    if (typeof targetProxy.reopen !== 'function') throw new LeaderChangedError(`${targetProxy.cls} handle`);
    const fresh = await targetProxy.reopen();
    targetProxy.$handle = fresh.$handle;
    targetProxy.leaderKey = this.leaderKey;
    return fresh.$handle;
  }

  /** A fresh request id per attempt: a re-issue after a leader change must not hit the new leader's dedup. */
  #envelope(target, method, args, requestId = crypto.randomUUID()) {
    return { kind: 'request', v: PROTOCOL_VERSION, tabId: this.tabId, requestId, target, method, args };
  }

  #post(envelope) {
    this.requests.postMessage(envelope);
  }

  #fromChannel(m) {
    if (!m || m.v !== PROTOCOL_VERSION) return;
    if (m.kind === 'leader') {
      if (this.#stale(m.epoch)) return;
      if (m.getters && typeof m.getters === 'object') this.leaderGetters = m.getters;
      this.#leaderSeen(m.leaderTabId, m.epoch);
    } else if (m.kind === 'event') {
      // A stale event is still delivered — its data was stored — but moves no view.
      if (!this.#stale(m.epoch)) this.#leaderSeen(m.leaderTabId, m.epoch);
      for (const sub of this.subscriptions.values()) {
        if (sub.clientProxy.getters.userId === m.userId) sub.deliver(m.event, m.payload);
      }
      this.onEvent(m.event, m.payload);
    } else if (m.kind === 'evicted') {
      // Step 111c: the leader's instance stopped for every tab. Deliberately
      // not a sighting of that leader — a successor may already be known.
      this.onEvicted({ leaderTabId: m.leaderTabId, epoch: m.epoch, reason: m.reason });
    }
  }

  /**
   * Step 116: a message of an older leader than the one followed is late —
   * its last heartbeat or event, overtaken by the new leader's announcement
   * (BroadcastChannel orders nothing between senders). Epochs rise with
   * every leadership, so an older leader is never the current one.
   */
  #stale(epoch) {
    return this.leader !== null && epoch < this.leader.epoch;
  }

  #leaderSeen(leaderTabId, epoch) {
    const key = `${leaderTabId}#${epoch}`;
    if (this.leader && this.leader.key === key) { this.onLeader(leaderTabId, epoch, false); return; }
    this.leader = { tabId: leaderTabId, epoch, key };
    this.onLeader(leaderTabId, epoch, true);
    this.#failOver();
    for (const entry of this.waiting) { clearTimeout(entry.timer); this.#send(entry); }
    this.waiting.clear();
  }

  /** ADR-0026 L3: mutations in flight fail typed, idempotent calls re-issue, subscriptions re-establish. */
  #failOver() {
    const stale = [...this.pending.values()].filter((entry) => entry.leaderKey !== this.leaderKey);
    for (const entry of stale) {
      this.pending.delete(entry.requestId);
      if (entry.kind === 'mutation') entry.reject(new LeaderChangedError(entry.method));
      else if (entry.kind === 'subscription') entry.reject(new LeaderChangedError(entry.method));
      else this.#send(entry);
    }
    for (const sub of this.subscriptions.values()) {
      if (sub.leaderKey !== this.leaderKey) {
        sub.remoteHandle = null;
        this.#establish(sub);
      }
    }
  }

  #onResponse(m) {
    if (!m || m.kind !== 'response' || m.v !== PROTOCOL_VERSION) return;
    const entry = this.pending.get(m.requestId);
    if (!entry) return;
    this.pending.delete(m.requestId);
    if (m.ok) entry.resolve(m.value);
    else entry.reject(remoteError(m.error || {}));
  }
}

function closedError(method) {
  const err = new Error(`kupferdraht-tabs: the tab host is closed (${method})`);
  err.name = 'HostClosed';
  return err;
}

/** A wrapper the leader handed out: `{ $handle, $class, getters }` bound to this transport. */
class HandleProxy {
  constructor(transport, descriptor) {
    this.transport = transport;
    // `$handle` as on the wire, and deliberately not `handle`: a mirrored
    // getter of that name would shadow this field (Step 125).
    this.$handle = descriptor.$handle;
    this.cls = descriptor.$class;
    this.getters = descriptor.getters || {};
    this.leaderKey = transport.leaderKey;
  }
}

/** `parent` is the proxy the call went to — a `Recovery` handle reopens through its client. */
function wrapValue(transport, value, cls, parent) {
  if (!value || typeof value !== 'object' || !value.$handle) return value;
  if (cls === 'Client') return new ClientProxy(transport, value);
  if (cls === 'PendingLogin') return new PendingLoginProxy(transport, value);
  if (cls === 'Recovery') return new RecoveryProxy(transport, value, parent);
  return value;
}

/** Thin-client face of the wasm `Kupferdraht` (everything but the leader-only `open`). */
export class KupferdrahtProxy {
  #t;
  constructor(transport) { this.#t = transport; }
  #call(method, args, returns) {
    return this.#t.call(null, 'Kupferdraht', method, args).then((v) => wrapValue(this.#t, v, returns));
  }
  /** Always `false` on a proxy: it follows whichever leader is current; a tab's own eviction is the host's `evicted` event. */
  get evicted() { return false; }
  /** The leader's home id (Step 93) — `undefined` until a leader has announced itself. */
  get homeId() { return this.#t.leaderGetters.homeId; }
  /** The leader's login cookie label (Step 93) — `undefined` until a leader has announced itself. */
  get cookieLabel() { return this.#t.leaderGetters.cookieLabel; }
  login(apiBaseUrl, websocketUrl, title, email, password, secondFactorCode, deviceLabel) {
    return this.#call('login', [apiBaseUrl, websocketUrl, title, email, password, secondFactorCode, deviceLabel], 'PendingLogin');
  }
  initiateSso(apiBaseUrl, code, successRedirect, errorRedirect) {
    return this.#call('initiateSso', [apiBaseUrl, code, successRedirect, errorRedirect]);
  }
  initiateSsoByEmail(apiBaseUrl, email, successRedirect, errorRedirect) {
    return this.#call('initiateSsoByEmail', [apiBaseUrl, email, successRedirect, errorRedirect]);
  }
  finalizeSso(apiBaseUrl, websocketUrl, title, ssoCookie, deviceLabel) {
    return this.#call('finalizeSso', [apiBaseUrl, websocketUrl, title, ssoCookie, deviceLabel], 'PendingLogin');
  }
  resume(accountId, options) { return this.#call('resume', [accountId, options], 'Client'); }
  accounts() { return this.#call('accounts', []); }
  validateNewPin(pin) { return this.#call('validateNewPin', [pin]); }
}

/** Thin-client face of the wasm `PendingLogin`. */
export class PendingLoginProxy extends HandleProxy {
  get userId() { return this.getters.userId; }
  get email() { return this.getters.email; }
  /** Snapshot of the leader's `keys` when it handed out this login (ADR-0035). */
  get keys() { return this.getters.keys; }
  get recoveryExpected() { return this.getters.recoveryExpected; }
  registerDevice(options) {
    return this.transport.call(this, 'PendingLogin', 'registerDevice', [options]).then((v) => wrapValue(this.transport, v, 'Client'));
  }
  recover(data, pin, options) {
    return this.transport.call(this, 'PendingLogin', 'recover', [data, pin, options]).then((v) => wrapValue(this.transport, v, 'Client'));
  }
  devices() { return this.transport.call(this, 'PendingLogin', 'devices', []); }
  deleteDevice(deviceId) { return this.transport.call(this, 'PendingLogin', 'deleteDevice', [deviceId]); }
  cancel() { return this.transport.call(this, 'PendingLogin', 'cancel', []); }
}

/** Thin-client face of the wasm `Client`. */
export class ClientProxy extends HandleProxy {
  #call(method, args) { return this.transport.call(this, 'Client', method, args); }
  get userId() { return this.getters.userId; }
  get displayName() { return this.getters.displayName; }
  get email() { return this.getters.email; }
  get handle() { return this.getters.handle; }
  get deviceReplaced() { return this.getters.deviceReplaced; }
  /** A new leader's handle for this account (internal to the failover). */
  reopen() { return this.transport.call(null, 'Kupferdraht', 'resume', [this.getters.userId]); }
  /** Snapshot when the handle was returned; `startSync`'s `onStorage` reports each change (Step 113). */
  get storageState() { return this.getters.storageState ?? 'online'; }
  storageEngine() { return this.#call('storageEngine', []); }
  /** Asked of the leader each time — it changes, so it is no snapshot (Step 122). */
  pushState() { return this.#call('pushState', []); }
  setDisplayName(name) { return this.#call('setDisplayName', [name]); }
  setHandle(handle) { return this.#call('setHandle', [handle]); }
  updateEmail(email) { return this.#call('updateEmail', [email]); }
  devices() { return this.#call('devices', []); }
  deleteDevice(deviceId, password) { return this.#call('deleteDevice', [deviceId, password]); }
  renameRoom(roomId, name) { return this.#call('renameRoom', [roomId, name]); }
  deleteRoom(roomId) { return this.#call('deleteRoom', [roomId]); }
  roomAccess(roomId) { return this.#call('roomAccess', [roomId]); }
  setRoomAccess(roomId, modes, roles) { return this.#call('setRoomAccess', [roomId, modes, roles]); }
  allowGuests(roomId) { return this.#call('allowGuests', [roomId]); }
  createJoinLink(roomId, password) { return this.#call('createJoinLink', [roomId, password]); }
  revokeJoinLink(roomId) { return this.#call('revokeJoinLink', [roomId]); }
  joinViaLink(code, key, password) { return this.#call('joinViaLink', [code, key, password]); }
  startSync(onMessage, onTyping, onExpired, onSync, onDevicesChanged, onStorage) {
    if (typeof onMessage !== 'function') throw new TypeError('startSync: onMessage must be a function');
    const subscription = {
      id: crypto.randomUUID(),
      clientProxy: this,
      remoteHandle: null,
      leaderKey: null,
      onExpired,
      deliver(event, payload) {
        if (event === 'message') onMessage(payload);
        else if (event === 'typing' && onTyping) onTyping(payload);
        else if (event === 'expired' && onExpired) onExpired();
        else if (event === 'sync' && onSync) onSync(payload);
        else if (event === 'devices' && onDevicesChanged) onDevicesChanged(payload);
        else if (event === 'storage' && onStorage) onStorage(payload);
      },
    };
    this.transport.subscribe(subscription);
    return new SyncHandleProxy(this.transport, subscription);
  }
  startTyping(roomId) { return this.#call('startTyping', [roomId]); }
  stopTyping(roomId) { return this.#call('stopTyping', [roomId]); }
  syncOnce() { return this.#call('syncOnce', []); }
  rooms() { return this.#call('rooms', []); }
  sendText(roomId, body) { return this.#call('sendText', [roomId, body]); }
  messages(roomId, limit, before) { return this.#call('messages', [roomId, limit, before]); }
  sendKnock(roomId, hotKnock) { return this.#call('sendKnock', [roomId, hotKnock]); }
  sendLocation(roomId, latitude, longitude, name, zoom) {
    return this.#call('sendLocation', [roomId, latitude, longitude, name, zoom]);
  }
  sendReply(roomId, parentId, body) { return this.#call('sendReply', [roomId, parentId, body]); }
  toggleReaction(roomId, itemId, emoji) { return this.#call('toggleReaction', [roomId, itemId, emoji]); }
  editMessage(roomId, itemId, newBody) { return this.#call('editMessage', [roomId, itemId, newBody]); }
  redact(roomId, itemId) { return this.#call('redact', [roomId, itemId]); }
  markAsRead(roomId, itemId) { return this.#call('markAsRead', [roomId, itemId]); }
  createRoom(name) { return this.#call('createRoom', [name]); }
  createDm(peer) { return this.#call('createDm', [peer]); }
  members(roomId) { return this.#call('members', [roomId]); }
  roomDevices(roomId) { return this.#call('roomDevices', [roomId]); }
  ownDeviceThumbprint() { return this.#call('ownDeviceThumbprint', []); }
  invite(roomId, userIds) { return this.#call('invite', [roomId, userIds]); }
  kick(roomId, userIds) { return this.#call('kick', [roomId, userIds]); }
  leave(roomId) { return this.#call('leave', [roomId]); }
  featureConfigs() { return this.#call('featureConfigs', []); }
  fileSharing() { return this.#call('fileSharing', []); }
  logout(options) { return this.#call('logout', [options]); }
  sendFile(roomId, bytes, filename, mimeType) { return this.#call('sendFile', [roomId, bytes, filename, mimeType]); }
  downloadAsset(roomId, itemId) { return this.#call('downloadAsset', [roomId, itemId]); }
  recovery() { return this.#call('recovery', []).then((v) => wrapValue(this.transport, v, 'Recovery', this)); }
}

/** Thin-client face of the wasm `Recovery` (Step 112), bound to the client it came from. */
export class RecoveryProxy extends HandleProxy {
  #client;
  constructor(transport, descriptor, client) { super(transport, descriptor); this.#client = client; }
  #call(method, args) { return this.transport.call(this, 'Recovery', method, args); }
  /** A new leader's handle, asked of the client (internal to the failover). */
  reopen() { return this.transport.call(this.#client, 'Client', 'recovery', []); }
  state() { return this.#call('state', []); }
  enable(pin) { return this.#call('enable', [pin]); }
  changePin(currentPin, newPin) { return this.#call('changePin', [currentPin, newPin]); }
  verifyPin(pin) { return this.#call('verifyPin', [pin]); }
}

/** Thin-client face of the wasm `SyncHandle`: `stop()` ends this tab's subscription. */
export class SyncHandleProxy {
  #t;
  #subscription;
  constructor(transport, subscription) { this.#t = transport; this.#subscription = subscription; }
  stop() { this.#t.unsubscribe(this.#subscription); }
}
