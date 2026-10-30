// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026) — kupferdraht-tabs: many tabs, one Kupferdraht instance.
//
// Page side:   `createTabHost({ worker })` → `host.kupferdraht` (the proxy), events.
// Worker side: `serveTab({ load, permitHttp })` in the embedder's worker script.
// The protocol constants and the method table are exported for embedders
// that build their own tooling on the wire format.
export { createTabHost, TabHost } from './host.js';
export { serveTab } from './worker.js';
export { KupferdrahtProxy, PendingLoginProxy, ClientProxy, RecoveryProxy, SyncHandleProxy } from './proxy.js';
export { createLeaderDispatcher } from './leader.js';
export {
  LeaderChangedError, NoLeaderError, METHODS, GETTERS, LEADER_EVENTS, PROTOCOL_VERSION,
  REQUEST_CHANNEL, LEASE_NAME, responseChannelName,
} from './protocol.js';
