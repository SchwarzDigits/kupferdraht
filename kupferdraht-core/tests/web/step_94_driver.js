// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — the browser-side scenarios behind
// `tests/step_94_leader_tab.rs`. Runs on the main thread (the thin
// client); every candidate worker is a Blob-URL module worker that plays
// the embedder's worker script in miniature: import `serveTab`, hand it a
// loader — here the stub `Kupferdraht` — and let the module do the rest. The
// module is imported relative to this file's copy beside the test glue
// (`snippets/<crate>/tests/web/` → `snippets/bindings/web/kupferdraht-tabs/`).
import { createTabHost, NoLeaderError } from '../../../bindings/web/kupferdraht-tabs/index.js';
import { PROTOCOL_VERSION, REQUEST_CHANNEL, responseChannelName } from '../../../bindings/web/kupferdraht-tabs/protocol.js';
import { FIXTURE_ACCOUNT } from './step_94_stub_kupferdraht.js';

const workerModuleUrl = new URL('../../../bindings/web/kupferdraht-tabs/worker.js', import.meta.url).href;
const stubUrl = new URL('./step_94_stub_kupferdraht.js', import.meta.url).href;

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** The embedder's worker script, as a Blob-URL module; `extra` joins the `serveTab` options. */
function candidateWorker(extra = {}) {
  const source = `import { serveTab } from ${JSON.stringify(workerModuleUrl)};
import { StubKupferdraht } from ${JSON.stringify(stubUrl)};
serveTab({ load: async () => StubKupferdraht, permitHttp: true, ...${JSON.stringify(extra)} });
`;
  const url = URL.createObjectURL(new Blob([source], { type: 'text/javascript' }));
  return () => new Worker(url, { type: 'module' });
}

function once(target, type, predicate = () => true, timeoutMs = 8000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { target.removeEventListener(type, handler); reject(new Error(`timeout waiting for ${type}`)); }, timeoutMs);
    const handler = (event) => {
      if (!predicate(event)) return;
      clearTimeout(timer);
      target.removeEventListener(type, handler);
      resolve(event);
    };
    target.addEventListener(type, handler);
  });
}

/** Resolve once `host` sees `tabId` as the leader. */
async function leaderOf(host, tabId) {
  if (host.leaderTabId === tabId) return;
  await once(host, 'leader-changed', (e) => e.detail.leaderTabId === tabId);
}

/** Whether anyone holds `kupferdraht:instance` right now (`ifAvailable` probe). */
async function leaseHeld() {
  let held = false;
  await navigator.locks.request('kupferdraht:instance', { ifAvailable: true }, (lock) => { held = lock === null; });
  return held;
}

/** Two tabs on one page: A leads, B is thin. */
async function twoTabs() {
  const a = createTabHost({ worker: candidateWorker() });
  await leaderOf(a, a.tabId);
  const b = createTabHost({ worker: candidateWorker() });
  await leaderOf(b, a.tabId);
  return { a, b };
}

export async function rpcRoundTrip() {
  const { a, b } = await twoTabs();
  try {
    const accounts = await b.kupferdraht.accounts();
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const rooms = await client.rooms();
    const messages = await client.messages('room-1', 2);

    // Raw protocol: the same request twice from a foreign tab → answered
    // once, on that tab's response channel only — never on B's.
    const fakeTab = `fake-${crypto.randomUUID()}`;
    const mine = [];
    const onB = [];
    const mineChannel = new BroadcastChannel(responseChannelName(fakeTab));
    mineChannel.onmessage = (e) => mine.push(e.data);
    const bChannel = new BroadcastChannel(responseChannelName(b.tabId));
    bChannel.onmessage = (e) => onB.push(e.data);
    const requests = new BroadcastChannel(REQUEST_CHANNEL);
    const envelope = { kind: 'request', v: PROTOCOL_VERSION, tabId: fakeTab, requestId: 'dup-1', target: 'kupferdraht', method: 'accounts', args: [] };
    requests.postMessage(envelope);
    requests.postMessage(envelope);
    // A forged response with an unknown id on B's channel must be ignored.
    bChannel.postMessage({ kind: 'response', v: PROTOCOL_VERSION, requestId: 'nobody-asked', ok: true, value: 42 });
    await wait(300);
    const again = await b.kupferdraht.accounts();
    mineChannel.close();
    bChannel.close();
    requests.close();
    return {
      accountsLength: accounts.length,
      accountId: accounts[0].accountId,
      userId: client.userId,
      displayName: client.displayName,
      roomsLength: rooms.length,
      messagesLength: messages.length,
      lastMessageId: messages[messages.length - 1].itemId,
      dedupResponses: mine.filter((r) => r.requestId === 'dup-1').length,
      strayResponsesOnB: onB.filter((r) => r.requestId === 'dup-1').length,
      survivedForgedResponse: again.length === 1,
      homeId: b.kupferdraht.homeId,
      cookieLabel: b.kupferdraht.cookieLabel,
    };
  } finally {
    a.close();
    b.close();
  }
}

