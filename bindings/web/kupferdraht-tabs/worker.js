// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — the candidate: runs in every tab's dedicated
// worker, queues for the ADR-0023 lease and, on the grant, becomes the
// leader by the hand-off `Kupferdraht.open({ takeOver: true })` — a lock is not
// reentrant within one agent, so the candidate's own JavaScript grant must
// be *stolen* by the Rust instance rather than released and re-taken
// (a release would let the next queued tab in first; a steal preempts the
// queue). The embedder's worker script imports this module and hands it a
// loader for the wasm package — this file never knows where `pkg/` lives:
//
//   import init, { Kupferdraht } from '<wasm-pack out-dir>/kupferdraht.js';
//   import { serveTab } from '../kupferdraht-tabs/worker.js';
//   serveTab({ load: async () => { await init(); return Kupferdraht; }, permitHttp: true });
//
// `storage` (Step 111c) is `Kupferdraht.open`'s option as it is: the local mode
// when absent, `{ mode: 'server', url, tokenExchangeUrl }` for the account
// databases on a storage server.
import { createLeaderDispatcher } from './leader.js';
import { DEFAULT_RETRY_MS, KUPFERDRAHT_VERSION, LEASE_NAME, PROTOCOL_VERSION, REQUEST_CHANNEL } from './protocol.js';

/**
 * Step 157: failures a retry cannot cure — data newer than this build, or a
 * wasm package of another release than this module. The candidate reports
 * them once and leaves the queue.
 */
const FINAL_FAILURES = new Set(['IncompatibleStore', 'VersionMismatch']);

/**
 * `version` (Step 157) is the wasm package's `kupferdraht_version()`, or a
 * function returning it — called after `load`, so only a tab about to lead
 * loads the wasm: when it is not this module's `KUPFERDRAHT_VERSION`, the
 * candidate never leads.
 *
 * @param {{ load: () => Promise<{ open: Function }>, permitHttp?: boolean,
 *           storage?: object, logLevel?: string, onLog?: Function,
 *           heartbeatMs?: number, retryMs?: number, version?: string | (() => string) }} options
 */
export function serveTab({ load, permitHttp = false, storage, logLevel, onLog, heartbeatMs, retryMs = DEFAULT_RETRY_MS, version } = {}) {
  if (typeof load !== 'function') throw new TypeError('serveTab: options.load must resolve to the Kupferdraht class');
  const state = { tabId: null, lastEpoch: 0, abort: null, leader: null, bootstrapping: false, evicted: false, stopped: false };
  const post = (m) => self.postMessage(m);
  const report = (err) => post({ type: 'error', name: (err && err.name) || 'Error', message: String((err && err.message) ?? err) });

  // Candidates track the newest epoch so the one that wins announces a higher one.
  const announcements = new BroadcastChannel(REQUEST_CHANNEL);
  announcements.onmessage = (e) => {
    const m = e.data;
    if (m && (m.kind === 'leader' || m.kind === 'event') && typeof m.epoch === 'number') {
      state.lastEpoch = Math.max(state.lastEpoch, m.epoch);
    }
  };

  self.addEventListener('message', (e) => {
    const m = e.data;
    if (!m || typeof m !== 'object') return;
    if (m.type === 'init') init(m);
    else if (m.type === 'take-over') takeOver();
    else if (m.type === 'epoch') state.lastEpoch = Math.max(state.lastEpoch, Number(m.epoch) || 0);
  });

  function init({ tabId, lastEpoch }) {
    if (state.tabId) return;
    state.tabId = tabId;
    state.lastEpoch = Math.max(state.lastEpoch, Number(lastEpoch) || 0);
    // Ask the current leader (if any) to announce itself now, so the epoch
    // this candidate would succeed is known before it can win.
    announcements.postMessage({ kind: 'hello', v: PROTOCOL_VERSION, tabId });
    queue();
  }

  /** Step 157: the wasm package and this module must be one release. */
  function checkWasmVersion() {
    const wasm = typeof version === 'function' ? version() : version;
    if (wasm === undefined || wasm === KUPFERDRAHT_VERSION) return;
    const err = new Error(`kupferdraht-tabs: the wasm package is Kupferdraht ${wasm}, this module ${KUPFERDRAHT_VERSION}; ship both from one release`);
    err.name = 'VersionMismatch';
    throw err;
  }

  /** Step 157: report a failure no retry cures, once, and leave the queue for good. */
  function stopFor(err) {
    state.stopped = true;
    report(err);
  }

  /** One queued request; the grant runs the bootstrap while the JS still holds the lease. */
  function queue() {
    if (state.evicted || state.stopped) return;
    state.abort = new AbortController();
    navigator.locks
      .request(LEASE_NAME, { mode: 'exclusive', signal: state.abort.signal }, () => bootstrap())
      .catch((err) => {
        // Expected: our own hand-off (the Rust instance stole the lease from
        // this callback) and our own `take-over` abort both reject with AbortError.
        if (err && err.name === 'AbortError') return;
        report(err);
      });
    post({ type: 'queued' });
  }

  /** The ADR-0023 override: leave the queue and steal the lease now. */
  function takeOver() {
    if (state.leader || state.bootstrapping || state.evicted || state.stopped) return;
    if (state.abort) state.abort.abort();
    bootstrap();
  }

  async function bootstrap() {
    if (state.leader || state.bootstrapping || state.evicted || state.stopped) return;
    state.bootstrapping = true;
    try {
      const Kupferdraht = await load();
      checkWasmVersion();
      const kupferdraht = await Kupferdraht.open({ permitHttp, takeOver: true, storage, logLevel, onLog }, onEvicted);
      const epoch = state.lastEpoch + 1;
      state.lastEpoch = epoch;
      state.leader = createLeaderDispatcher(kupferdraht, { tabId: state.tabId, epoch, heartbeatMs });
      post({ type: 'leader', epoch });
    } catch (err) {
      if (err && FINAL_FAILURES.has(err.name)) { stopFor(err); return; }
      report(err);
      // The lease goes back to the queue with this callback's settlement;
      // re-enter it after a pause so a transient failure does not strand the tab.
      setTimeout(queue, retryMs);
    } finally {
      state.bootstrapping = false;
    }
  }

  /**
   * The instance stopped (ADR-0023, ADR-0035): stop serving; the host
   * terminates this worker. `"tab"`: another tab took over and leads now.
   * `"device"` (another browser took the databases over) and `"access"`
   * (the storage no longer admits the user) concern every tab: this leader
   * tells them first, in a message of its own kind that moves no tab's view
   * of the leader (Step 111c).
   */
  function onEvicted(reason = 'tab') {
    state.evicted = true;
    const epoch = state.leader ? state.leader.epoch : state.lastEpoch;
    if (state.leader) state.leader.stop();
    state.leader = null;
    if (reason !== 'tab') {
      announcements.postMessage({ kind: 'evicted', v: PROTOCOL_VERSION, leaderTabId: state.tabId, epoch, reason });
    }
    announcements.close();
    post({ type: 'evicted', reason });
  }
}
