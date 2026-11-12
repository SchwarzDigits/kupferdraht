// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — the browser-side scenarios behind
// `tests/step_94_leader_tab.rs`. Runs on the main thread (the thin
// client); every candidate worker is a Blob-URL module worker that plays
// the embedder's worker script in miniature: import `serveTab`, hand it a
// loader — here the stub `Kupferdraht` — and let the module do the rest. The
// module is imported relative to this file's copy beside the test glue
// (`snippets/<crate>/tests/web/` → `snippets/bindings/web/kupferdraht-tabs/`).
import { createTabHost, NoLeaderError, TabHost } from '../../../bindings/web/kupferdraht-tabs/index.js';
import { PROTOCOL_VERSION, REQUEST_CHANNEL, responseChannelName } from '../../../bindings/web/kupferdraht-tabs/protocol.js';
// Step 157: a namespace import, so the scenarios load before the constant exists.
import * as protocol from '../../../bindings/web/kupferdraht-tabs/protocol.js';
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

/**
 * Step 158: the page's own view of the election — every announcement, hello
 * and eviction on the request channel, newest last — so a timeout says
 * what the tabs were told.
 */
const sightings = [];
const sight = (text) => {
  sightings.push(`${Math.round(performance.now())}ms ${text}`);
  if (sightings.length > 40) sightings.shift();
};
const spy = new BroadcastChannel(REQUEST_CHANNEL);
spy.onmessage = (e) => {
  const m = e.data;
  if (!m || !['leader', 'hello', 'evicted'].includes(m.kind)) return;
  sight(`${m.kind} ${(m.leaderTabId ?? m.tabId ?? '').slice(0, 8)} e${m.epoch ?? '-'}`);
};

const now = () => `${Math.round(performance.now())}ms`;
const short = (id) => (id ?? 'none').slice(0, 8);
for (const kind of ['error', 'unhandledrejection']) {
  self.addEventListener(kind, (e) => sight(`page ${kind}: ${e.message ?? e.reason}`));
}

/**
 * Step 158: every event a tab host dispatches, from inside its constructor
 * on, keyed by tab — so a timeout shows whether the event came, and when.
 */
const dispatched = new Map();
const moduleDispatch = TabHost.prototype.dispatchEvent;
TabHost.prototype.dispatchEvent = function recordDispatch(event) {
  const log = dispatched.get(this.tabId) ?? [];
  const detail = event.detail && typeof event.detail === 'object' ? ` ${short(event.detail.leaderTabId)} e${event.detail.epoch ?? '-'}` : '';
  log.push(`${now()} ${event.type}${detail}`);
  dispatched.set(this.tabId, log);
  return moduleDispatch.call(this, event);
};

/** Step 158: a waiting tab's state and events, the lease's holders and queue, and the last sightings. */
async function describeElection(target, since) {
  const host = 'state' in target
    ? `tab ${short(target.tabId)} waiting since ${since} state=${target.state} leader=${short(target.leaderTabId)} epoch=${target.epoch} dispatched: ${(dispatched.get(target.tabId) ?? []).join(', ') || 'nothing'}`
    : 'not a tab host';
  const locks = await navigator.locks.query();
  const lease = (list) => list.filter((l) => l.name === 'kupferdraht:instance').length;
  return `${host}; lease held=${lease(locks.held)} queued=${lease(locks.pending)}; sightings: ${sightings.join(' | ')}`;
}

function once(target, type, predicate = () => true, timeoutMs = 8000) {
  const since = now();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(async () => {
      target.removeEventListener(type, handler);
      reject(new Error(`timeout waiting for ${type} — ${await describeElection(target, since)}`));
    }, timeoutMs);
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
      changedRooms: sync.changedRooms,
      hostEventMessageId: (await hostEvent).detail.messageId,
      otherTabMessageId: (await otherTabEvent).detail.messageId,
    };
  } finally {
    a.close();
    b.close();
  }
}

