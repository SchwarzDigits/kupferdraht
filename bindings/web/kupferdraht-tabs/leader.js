// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — the leader side: dispatch requests from every tab
// of the origin to the one `Kupferdraht` instance, keep the wrapper objects it
// hands out as handles, broadcast the sync loop's events, and announce
// leadership on a heartbeat. Runs inside the elected worker (`worker.js`).
import {
  DEFAULT_HEARTBEAT_MS, LEADER_EVENTS, PROTOCOL_VERSION, REQUEST_CHANNEL, KUPFERDRAHT_TARGET,
  describeError, getterTable, methodTable, responseChannelName,
} from './protocol.js';

/** Request ids remembered per tab for dedup (a duplicate is answered once). */
const SEEN_PER_TAB = 512;
/** Response channels kept open (one per tab that ever asked). */
const RESPONDERS = 128;

/**
 * @param {object} kupferdraht the leader's open `Kupferdraht` instance
 * @param {{ tabId: string, epoch: number, heartbeatMs?: number }} options
 */
export function createLeaderDispatcher(kupferdraht, { tabId, epoch, heartbeatMs = DEFAULT_HEARTBEAT_MS }) {
  if (!kupferdraht || typeof kupferdraht.accounts !== 'function') throw new TypeError('createLeaderDispatcher: an open Kupferdraht instance is required');
  if (typeof tabId !== 'string' || !tabId) throw new TypeError('createLeaderDispatcher: tabId is required');
  const methods = methodTable();
  const getters = getterTable();
  const requests = new BroadcastChannel(REQUEST_CHANNEL);
  /** @type {Map<string, BroadcastChannel>} tabId → response channel */
  const responders = new Map();
  /** @type {Map<string, { object: object, cls: string }>} handleId → wrapper */
  const handles = new Map([[KUPFERDRAHT_TARGET, { object: kupferdraht, cls: 'Kupferdraht' }]]);
  /** @type {Map<string, { set: Set<string>, order: string[] }>} tabId → seen request ids */
  const seen = new Map();
  /** @type {Map<string, { handle: object, subscribers: Set<string> }>} client handle → live sync */
  const syncs = new Map();
  let handleCounter = 0;
  let stopped = false;

  // The leader's own `Kupferdraht` getters ride on the announcement, so a thin
  // tab's `KupferdrahtProxy` can answer them synchronously like the wasm class.
  const kupferdrahtGetters = () => {
    const snapshot = {};
    for (const getter of getters.get('Kupferdraht') ?? []) snapshot[getter] = kupferdraht[getter];
    return snapshot;
  };
  const announce = () => requests.postMessage({ kind: 'leader', v: PROTOCOL_VERSION, leaderTabId: tabId, epoch, getters: kupferdrahtGetters() });
  const beat = setInterval(announce, heartbeatMs);
  announce();

  requests.onmessage = (e) => {
    const m = e.data;
    if (!m || stopped) return;
    if (m.kind === 'hello') { announce(); return; }
    if (m.kind !== 'request' || m.v !== PROTOCOL_VERSION || duplicate(m)) return;
    dispatch(m).then((response) => respond(m.tabId, response));
  };

  function duplicate({ tabId: from, requestId }) {
    let memo = seen.get(from);
    if (!memo) { memo = { set: new Set(), order: [] }; seen.set(from, memo); }
    if (memo.set.has(requestId)) return true;
    memo.set.add(requestId);
    memo.order.push(requestId);
    if (memo.order.length > SEEN_PER_TAB) memo.set.delete(memo.order.shift());
    return false;
  }

  function respond(to, response) {
    if (stopped) return;
    let channel = responders.get(to);
    if (!channel) {
      if (responders.size >= RESPONDERS) {
        const [oldest, stale] = responders.entries().next().value;
        stale.close();
        responders.delete(oldest);
      }
      channel = new BroadcastChannel(responseChannelName(to));
      responders.set(to, channel);
    }
    channel.postMessage(response);
  }

  async function dispatch({ requestId, target, method, args }) {
    try {
      const entry = handles.get(target);
      if (!entry) throw named('UnknownHandle', `kupferdraht-tabs: no handle ${target} on this leader (it may have changed)`);
      const spec = methods.get(entry.cls)?.get(method);
      if (!spec) throw named('UnknownMethod', `kupferdraht-tabs: ${entry.cls}.${method} is not on the wasm surface`);
      const value = spec.kind === 'subscription'
        ? subscribe(target, entry.object)
        : wrap(await entry.object[method](...(Array.isArray(args) ? args : [])), spec.returns);
      return { kind: 'response', v: PROTOCOL_VERSION, requestId, ok: true, value };
    } catch (err) {
      return { kind: 'response', v: PROTOCOL_VERSION, requestId, ok: false, error: describeError(err) };
    }
  }

  /** A wrapper object becomes a handle descriptor with its getters snapshotted. */
  function wrap(value, cls) {
    if (!cls) return value;
    const id = `${cls.toLowerCase()}-${++handleCounter}`;
    handles.set(id, { object: value, cls });
    const snapshot = {};
    for (const getter of getters.get(cls) ?? []) snapshot[getter] = value[getter];
    return { $handle: id, $class: cls, getters: snapshot };
  }

  /** One live sync loop per client; every tab's `startSync` is a subscription to it. */
  function subscribe(clientHandle, client) {
    let live = syncs.get(clientHandle);
    if (!live) {
      const userId = client.userId;
      const emit = (event) => (payload) => broadcast(event, userId, payload);
      const handle = client.startSync(emit('message'), emit('typing'), () => {
        broadcast('expired', userId, null);
        syncs.delete(clientHandle);
      }, emit('sync'), emit('devices'), emit('storage'));
      live = { handle, subscribers: new Set() };
      syncs.set(clientHandle, live);
    }
    const subId = `synchandle-${++handleCounter}`;
    live.subscribers.add(subId);
    handles.set(subId, {
      cls: 'SyncHandle',
      object: {
        stop() {
          live.subscribers.delete(subId);
          handles.delete(subId);
          if (live.subscribers.size === 0 && syncs.get(clientHandle) === live) {
            live.handle.stop();
            syncs.delete(clientHandle);
          }
        },
      },
    });
    return { $handle: subId, $class: 'SyncHandle', getters: {} };
  }

  function broadcast(event, userId, payload) {
    if (stopped || !LEADER_EVENTS.includes(event)) return;
    requests.postMessage({ kind: 'event', v: PROTOCOL_VERSION, leaderTabId: tabId, epoch, event, userId, payload });
  }

  return {
    tabId,
    epoch,
    /** Stop serving: no more answers, no heartbeat, every live sync loop stopped. */
    stop() {
      if (stopped) return;
      stopped = true;
      clearInterval(beat);
      for (const live of syncs.values()) live.handle.stop();
      syncs.clear();
      requests.close();
      for (const channel of responders.values()) channel.close();
      responders.clear();
    },
  };
}

function named(name, message) {
  const err = new Error(message);
  err.name = name;
  return err;
}