export async function eventFanOut() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const hostEvent = once(b, 'message');
    const otherTabEvent = once(a, 'message');
    let message;
    let sync;
    const both = new Promise((resolve) => {
      let seen = 0;
      const tick = () => { if (++seen === 2) resolve(); };
      client.startSync((m) => { message = m; tick(); }, undefined, undefined, (s) => { sync = s; tick(); });
    });
    await Promise.race([both, wait(5000).then(() => { throw new Error('no fan-out within 5 s'); })]);
    return {
      messageId: message.messageId,
      roomsUpdated: sync.roomsUpdated,
      hostEventMessageId: (await hostEvent).detail.messageId,
      otherTabMessageId: (await otherTabEvent).detail.messageId,
    };
  } finally {
    a.close();
    b.close();
  }
}

export async function electionAndFailover() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const epochBefore = b.epoch;
    const send = client.sendText('room-1', 'in flight');
    const read = client.rooms();
    await wait(80); // both requests have reached A, whose stub answers slowly
    const promoted = once(b, 'leader-changed', (e) => e.detail.isSelf);
    a.close(); // terminates A's worker: the browser releases the lease
    const event = await promoted;
    const sendOutcome = await send.then(() => 'resolved', (err) => err.name);
    const rooms = await read;
    return {
      newLeaderIsB: event.detail.leaderTabId === b.tabId,
      bState: b.state,
      sendOutcome,
      roomsLength: rooms.length,
      epochIncreased: b.epoch > epochBefore,
    };
  } finally {
    b.close();
  }
}

/**
 * Step 112b: a thin tab's recovery calls reach the leader; a recovery error
 * keeps its name and detail across the channel; `validateNewPin` answers.
 */
export async function recoveryThroughATab() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const recovery = await client.recovery();
    const state = await recovery.state();
    const data = await recovery.enable('482913');
    const error = await recovery.verifyPin('000000').then(() => null, (err) => err);
    return {
      state: state.state,
      exportedAt: state.exportedAt,
      isBytes: data instanceof Uint8Array,
      dataLength: data.length,
      errorName: error?.name ?? null,
      attemptsRemaining: error?.attemptsRemaining ?? null,
      pinCheck: await b.kupferdraht.validateNewPin('12345'),
    };
  } finally {
    a.close();
    b.close();
  }
}

/** Step 112b: a `Recovery` handle outlives its leader — it reopens through its client on the next one. */
export async function recoveryHandleSurvivesTheLeader() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const recovery = await client.recovery();
    const promoted = once(b, 'leader-changed', (e) => e.detail.isSelf);
    a.close();
    await promoted;
    const state = await recovery.state();
    return { state: state.state };
  } finally {
    b.close();
  }
}

export async function noLeaderFailsTyped() {
  const idle = URL.createObjectURL(new Blob(['// a candidate that never serves\n'], { type: 'text/javascript' }));
  const host = createTabHost({ worker: () => new Worker(idle, { type: 'module' }), leaderWaitMs: 300 });
  try {
    await host.kupferdraht.accounts();
    return { name: 'resolved', isNoLeader: false };
  } catch (err) {
    return { name: err.name, isNoLeader: err instanceof NoLeaderError };
  } finally {
    host.close();
  }
}

/**
 * Whether the lease is still held once the previous scenario had up to
 * `ms` to let go of it: closing a host terminates its worker, and the
 * browser releases the worker's lock a moment later — the scenario before
 * this one may have closed its hosts just now.
 */
async function leaseHeldAfterSettling(ms = 2000) {
  const until = Date.now() + ms;
  while (await leaseHeld()) {
    if (Date.now() > until) return true;
    await wait(50);
  }
  return false;
}