export async function electionAndFailover() {
  const reports = stubReports();
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const epochBefore = b.epoch;
    // Step 130a: a keyed send in flight is re-issued with its key — the
    // caller's, or the one the proxy added; a mutation still rejects.
    const send = client.sendText('room-1', 'in flight', { idempotencyKey: 'key-130a' });
    const unkeyed = client.sendText('room-1', 'no key given');
    const create = client.createRoom('in flight');
    const read = client.rooms();
    await wait(80); // the requests have reached A, whose stub answers slowly
    const promoted = once(b, 'leader-changed', (e) => e.detail.isSelf);
    a.close(); // terminates A's worker: the browser releases the lease
    const event = await promoted;
    const sendOutcome = await send.then((h) => h.idempotencyKey, (err) => err.name);
    const unkeyedOutcome = await unkeyed.then((h) => h.idempotencyKey, (err) => err.name);
    const createOutcome = await create.then(() => 'resolved', (err) => err.name);
    const rooms = await read;
    const keys = reports.seen.filter((m) => m.call === 'sendText').map((m) => m.idempotencyKey);
    const added = [...new Set(keys.filter((k) => k !== 'key-130a'))];
    return {
      newLeaderIsB: event.detail.leaderTabId === b.tabId,
      bState: b.state,
      sendOutcome,
      unkeyedOutcome,
      createOutcome,
      keyedReachedBothLeaders: keys.filter((k) => k === 'key-130a').length === 2,
      addedKeyKept: added.length === 1 && added[0] === unkeyedOutcome,
      roomsLength: rooms.length,
      epochIncreased: b.epoch > epochBefore,
    };
  } finally {
    reports.close();
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
    channel.postMessage({ kind: 'event', v: PROTOCOL_VERSION, leaderTabId: a.tabId, epoch: oldEpoch, event: 'sync', userId: FIXTURE_ACCOUNT, payload: { changedRooms: [], eventsDispatched: 0 } });
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

/**
 * Step 138: a read-state change made through one tab reaches every tab —
 * the caller's `startSync` subscriber (the seventh callback,
 * `onRoomsChanged`) and every host's `rooms` event, carrying the room's
 * read state. The stub's `sync` event proves the subscription is live
 * before the change is made.
 */
export async function readStateReachesEveryTab() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const live = once(b, 'sync');
    const hostA = once(a, 'rooms');
    const hostB = once(b, 'rooms');
    const subscriber = new Promise((resolve) => {
      client.startSync(() => {}, undefined, undefined, undefined, undefined, undefined, (change) => resolve(change));
    });
    await live;
    await client.markAsUnread('room-1');
    const change = await Promise.race([subscriber, wait(5000).then(() => ({ roomId: 'none within 5 s' }))]);
    const a_ = (await hostA).detail;
    const b_ = (await hostB).detail;
    return {
      subscriberRoomId: change.roomId,
      subscriberMarkedUnread: change.markedUnread === true,
      hostARoomId: a_.roomId,
      hostAMarkedUnread: a_.markedUnread === true,
      hostBRoomId: b_.roomId,
      hostBMarkedUnread: b_.markedUnread === true,
    };
  } finally {
    a.close();
    b.close();
  }
}

/** Step 139: a thin tab's `serviceToken` reaches the leader; the answer and a refusal's name cross the channel. */
export async function serviceTokenThroughATab() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const answer = await client.serviceToken('cases');
    const error = await client.serviceToken('pin').then(() => null, (err) => err);
    return { token: answer.token, expiresIn: answer.expiresIn, errorName: error?.name ?? null };
  } finally {
    a.close();
    b.close();
  }
}

/** Step 152: a tab's `refreshAccount` resolves with that tab's getters current. */
export async function ownRenameThroughATab() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const before = { displayName: client.displayName, teamName: client.teamName };
    const resolved = await client.setDisplayName('Tab Renamed');
    const after = { displayName: client.displayName, teamName: client.teamName };
    return { before, after, resolvedUndefined: resolved === undefined };
  } finally {
    a.close();
    b.close();
  }
}

export async function accountRefreshThroughATab() {
  const { a, b } = await twoTabs();
  try {
    const client = await b.kupferdraht.resume(FIXTURE_ACCOUNT);
    const before = { displayName: client.displayName, teamName: client.teamName };
    const resolved = await client.refreshAccount();
    const after = { displayName: client.displayName, teamName: client.teamName };
    return { before, after, resolvedUndefined: resolved === undefined };
  } finally {
    a.close();
    b.close();
  }
}

// ---- Step 157: tabs of different versions do not mix ----

/** `promise`, or a rejection named `Timeout` after `ms`: a call to a silent fake leader must not hang a red run. */
function within(promise, ms) {
  return Promise.race([promise, wait(ms).then(() => { const err = new Error('timeout'); err.name = 'Timeout'; throw err; })]);
}

export async function theLeaderNamesItsVersion() {
  const a = createTabHost({ worker: candidateWorker() });
  try {
    await leaderOf(a, a.tabId);
    const b = createTabHost({ worker: candidateWorker() });
    try {
      const detail = (await once(b, 'leader-changed', (e) => e.detail.leaderTabId === a.tabId)).detail;
      return {
        moduleVersion: protocol.KUPFERDRAHT_VERSION ?? null,
        own: b.version ?? null,
        leader: b.leaderVersion ?? null,
        detailLeaderVersion: detail.leaderVersion ?? null,
        relation: detail.relation ?? null,
      };
    } finally {
      b.close();
    }
  } finally {
    a.close();
  }
}

/**
 * No real leader (the page holds the lease); the page announces leaders of
 * other versions. The tab names the relation, and does not call a leader of
 * another version.
 */
