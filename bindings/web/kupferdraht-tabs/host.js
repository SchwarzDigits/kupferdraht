// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — the tab host: runs in every tab's page, keeps one
// candidate worker alive (queued for the lease, or leading), follows the
// leader announcements, and exposes the thin-client proxy. The lock is the
// source of truth for leadership; the heartbeat only raises a hint.
//
//   const host = createTabHost({ worker: () => new Worker('./tabs-worker.js', { type: 'module' }) });
//   host.addEventListener('leader-changed', (e) => render(e.detail));
//   const accounts = await host.kupferdraht.accounts();
import { DEFAULT_HEARTBEAT_MS, DEFAULT_LEADER_WAIT_MS, MISSED_HEARTBEATS } from './protocol.js';
import { KupferdrahtProxy, TabTransport } from './proxy.js';

/**
 * @param {{ worker: () => Worker, tabId?: string, heartbeatMs?: number, leaderWaitMs?: number }} options
 * @returns {TabHost}
 */
export function createTabHost(options = {}) {
  if (typeof options.worker !== 'function') {
    throw new TypeError("createTabHost: options.worker must construct the tab's dedicated worker (a module worker that calls serveTab)");
  }
  return new TabHost(options);
}

/**
 * Events: `leader-changed` `{ leaderTabId, epoch, isSelf }`, `evicted`
 * `{ epoch, reason }` — `"tab"`: this tab led and another tab took over
 * (the host re-queued a fresh candidate); `"device"` / `"access"` (Step
 * 111c): the leader's instance stopped for every tab — another browser took
 * the account's databases over, or the storage no longer admits the user;
 * every tab hears it once, and the next leader serves again —
 * `leader-unresponsive` (missed heartbeats — a hint, not a verdict), `error`
 * `{ name, message }` (the candidate's bootstrap failed), and every leader
 * broadcast: `message`, `typing`, `expired`, `sync`.
 */
export class TabHost extends EventTarget {
  #worker = null;
  #spawn;
  #transport;
  #state = 'electing';
  #leaderTabId = null;
  #epoch = 0;
  #heartbeatMs;
  #watchdog = null;
  #unresponsive = false;
  #closed = false;

  constructor({ worker, tabId = crypto.randomUUID(), heartbeatMs = DEFAULT_HEARTBEAT_MS, leaderWaitMs = DEFAULT_LEADER_WAIT_MS }) {
    super();
    this.tabId = tabId;
    this.#spawn = worker;
    this.#heartbeatMs = heartbeatMs;
    this.#transport = new TabTransport({
      tabId,
      leaderWaitMs,
      onLeader: (leaderTabId, epoch, changed) => this.#onLeader(leaderTabId, epoch, changed),
      onEvent: (event, payload) => this.dispatchEvent(new CustomEvent(event, { detail: payload })),
      onEvicted: (notice) => this.#onLeaderEvicted(notice),
    });
    /** The thin-client face of the leader's `Kupferdraht` — same method names as the wasm class. */
    this.kupferdraht = new KupferdrahtProxy(this.#transport);
    this.#startCandidate();
  }

  /** `electing` (no leader known yet) | `leader` (this tab's worker holds the instance) | `thin`. */
  get state() { return this.#state; }
  get leaderTabId() { return this.#leaderTabId; }
  get epoch() { return this.#epoch; }

  /** The ADR-0023 override for a hung leader: this tab's candidate steals the lease now. */
  takeOver() {
    if (this.#closed || this.#state === 'leader') return;
    this.#worker?.postMessage({ type: 'take-over' });
  }

  /** Terminate this tab's candidate (a leader releases the lease) and stop following. */
  close() {
    if (this.#closed) return;
    this.#closed = true;
    clearTimeout(this.#watchdog);
    this.#worker?.terminate();
    this.#worker = null;
    this.#transport.close();
  }

  #startCandidate() {
    const worker = this.#spawn();
    this.#worker = worker;
    worker.addEventListener('message', (e) => this.#fromWorker(e.data));
    worker.addEventListener('error', (e) => this.#emit('error', { name: 'WorkerError', message: e.message || 'worker error' }));
    worker.postMessage({ type: 'init', tabId: this.tabId, lastEpoch: this.#epoch, heartbeatMs: this.#heartbeatMs });
  }

  #fromWorker(m) {
    if (!m || typeof m !== 'object' || this.#closed) return;
    if (m.type === 'leader') this.#onLeader(this.tabId, m.epoch, true);
    else if (m.type === 'evicted') this.#onEvicted(m);
    else if (m.type === 'error') this.#emit('error', { name: m.name, message: m.message });
  }

  /**
   * ADR-0023's contract, kept by the host: terminate the evicted worker, then queue again.
   * The new leader's announcement may arrive before this eviction (Step 100): a tab that
   * already follows another leader stays `thin` — no later `leader-changed` would fix it.
   */
  #onEvicted(m) {
    this.#worker?.terminate();
    this.#worker = null;
    const followsAnother = this.#leaderTabId !== null && this.#leaderTabId !== this.tabId;
    this.#state = followsAnother ? 'thin' : 'electing';
    this.#emit('evicted', { epoch: this.#epoch, reason: m.reason ?? 'tab' });
    this.#startCandidate();
  }

  /**
   * Step 111c: the leader's notice that its instance stopped for every tab.
   * The leader's own tab hears it from its worker instead; a thin tab keeps
   * its candidate queued, and the next leader serves.
   */
  #onLeaderEvicted({ leaderTabId, epoch, reason }) {
    if (this.#closed || leaderTabId === this.tabId) return;
    this.#emit('evicted', { epoch, reason });
  }

  #onLeader(leaderTabId, epoch, changed) {
    this.#armWatchdog();
    const isSelf = leaderTabId === this.tabId;
    // The worker's own `leader` post and the channel announcement both arrive; one event.
    if (!changed || (leaderTabId === this.#leaderTabId && epoch === this.#epoch)) return;
    this.#leaderTabId = leaderTabId;
    this.#epoch = epoch;
    this.#state = isSelf ? 'leader' : 'thin';
    // The candidate must announce a higher epoch than any leader this tab saw.
    if (!isSelf) this.#worker?.postMessage({ type: 'epoch', epoch });
    this.#emit('leader-changed', { leaderTabId, epoch, isSelf });
  }

  #armWatchdog() {
    clearTimeout(this.#watchdog);
    this.#unresponsive = false;
    this.#watchdog = setTimeout(() => {
      if (this.#closed || this.#unresponsive) return;
      this.#unresponsive = true;
      this.#emit('leader-unresponsive', { leaderTabId: this.#leaderTabId, epoch: this.#epoch });
    }, MISSED_HEARTBEATS * this.#heartbeatMs);
  }

  #emit(type, detail) {
    this.dispatchEvent(new CustomEvent(type, { detail }));
  }
}
