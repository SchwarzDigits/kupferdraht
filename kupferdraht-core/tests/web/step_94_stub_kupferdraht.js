// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — a stub of the wasm `Kupferdraht` surface for the browser
// tests of `bindings/web/kupferdraht-tabs/`. It answers from fixtures instead of
// a Wire backend, and its `open` performs the one thing the module's
// lease hand-off relies on: it **steals** `kupferdraht:instance` from the
// candidate's JavaScript and keeps the lock for the worker's lifetime,
// exactly like `storage/browser.rs` does for the real instance, and it
// reports eviction the same way (the request rejects with `AbortError`).
//
// Step 111c: the stub reports what it was called with on the channel
// `stub:report` (`open` with its options, `resume` with its options), and
// the opened stub of the leader stops for a reason posted on
// `stub:control` (`{ evict: "device" }`), as the Rust instance does when
// another browser takes its databases over.

export const FIXTURE_ACCOUNT = 'stub@example.test';
export const FIXTURE_ROOMS = Object.freeze([
  { roomId: 'room-1', name: 'Fixture room' },
  { roomId: 'room-2', name: 'Second room' },
]);
const FIXTURE_MESSAGES = {
  'room-1': [1, 2, 3].map((n) => ({
    itemId: `m-${n}`, senderUserId: 'peer@example.test', sentAtMs: n, kind: 'text',
    body: `message ${n}`, replyTo: null, reactions: [], edited: false, redacted: false,
    asset: null, expiresAtMs: null,
  })),
};
/// Reads and mutations take this long, so a call can be in flight while
/// the leader is terminated (the failover scenario).
export const SLOW_MS = 400;

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const hold = () => new Promise(() => {});
const report = (message) => {
  const channel = new BroadcastChannel('stub:report');
  channel.postMessage(message);
  channel.close();
};

export class StubKupferdraht {
  static async open(options, onEvicted) {
    if (!options || options.takeOver !== true) {
      throw new Error('stub: the leader bootstrap must open with takeOver (the lease hand-off)');
    }
    report({ call: 'open', options: { permitHttp: options.permitHttp, takeOver: options.takeOver, storage: options.storage } });
    let grant;
    const granted = new Promise((resolve) => { grant = resolve; });
    let stopped = false;
    const stop = (reason) => {
      if (stopped) return;
      stopped = true;
      if (onEvicted) onEvicted(reason);
    };
    navigator.locks
      .request('kupferdraht:instance', { mode: 'exclusive', steal: true }, (lock) => { grant(lock !== null); return hold(); })
      .catch(() => stop('tab'));
    if (!(await granted)) throw new Error('stub: the lease was not granted');
    const control = new BroadcastChannel('stub:control');
    control.onmessage = (e) => { if (e.data && e.data.evict) stop(e.data.evict); };
    return new StubKupferdraht();
  }
  get evicted() { return false; }
  get homeId() { return 'stub-home-0123456789abcdef'; }
  get cookieLabel() { return 'kupferdraht-stub-home-0123456789abcdef'; }
  async accounts() {
    return [{ accountId: FIXTURE_ACCOUNT, server: 'https://wire.example.test', handle: 'stub', displayName: 'Stub', lastActiveMs: 0 }];
  }
  async resume(accountId, options) {
    report({ call: 'resume', accountId, options: options ?? null });
    if (accountId !== FIXTURE_ACCOUNT) throw new Error(`unknown account: ${accountId}`);
    return new StubClient(accountId);
  }
  async login() { throw new Error('stub: no Wire backend behind the fixture'); }
  async finalizeSso() { throw new Error('stub: no Wire backend behind the fixture'); }
  async initiateSso() { return 'https://wire.example.test/sso'; }
  async initiateSsoByEmail() { return 'https://wire.example.test/sso'; }
  validateNewPin(pin) { return pin.length < 6 ? 'tooShort' : 'ok'; }
}

/// Step 112b: the recovery of the fixture account. `verifyPin` fails as the
/// wasm one does for a wrong PIN: a named error with its detail.
export class StubRecovery {
  async state() { return { state: 'enabled', exportedAt: 1 }; }
  async enable() { return new Uint8Array([1, 2, 3]); }
  async changePin() { return new Uint8Array([4, 5, 6]); }
  async verifyPin() {
    const err = new Error('stub: another PIN');
    err.name = 'WrongPin';
    err.attemptsRemaining = 3;
    throw err;
  }
}

export class StubClient {
  constructor(userId) { this._userId = userId; }
  get userId() { return this._userId; }
  get displayName() { return 'Stub'; }
  async storageEngine() { return { engine: 'stub', version: '0', cipher: 'none', plaintextHeaderSize: 0 }; }
  async rooms() { await delay(SLOW_MS); return FIXTURE_ROOMS.map((r) => ({ ...r })); }
  async messages(roomId, limit) {
    const all = FIXTURE_MESSAGES[roomId] ?? [];
    return (limit ? all.slice(-limit) : all).map((m) => ({ ...m }));
  }
  async sendText(roomId, body) { await delay(SLOW_MS); return { messageId: `sent-${roomId}-${body.length}`, sentAtMs: Date.now() }; }
  startSync(onMessage, onTyping, onExpired, onSync, onDevicesChanged, onStorage) {
    const timer = setTimeout(() => {
      onMessage({ roomId: 'room-1', senderUserId: 'peer@example.test', messageId: 'live-1', body: 'live', replyToId: null, receivedAtMs: 1, expiresAtMs: null });
      if (onSync) onSync({ roomsUpdated: 1, eventsDispatched: 1 });
      // Step 113: the storage state, as the Rust loop reports an outage.
      if (onStorage) onStorage('reconnecting');
    }, 50);
    return { stop() { clearTimeout(timer); } };
  }
  async syncOnce() { return { roomsUpdated: 0, eventsDispatched: 0 }; }
  recovery() { return new StubRecovery(); }
}