export async function aThinTabDoesNotCallALeaderOfAnotherVersion() {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  await new Promise((granted) => navigator.locks.request('kupferdraht:instance', () => { granted(); return held; }));
  const idle = URL.createObjectURL(new Blob(['// a candidate that never serves\n'], { type: 'text/javascript' }));
  const host = createTabHost({ worker: () => new Worker(idle, { type: 'module' }), leaderWaitMs: 500 });
  const channel = new BroadcastChannel(REQUEST_CHANNEL);
  const requests = [];
  channel.onmessage = (e) => { if (e.data && e.data.kind === 'request' && e.data.tabId === host.tabId) requests.push(e.data); };
  const relationAfter = async (epoch, version) => {
    const seen = once(host, 'leader-changed', (e) => e.detail.epoch === epoch);
    const announcement = { kind: 'leader', v: PROTOCOL_VERSION, leaderTabId: `old-${epoch}`, epoch, getters: {} };
    if (version !== undefined) announcement.version = version;
    channel.postMessage(announcement);
    return (await seen).detail.relation ?? null;
  };
  try {
    const whenLeaderOlder = await relationAfter(1, '0.0.1');
    let callName = 'resolved';
    try { await within(host.kupferdraht.accounts(), 1000); } catch (err) { callName = err.name; }
    const whenLeaderUnknown = await relationAfter(2, undefined);
    const whenLeaderNewer = await relationAfter(3, '999.0.0');
    await wait(100);
    return { whenLeaderOlder, whenLeaderUnknown, whenLeaderNewer, callName, requestsSent: requests.length };
  } finally {
    channel.close();
    host.close();
    release();
  }
}

/** The leader answers a request of another version, or of none, with `VersionMismatch`. */
export async function theLeaderRefusesARequestOfAnotherVersion() {
  const a = createTabHost({ worker: candidateWorker() });
  try {
    await leaderOf(a, a.tabId);
    const fakeTab = `fake-${crypto.randomUUID()}`;
    const answers = [];
    const mine = new BroadcastChannel(responseChannelName(fakeTab));
    mine.onmessage = (e) => answers.push(e.data);
    const requests = new BroadcastChannel(REQUEST_CHANNEL);
    const ask = (requestId, version) => {
      const m = { kind: 'request', v: PROTOCOL_VERSION, tabId: fakeTab, requestId, target: 'kupferdraht', method: 'accounts', args: [] };
      if (version !== undefined) m.version = version;
      requests.postMessage(m);
    };
    ask('r-other', '0.0.1');
    ask('r-none', undefined);
    ask('r-same', protocol.KUPFERDRAHT_VERSION);
    await wait(600);
    mine.close();
    requests.close();
    const answer = (id) => {
      const m = answers.find((r) => r.requestId === id);
      if (!m) return 'none';
      return m.ok ? 'ok' : (m.error && m.error.name) || 'error';
    };
    return { other: answer('r-other'), none: answer('r-none'), same: answer('r-same') };
  } finally {
    a.close();
  }
}

/** A candidate whose open finds newer data reports it once and stops queuing. */
export async function aCandidateRefusedByNewerDataStops() {
  const reports = stubReports();
  const host = createTabHost({ worker: candidateWorker({ storage: { stubFail: 'IncompatibleStore' }, retryMs: 100 }) });
  const errors = [];
  host.addEventListener('error', (e) => errors.push(e.detail.name));
  try {
    await wait(1500);
    return { opens: reports.seen.filter((r) => r.call === 'open').length, errors };
  } finally {
    reports.close();
    host.close();
  }
}

/** A worker handed a wasm package of another version than the module's does not lead. */
export async function aWorkerOfAnotherWasmVersionDoesNotLead() {
  const reports = stubReports();
  const host = createTabHost({ worker: candidateWorker({ version: '0.0.1', retryMs: 100 }) });
  const errors = [];
  host.addEventListener('error', (e) => errors.push(e.detail.name));
  try {
    await wait(1500);
    return { opens: reports.seen.filter((r) => r.call === 'open').length, errors, state: host.state };
  } finally {
    reports.close();
    host.close();
  }
}

/**
 * Step 158: a timeout names the election it waited in. A leader waits for an
 * event no tab dispatches; the error must name the tab, its state, what it
 * dispatched, the lease and the sightings.
 */
export async function aTimeoutNamesTheElection() {
  const a = createTabHost({ worker: candidateWorker() });
  try {
    await leaderOf(a, a.tabId);
    const message = await once(a, 'never-dispatched', () => true, 50).then(() => '', (err) => err.message);
    const tab = a.tabId.slice(0, 8);
    return {
      namesTheTab: message.includes(`tab ${tab}`),
      namesItsState: message.includes(`state=leader leader=${tab}`),
      namesWhatItDispatched: message.includes(`leader-changed ${tab}`),
      namesTheLease: message.includes('lease held=1'),
      namesTheSightings: message.includes(`leader ${tab} e`),
    };
  } finally {
    a.close();
  }
}

