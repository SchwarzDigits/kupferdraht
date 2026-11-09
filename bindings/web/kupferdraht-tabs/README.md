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
const handle = client.startSync((m) => render(m), undefined, () => relogin(), (s) => s.changedRooms.forEach(refreshRoom));
const sent = await client.sendText(roomId, 'hello'); // keyed: survives a leader change, sent once
// sent.idempotencyKey is the key the proxy added — a retry with it never sends twice
try {
  await client.createRoom('Team');
} catch (err) {
  if (err instanceof LeaderChangedError) offerRetry(); // the leader changed mid-call; never resent by the module
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
and `client.storageState` is a snapshot. A seventh, `onRoomsChanged(change)`
(Step 138), carries a room's read state after `markAsRead` / `markAsUnread`
on any tab — the host fires `rooms`; `client.rooms()` lists every room with
`unreadCount`, `lastReadAtMs` and `markedUnread`.

**Recovery data (Step 112).** `client.recovery()` resolves to a
`RecoveryProxy` (`state`, `enable`, `changePin`, `verifyPin`);
`pendingLogin.recover(data, pin, { takeOver })` restores an account;
`host.kupferdraht.validateNewPin(pin)` checks a new PIN. A recovery error keeps
its detail across tabs — `reason`, `attemptsRemaining`, `retryAfter`.
`client.logout({ purgeDevice, password, purgeLocal })` (Step 114) leaves a
browser; after a purge the client is closed for every tab.

**Service tokens (Step 139).** `client.serviceToken(audience)` resolves to
`{ token, expiresIn }` (seconds) of the token exchange for the embedder's
own service (ADR-0036 revision 1.1), as the exchange answered — idempotent,
so a call in flight when the leader changes is re-issued. The reserved
audiences `storage` and `pin`, `""` and the local mode reject `InvalidInput`.

`host.kupferdraht` has the same method names as the wasm `Kupferdraht` class
(everything but `open`); the objects it returns mirror `PendingLogin`,
`Client`, `Recovery` and `SyncHandle`. `host.state` is `electing` | `leader` | `thin`;
`host.takeOver()` is the override for a hung leader; `host.close()` on
page teardown.

## Failover

What a call in flight does when the leader changes — `protocol.js`'s
`METHODS` table, class by class (Step 130a; a native pin compares this
table against it):

| Class | Calls | At a leader change |
|---|---|---|
| keyed | `Client.sendText`, `Client.sendReply`, `Client.sendKnock`, `Client.sendLocation`, `Client.sendFile`, `Client.toggleReaction` | Re-issued to the new leader with the call's idempotency key — `options.idempotencyKey`, or the one the proxy added before the first attempt. The new leader answers a send the old one completed with its stored handle (nothing reaches the backend twice), resends one that failed under the same message id, and sends one that never got that far. The handle carries the key (`{ messageId, sentAtMs, idempotencyKey }`). |
| idempotent | `Kupferdraht.initiateSso`, `Kupferdraht.initiateSsoByEmail`, `Kupferdraht.resume`, `Kupferdraht.accounts`, `Kupferdraht.validateNewPin`, `PendingLogin.devices`, `Client.storageEngine`, `Client.pushState`, `Client.syncOnce`, `Client.rooms`, `Client.messages`, `Client.failedSends`, `Client.resend`, `Client.resendFailed`, `Client.members`, `Client.avatar`, `Client.avatarKey`, `Client.teamAvatar`, `Client.roomDevices`, `Client.ownDeviceThumbprint`, `Client.roomAccess`, `Client.devices`, `Client.featureConfigs`, `Client.fileSharing`, `Client.serviceToken`, `Client.downloadAsset`, `Client.recovery`, `Recovery.state`, `Recovery.verifyPin`, `SyncHandle.stop`, `Client.markAsRead`, `Client.markAsUnread`, `Client.startTyping`, `Client.stopTyping`, `Client.redact`, `Client.createDm`, `Client.renameRoom`, `Client.setRoomAccess`, `Client.allowGuests`, `Client.setDisplayName` | Re-issued to the new leader: the reads, and the mutations a repeat converges and answers like the first call. A client re-resumes by its `userId` first; a recovery handle asks its client again. |
| mutation | `Kupferdraht.login`, `Kupferdraht.finalizeSso`, `PendingLogin.registerDevice`, `PendingLogin.recover`, `PendingLogin.deleteDevice`, `PendingLogin.cancel`, `Client.createRoom`, `Client.editMessage`, `Client.uploadAvatar`, `Client.removeAvatar`, `Client.invite`, `Client.kick`, `Client.leave`, `Client.deleteRoom`, `Client.createJoinLink`, `Client.revokeJoinLink`, `Client.joinViaLink`, `Client.setHandle`, `Client.updateEmail`, `Client.deleteDevice`, `Client.logout`, `Recovery.enable`, `Recovery.changePin` | Rejects with `LeaderChangedError` and is never resent by the module: it cannot know whether the call reached the backend, and a repeat is not safe — a second room, a second edit, a repeat that fails after the first landed (an `invite` of a member already in the group, a `leave` of a room already left). Check the state, then retry deliberately. |
| subscription | `Client.startSync` | Re-established on the new leader; the callbacks stay in this tab. |

- A keyed call without `options.idempotencyKey` gets a fresh key from the
  proxy (`crypto.randomUUID()`), so both leaders see the same key; keep
  `idempotencyKey` from the handle — or pass your own — to retry after an
  error without sending twice. A repeated `toggleReaction` needs its key:
  a new key toggles back.
- An unkeyed send whose leader went away mid-send is a failed row on the
  new leader: `failedSends(roomId)` lists it, `resend(roomId, messageId)`
  and `resendFailed(roomId)` re-drive it under its message id (Step 130b).
  `resend` of a row this device already sent answers its handle, so a
  resend that was itself in flight is safe to re-issue.
- A call while no leader exists rejects with `NoLeaderError` after
  `leaderWaitMs`.
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
