# kupferdraht-tabs — many tabs, one Kupferdraht instance

The reference thin-client layer of ADR-0026 over Kupferdraht's wasm surface:
the tab whose dedicated worker holds the origin's lease (`kupferdraht:instance`,
ADR-0023) **is** Kupferdraht — the leader; every other tab of the origin is a
thin client that reads and sends through it over `BroadcastChannel`.
When the leader tab closes or crashes, the browser releases the lease and
the next queued tab's worker takes over — no dialog, no lock file.

Plain ES modules with JSDoc and a hand-written `index.d.ts`. No bundler,
no build step, no runtime dependency. The full contract is in
`docs/bindings.md` ("Many tabs, one instance").

## Use

Worker script (yours — the only file that knows where `pkg/` lives):

```js
// tabs-worker.js
import init, { Kupferdraht } from './pkg/kupferdraht.js';
import { serveTab } from '../kupferdraht-tabs/worker.js';
serveTab({ load: async () => { await init(); return Kupferdraht; }, permitHttp: false });
```

Page:

```js
import { createTabHost, LeaderChangedError } from '../kupferdraht-tabs/index.js';

const host = createTabHost({ worker: () => new Worker('./tabs-worker.js', { type: 'module' }) });
host.addEventListener('leader-changed', (e) => console.log(e.detail)); // { leaderTabId, epoch, isSelf }

const [account] = await host.kupferdraht.accounts();     // through the leader, whichever tab that is
const client = await host.kupferdraht.resume(account.accountId);
const handle = client.startSync((m) => render(m), undefined, () => relogin(), (s) => { if (s.roomsUpdated) refreshRooms(); });
try {
  await client.sendText(roomId, 'hello');
} catch (err) {
  if (err instanceof LeaderChangedError) offerRetry(); // the leader changed mid-send; never resent by the module
}
```

**The server mode (Step 111c).** `serveTab({ load, storage })` passes
`storage` to the leader's `Kupferdraht.open` as it is — `{ mode: 'server', url,
tokenExchangeUrl, pinServiceUrl }` keeps the account databases on a storage
server, with recovery data through the PIN service (Step 112; the
addresses come from your configuration). Another browser's databases are
taken over only when asked: `host.kupferdraht.resume(accountId, { takeOver:
true })` or `registerDevice({ takeOver: true })` after a `HeldElsewhere`.

**Evictions.** `host` fires `evicted` with `{ epoch, reason }`:

- `"tab"` — this tab led and another tab took over; it follows the new
  leader.
- `"device"` — another browser took the account's databases over;
  `"access"` — the storage no longer admits the user. The leader's instance
  stopped for every tab, and every tab hears it once. The next leader
  serves again; the proxy's re-resume of a client then meets
  `HeldElsewhere` / `StorageAccessDenied` — show it, and offer "use here"
  (`resume(accountId, { takeOver: true })`) for `"device"`.

A fifth optional `startSync` callback, `onDevicesChanged({ roomId, added,
removed })`, reports a room's MLS device set changing (Step 95d; the
leader broadcasts it as `devices`) — `client.roomDevices(roomId)` lists the
devices with their MLS thumbprints. A sixth, `onStorage(state)` (Step 113),
reports `"reconnecting"` / `"online"` while the storage server is
unreachable and after the storage healed itself; the host fires `storage`,
and `client.storageState` is a snapshot.

**Recovery data (Step 112).** `client.recovery()` resolves to a
`RecoveryProxy` (`state`, `enable`, `changePin`, `verifyPin`);
`pendingLogin.recover(data, pin, { takeOver })` restores an account;
`host.kupferdraht.validateNewPin(pin)` checks a new PIN. A recovery error keeps
its detail across tabs — `reason`, `attemptsRemaining`, `retryAfter`.
`client.logout({ purgeDevice, password, purgeLocal })` (Step 114) leaves a
browser; after a purge the client is closed for every tab.

`host.kupferdraht` has the same method names as the wasm `Kupferdraht` class
(everything but `open`); the objects it returns mirror `PendingLogin`,
`Client`, `Recovery` and `SyncHandle`. `host.state` is `electing` | `leader` | `thin`;
`host.takeOver()` is the override for a hung leader; `host.close()` on
page teardown.

## Failover, in one line each

- Reads (`accounts`, `rooms`, `messages`, `members`, `downloadAsset`, …)
  in flight when the leader changes are re-issued to the new leader; a
  client re-resumes, a recovery handle asks its client again.
- Mutations (`sendText`, `login`, `registerDevice`, `recover`, `enable`,
  `changePin`, …) in flight reject
  with `LeaderChangedError` and are never retried by the module.
- `startSync` subscriptions are re-established; a call while no leader
  exists rejects with `NoLeaderError` after `leaderWaitMs`.
- A late message of an older leader (lower epoch) moves no tab back to it
  (Step 116); its event still reaches the callbacks.

## Files

`protocol.js` (channels, envelopes, the method table with the failover
class, errors) · `host.js` (`createTabHost`) · `worker.js` (`serveTab`:
queue → hand-off → serve) · `leader.js` (dispatcher, handles,
subscriptions, heartbeat) · `proxy.js` (`KupferdrahtProxy` and the handle
proxies) · `index.js` · `index.d.ts`.

Tests: `kupferdraht-core/tests/step_94_leader_tab.rs` (headless browser, via
`scripts/wasm-test.sh`) and `kupferdraht-core/tests/step_94_tab_proxy_parity.rs`
(the wasm surface ↔ proxy parity pin). Smoke: `../opfs-smoke/tabs.html`.