export async function handOffHoldsLease() {
  const heldBefore = await leaseHeldAfterSettling();
  const a = createTabHost({ worker: candidateWorker() });
  await leaderOf(a, a.tabId);
  const heldWhileLeading = await leaseHeld();
  const state = a.state;
  a.close();
  await wait(150);
  const heldAfterClose = await leaseHeld();
  return { heldBefore, heldWhileLeading, state, heldAfterClose };
}

export async function takeOverOverride() {
  const { a, b } = await twoTabs();
  try {
    const aEvicted = once(a, 'evicted');
    const bLeads = once(b, 'leader-changed', (e) => e.detail.isSelf);
    b.takeOver();
    await bLeads;
    await aEvicted;
    await leaderOf(a, b.tabId);
    const accounts = await a.kupferdraht.accounts();
    return { bLeads: b.state === 'leader', aState: a.state, aFollowsB: a.leaderTabId === b.tabId, accountsLength: accounts.length };
  } finally {
    a.close();
    b.close();
  }
}

/**
 * Step 116: an evicted leader's last heartbeat or event can arrive after the
 * new leader's announcement (BroadcastChannel orders nothing between
 * senders). Played deterministically: after B took over, the page posts an
 * announcement and an event in A's name with A's old epoch. No tab may move
 * back to A.
 */
export async function aStaleLeaderMessageMovesNoTab() {
  const { a, b } = await twoTabs();
  try {
    const oldEpoch = a.epoch;
    const aEvicted = once(a, 'evicted');
    const bLeads = once(b, 'leader-changed', (e) => e.detail.isSelf);
    b.takeOver();
    await bLeads;
    await aEvicted;
    await leaderOf(a, b.tabId);
    // Every view change from here on: the stale messages must cause none.
    const movedBack = [];
    const watch = (host, name) => host.addEventListener('leader-changed', (e) => {
      if (e.detail.leaderTabId === a.tabId) movedBack.push(`${name} → the evicted leader`);
    });
    watch(a, 'A');
    watch(b, 'B');
    let staleEventDelivered = false;
    b.addEventListener('sync', () => { staleEventDelivered = true; });
    const channel = new BroadcastChannel(REQUEST_CHANNEL);
    channel.postMessage({ kind: 'leader', v: PROTOCOL_VERSION, leaderTabId: a.tabId, epoch: oldEpoch, getters: {} });
    channel.postMessage({ kind: 'event', v: PROTOCOL_VERSION, leaderTabId: a.tabId, epoch: oldEpoch, event: 'sync', userId: FIXTURE_ACCOUNT, payload: { roomsUpdated: 0, eventsDispatched: 0 } });
    channel.close();
    await wait(300);
    return {
      movedBack,
      staleEventDelivered,
      bState: b.state,
      bFollows: b.leaderTabId,
      aFollows: a.leaderTabId,
      aTabId: a.tabId,
      bTabId: b.tabId,
      newEpochHigher: b.epoch > oldEpoch,
    };
  } finally {
    a.close();
    b.close();
  }
}

/** A candidate worker the scenario drives by hand (Step 100). */
class ScriptedWorker extends EventTarget {
  postMessage() {}
  terminate() {}
  send(data) { this.dispatchEvent(new MessageEvent('message', { data })); }
}

/**
 * The takeover race, forced into its bad order: this tab leads, another
 * tab's announcement arrives first, and only then this tab's worker
 * reports the eviction.
 */
export async function evictionAfterTheNewLeaderIsKnown() {
  const workers = [];
  const host = createTabHost({ worker: () => { const w = new ScriptedWorker(); workers.push(w); return w; } });
  const announcements = new BroadcastChannel(REQUEST_CHANNEL);
  try {
    const leads = once(host, 'leader-changed', (e) => e.detail.isSelf);
    workers[0].send({ type: 'leader', epoch: 1 });
    await leads;
    const followsB = once(host, 'leader-changed', (e) => e.detail.leaderTabId === 'tab-b');
    announcements.postMessage({ kind: 'leader', v: PROTOCOL_VERSION, leaderTabId: 'tab-b', epoch: 2, getters: {} });
    await followsB;
    const evicted = once(host, 'evicted');
    workers[0].send({ type: 'evicted' });
    await evicted;
    return { state: host.state, leaderTabId: host.leaderTabId, candidates: workers.length };
  } finally {
    announcements.close();
    host.close();
  }
}

/** What the stubs report on `stub:report` (Step 111c). */
function stubReports() {
  const seen = [];
  const channel = new BroadcastChannel('stub:report');
  channel.onmessage = (e) => seen.push(e.data);
  return { seen, close: () => channel.close() };
}

const SERVER_STORAGE = Object.freeze({
  mode: 'server',
  url: 'wss://storage.example.test/v1/ws',
  tokenExchangeUrl: 'https://exchange.example.test',
});

/** Step 111c: `serveTab({ storage })` reaches the leader's `Kupferdraht.open`. */
export async function storageReachesTheLeadersOpen() {
  const reports = stubReports();
  const a = createTabHost({ worker: candidateWorker({ storage: SERVER_STORAGE }) });
  try {
    await leaderOf(a, a.tabId);
    await wait(150);
    const open = reports.seen.find((m) => m.call === 'open');
    return {
      opened: Boolean(open),
      takeOver: open?.options?.takeOver === true,
      storageMode: open?.options?.storage?.mode ?? null,
      storageUrl: open?.options?.storage?.url ?? null,
      tokenExchangeUrl: open?.options?.storage?.tokenExchangeUrl ?? null,
    };
  } finally {
    reports.close();
    a.close();
  }
}

/** Step 111c: a thin tab's `resume(id, { takeOver })` reaches the leader with the option. */
export async function resumeCarriesTheTakeOver() {
  const reports = stubReports();
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT, { takeOver: true });
    await wait(150);
    const call = reports.seen.find((m) => m.call === 'resume');
    return { userId: client.userId, takeOver: call?.options?.takeOver === true };
  } finally {
    reports.close();
    a.close();
    b.close();
  }
}

/**
 * Step 111c: another browser took the databases over — the leader's
 * instance stops with `"device"`. Every tab's host fires `evicted` with the
 * reason once; the hosts re-queue and a leader serves again.
 */
export async function aDeviceEvictionReachesEveryTab() {
  const { a, b } = await twoTabs();
  const control = new BroadcastChannel('stub:control');
  const seenA = [];
  const seenB = [];
  a.addEventListener('evicted', (e) => seenA.push(e.detail));
  b.addEventListener('evicted', (e) => seenB.push(e.detail));
  try {
    const aEvicted = once(a, 'evicted');
    const bEvicted = once(b, 'evicted');
    control.postMessage({ evict: 'device' });
    await Promise.all([aEvicted, bEvicted]);
    await wait(500);
    const accounts = await b.kupferdraht.accounts();
    return {
      aReasons: seenA.map((d) => d.reason),
      bReasons: seenB.map((d) => d.reason),
      accountsLength: accounts.length,
    };
  } finally {
    control.close();
    a.close();
    b.close();
  }
}

/** Step 111c: a tab takeover is `"tab"`, and only the evicted tab hears of it. */
export async function aTabTakeOverIsReportedToTheEvictedTabOnly() {
  const { a, b } = await twoTabs();
  const seenB = [];
  b.addEventListener('evicted', (e) => seenB.push(e.detail));
  try {
    const aEvicted = once(a, 'evicted');
    const bLeads = once(b, 'leader-changed', (e) => e.detail.isSelf);
    b.takeOver();
    await bLeads;
    const detail = (await aEvicted).detail;
    await wait(300);
    return { aReason: detail.reason ?? null, bEvictions: seenB.length };
  } finally {
    a.close();
    b.close();
  }
}

/**
 * Step 113 (ADR-0038): the storage state reaches a thin tab — its
 * `startSync` subscriber's sixth callback `onStorage` and every host's
 * `storage` event.
 */
export async function storageStateReachesEveryTab() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const hostA = once(a, 'storage');
    const hostB = once(b, 'storage');
    const subscriber = new Promise((resolve) => {
      client.startSync(() => {}, undefined, undefined, undefined, undefined, (state) => resolve(state));
    });
    const state = await Promise.race([subscriber, wait(5000).then(() => 'none within 5 s')]);
    return {
      subscriberState: state,
      hostAState: (await hostA).detail,
      hostBState: (await hostB).detail,
    };
  } finally {
    a.close();
    b.close();
  }
}
