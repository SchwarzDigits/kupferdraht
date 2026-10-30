// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 91/92/111 (ADR-0022/0023/0035): Kupferdraht runs here, in a dedicated
// worker of a cross-origin isolated page — the storage blocks on a
// SharedArrayBuffer — and only one such worker per origin holds the instance
// lease. The main thread only renders.
import init, { Kupferdraht, kupferdraht_version } from './pkg/kupferdraht.js';

const say = (m) => postMessage({ log: m });
// The scripted smoke (headless Chrome, no driver) reads the verdict off the
// dev server's access log: a same-origin GET carrying it as a query string.
const beacon = (verdict) => fetch(`./smoke-verdict?${encodeURIComponent(verdict)}`).catch(() => {});

// `takeOver` comes from the page: a second tab (or a second worker on the
// same page) asks to take the origin's instance over from the holder.
const params = new URL(self.location.href).searchParams;
const takeOver = params.get('takeOver') === '1';
// `storage=server` (Step 111b): the account databases on a storage server.
// Its address and the token exchange's come from the dev server
// (`/smoke-storage.json`), never from this file.
const serverMode = params.get('storage') === 'server';
// Step 117: Kupferdraht writes nowhere itself; this page forwards its log events
// to the page (`&log=<level>`, default warn; `&log=none` passes no onLog)
// and beacons the first event of each level, and any foreign target.
const logLevel = params.get('log') ?? 'warn';
const logFirst = new Set();
const onLog = ({ level, target, message }) => {
  postMessage({ log: `[kupferdraht ${level}] ${target}: ${message}` });
  if (!logFirst.has(level)) {
    logFirst.add(level);
    fetch(`./smoke-verdict?${encodeURIComponent(`LOG-FIRST ${level} ${target}`)}`).catch(() => {});
  }
  if (level === 'warn' || level === 'error') {
    fetch(`./smoke-verdict?${encodeURIComponent(`LOG ${level} ${target}: ${message}`)}`).catch(() => {});
  }
  if (target !== 'kupferdraht' && !target.startsWith('kupferdraht::')) {
    fetch(`./smoke-verdict?${encodeURIComponent(`LOG-FOREIGN ${target}`)}`).catch(() => {});
  }
};
const logOptions = logLevel === 'none' ? {} : { logLevel, onLog };

let kupferdraht;
try {
  await init();
  say(`kupferdraht ${kupferdraht_version()} loaded in a dedicated worker (takeOver=${takeOver}, storage=${serverMode ? 'server' : 'local'})`);
  const storage = serverMode
    ? { mode: 'server', ...(await (await fetch('/smoke-storage.json')).json()) }
    : undefined;
  const t0 = performance.now();
  kupferdraht = await Kupferdraht.open({ permitHttp: true, takeOver, storage, ...logOptions }, (reason) => {
    // The instance stopped ("tab": another tab took over; "access": the
    // storage no longer admits the user): every store call now fails; tell
    // the page so it terminates this worker.
    say(`EVICTED (${reason}) — this worker must be terminated`);
    beacon(`EVICTED ${reason}`);
    postMessage({ evicted: true });
  });
  say(`Kupferdraht.open: ok in ${Math.round(performance.now() - t0)} ms (lease held, storage opened, global.db keyed)`);
  const accounts = await kupferdraht.accounts();
  say(`accounts(): ${JSON.stringify(accounts)}`);
  say('smoke: OPEN-OK');
  await beacon(`OPEN-OK accounts=${accounts.length} takeOver=${takeOver}`);
} catch (e) {
  say(`Kupferdraht.open failed: ${e}`);
  say('smoke: OPEN-FAILED');
  await beacon(`OPEN-FAILED ${e}`);
}

// `resume` (Step 111b): open the first registered account again — in the
// server mode its databases come from the storage server.
const resumeFirstAccount = async (room) => {
  try {
    const [account] = await kupferdraht.accounts();
    if (!account) throw new Error('no registered account');
    const client = await kupferdraht.resume(account.accountId);
    say(`resume: ${client.userId}`);
    const summary = await client.syncOnce();
    say(`syncOnce: ${JSON.stringify(summary)}`);
    if (room) {
      const messages = await client.messages(room, 3);
      say(`messages(${room}, 3): ${JSON.stringify(messages)}`);
    }
    say('smoke: RESUME-OK');
    await beacon(`RESUME-OK account=${account.accountId}`);
  } catch (err) {
    say(`resume failed: ${err.name}: ${err.message}`);
    await beacon(`RESUME-FAILED ${err.name}: ${err.message}`);
  }
};

// `hold` (Step 111c): resume and keep working — a sync pass every 3 s and
// one message after 30 s (a commit, which is where a takeover by another
// browser shows): that browser's `takeover` run makes this one report
// onEvicted("device").
const holdFirstAccount = async (room) => {
  try {
    const [account] = await kupferdraht.accounts();
    const client = await kupferdraht.resume(account.accountId);
    await beacon('HOLD-RESUMED');
    for (let pass = 0; pass < 20 && !kupferdraht.evicted; pass++) {
      try {
        await client.syncOnce();
        if (pass === 10 && room) await client.sendText(room, `hold smoke ${new Date().toISOString()}`);
      } catch (err) {
        say(`hold: pass ${pass}: ${err.name}: ${err.message}`);
      }
      await new Promise((resolve) => setTimeout(resolve, 3000));
    }
    await beacon(`HOLD-END evicted=${kupferdraht.evicted}`);
  } catch (err) {
    await beacon(`HOLD-FAILED ${err.name}: ${err.message}`);
  }
};

// `takeover` (Step 111c): log in (the seed is here, so `keys` is
// "present") and register; a database another browser holds throws
// HeldElsewhere — then take it over, as the embedder would after asking.
const takeOverWithLogin = async (login) => {
  try {
    const pending = await kupferdraht.login(login.api, login.ws ?? '', '', login.email, login.password, undefined, 'opfs-smoke');
    let client;
    try {
      client = await pending.registerDevice();
      await beacon(`TAKEOVER-NOT-HELD keys=${pending.keys}`);
      return;
    } catch (err) {
      await beacon(`TAKEOVER-HELD ${err.name} keys=${pending.keys}`);
      if (err.name !== 'HeldElsewhere') return;
    }
    client = await pending.registerDevice({ takeOver: true });
    await client.syncOnce();
    await beacon(`TAKEOVER-OK deviceReplaced=${client.deviceReplaced}`);
  } catch (err) {
    await beacon(`TAKEOVER-FAILED ${err.name}: ${err.message}`);
  }
};

// `enable` (Step 112b): resume the first account and make its recovery
// data — the first, or a new copy under the same PIN — then hand the bytes
// to the dev server for the `recover` run in another browser.
const enableRecovery = async (pin) => {
  try {
    const [account] = await kupferdraht.accounts();
    const client = await kupferdraht.resume(account.accountId);
    const recovery = client.recovery();
    const before = await recovery.state();
    const data = before.state === 'disabled'
      ? await recovery.enable(pin)
      : await recovery.changePin(pin, pin);
    const after = await recovery.state();
    const verified = await recovery.verifyPin(pin);
    const otherVerified = await recovery.verifyPin(`${pin}0`);
    await fetch('/smoke-recovery', { method: 'PUT', body: data });
    await beacon(`ENABLE-OK before=${before.state} after=${after.state} bytes=${data.length} verifyPin=${verified} otherPin=${otherVerified}`);
  } catch (err) {
    await beacon(`ENABLE-FAILED ${err.name}: ${err.message}`);
  }
};

// `recover` (Step 112b): a browser without the account's keys logs in and
// restores them from the recovery data — a wrong PIN first (nothing is
// kept, the attempts left are reported), then the right one; a database
// another browser holds is taken over, as the embedder would after asking.
const recoverWithLogin = async (login) => {
  try {
    const data = new Uint8Array(await (await fetch('/smoke-recovery')).arrayBuffer());
    const pending = await kupferdraht.login(login.api, login.ws ?? '', '', login.email, login.password, undefined, 'opfs-smoke');
    await beacon(`RECOVER-LOGIN keys=${pending.keys} recoveryExpected=${pending.recoveryExpected}`);
    try {
      await pending.recover(data, `${login.recoveryPin}0`);
      await beacon('RECOVER-WRONG-PIN-ACCEPTED');
      return;
    } catch (err) {
      await beacon(`RECOVER-WRONG-PIN ${err.name} attemptsRemaining=${err.attemptsRemaining} keys=${pending.keys}`);
    }
    let client;
    try {
      client = await pending.recover(data, login.recoveryPin);
    } catch (err) {
      await beacon(`RECOVER-HELD ${err.name} keys=${pending.keys}`);
      if (err.name !== 'HeldElsewhere') return;
      client = await pending.registerDevice({ takeOver: true });
    }
    await client.syncOnce();
    const state = await client.recovery().state();
    await beacon(`RECOVER-OK deviceReplaced=${client.deviceReplaced} recovery=${state.state}`);
  } catch (err) {
    await beacon(`RECOVER-FAILED ${err.name}: ${err.message}`);
  }
};

// `purge-device` (Step 114): resume the first account and log out with
// `purgeDevice` — the device at the backend and, in the server mode, the
// account's databases there are deleted. Another browser of the account
// shares the device, so its session ended with it: it logs in again and
// starts with empty databases (`purge-local` shows it).
const purgeDevice = async (login) => {
  try {
    const [account] = await kupferdraht.accounts();
    const client = await kupferdraht.resume(account.accountId);
    await client.logout({ purgeDevice: true, password: login.password });
    const accounts = await kupferdraht.accounts();
    await beacon(`PURGE-DEVICE-OK accounts=${accounts.length}`);
  } catch (err) {
    await beacon(`PURGE-DEVICE-FAILED ${err.name}: ${err.message}`);
  }
};

// `purge-local` (Step 114): log in, register, log out with `purgeLocal` —
// the account is no longer listed, and the next login finds no keys.
const purgeLocal = async (login) => {
  try {
    const first = await kupferdraht.login(login.api, login.ws ?? '', '', login.email, login.password, undefined, 'opfs-smoke');
    const client = first.keys === 'missing'
      ? await first.registerDevice({ newSetup: true })
      : await first.registerDevice();
    // After a `purge-device` elsewhere the databases start empty: no
    // recovery state, whatever the deleted ones held.
    const recovery = (await client.recovery().state()).state;
    await client.logout({ purgeLocal: true });
    const accounts = await kupferdraht.accounts();
    const again = await kupferdraht.login(login.api, login.ws ?? '', '', login.email, login.password, undefined, 'opfs-smoke');
    await beacon(`PURGE-LOCAL-OK keys=${first.keys} recovery=${recovery} accounts=${accounts.length} keysAfter=${again.keys} recoveryExpected=${again.recoveryExpected}`);
    again.cancel();
  } catch (err) {
    await beacon(`PURGE-LOCAL-FAILED ${err.name}: ${err.message}`);
  }
};

// `outage` (Step 113): log in (the seed is here), keep syncing and report
// the storage state; the dev server stops the storage server's relay a few
// seconds after OUTAGE-READY and starts it again later. A send during the
// outage fails, one after the heal goes through — no resume, no reload.
const sendAndReport = async (client, room, label) => {
  try {
    await client.sendText(room, `outage smoke ${label} ${new Date().toISOString()}`);
    await beacon(`${label} ok`);
  } catch (err) {
    await beacon(`${label} ${err.name}`);
  }
};
const outage = async (login) => {
  try {
    const pending = await kupferdraht.login(login.api, login.ws ?? '', '', login.email, login.password, undefined, 'opfs-smoke');
    const client = await pending.registerDevice();
    let healed;
    const online = new Promise((resolve) => { healed = resolve; });
    client.startSync(() => {}, undefined, undefined, undefined, undefined, (state) => {
      beacon(`STORAGE ${state}`);
      if (state === 'online') healed();
    });
    await beacon(`OUTAGE-READY state=${client.storageState}`);
    await new Promise((resolve) => setTimeout(resolve, 10000));
    await sendAndReport(client, login.room, 'SEND-1');
    await sendAndReport(client, login.room, 'SEND-2');
    await Promise.race([online, new Promise((resolve) => setTimeout(resolve, 120000))]);
    await sendAndReport(client, login.room, 'SEND-3');
    await beacon(`OUTAGE-END state=${client.storageState}`);
  } catch (err) {
    await beacon(`OUTAGE-FAILED ${err.name}: ${err.message}`);
  }
};

// `idle` (Step 118): resume the first account, start the live sync and stay
// idle for 12 minutes. Every minute it beacons how often each endpoint was
// called — with a live socket the room list only after a room event, a drain
// only after a ring and once per 5 minutes; a message, a new room and a
// renamed room from another device are beaconed as they arrive.
const idle = async () => {
  try {
    const [account] = await kupferdraht.accounts();
    const client = await kupferdraht.resume(account.accountId);
    performance.setResourceTimingBufferSize(10000);
    const calls = (path) => performance.getEntriesByType('resource')
      .filter((entry) => new URL(entry.name).pathname.endsWith(path)).length;
    const t0 = Date.now();
    const at = () => `t=${Math.round((Date.now() - t0) / 1000)}s`;
    const names = new Map((await client.rooms()).map((r) => [r.roomId, r.name]));
    const reportRooms = async () => {
      for (const { roomId, name } of await client.rooms()) {
        if (names.get(roomId) !== name) beacon(`IDLE-ROOM ${at()} ${roomId.slice(0, 8)} ${name}`);
        names.set(roomId, name);
      }
    };
    client.startSync((m) => beacon(`IDLE-MSG ${at()} ${m.body ?? '(no text)'}`), undefined, undefined, reportRooms);
    for (let minute = 1; minute <= 12; minute++) {
      await new Promise((resolve) => setTimeout(resolve, 60000));
      await beacon(`IDLE ${minute}m list-ids=${calls('/conversations/list-ids')} notifications=${calls('/notifications')}`);
    }
    await beacon('IDLE-END');
  } catch (err) {
    await beacon(`IDLE-FAILED ${err.name}: ${err.message}`);
  }
};

// `echo` (Step 122): resume the first account, read the push state before
// and during the live sync, `email` and `storageEngine().location`, then
// create a room, send into it and time the `onSync` that follows — the
// backend sends the sender no frame for its own message, so before Step 122
// the next pass came with the socket renewal, up to a minute later. Ends
// with the push state after `stop()`.
const echo = async () => {
  try {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const [account] = await kupferdraht.accounts();
    const client = await kupferdraht.resume(account.accountId);
    const before = client.pushState();
    let lastSync = 0;
    const handle = client.startSync(() => {}, undefined, undefined, () => { lastSync = Date.now(); });
    await sleep(5000);
    const engine = await client.storageEngine();
    await beacon(`ECHO before=${before} during=${client.pushState()} email=${client.email ?? '-'} location=${engine?.location ?? '-'}`);
    const { roomId } = await client.createRoom(`echo ${Date.now()}`);
    await sleep(3000);
    const body = `echo ${new Date().toISOString()}`;
    lastSync = 0;
    await client.sendText(roomId, body);
    const resolved = Date.now();
    for (let i = 0; i < 100 && lastSync === 0; i++) await sleep(100);
    const own = (await client.messages(roomId, 10)).find((m) => m.body === body);
    await beacon(`ECHO onSync=${lastSync ? `${lastSync - resolved}ms` : 'none-in-10s'} inTimeline=${Boolean(own)} status=${own?.deliveryStatus ?? '-'}`);
    handle.stop();
    await beacon(`ECHO afterStop=${client.pushState()}`);
  } catch (err) {
    await beacon(`ECHO-FAILED ${err.name}: ${err.message}`);
  }
};

onmessage = async (e) => {
  if (e.data.echo && kupferdraht) return echo();
  if (e.data.idle && kupferdraht) return idle();
  if (e.data['purge-device'] && kupferdraht) return purgeDevice(e.data.login);
  if (e.data['purge-local'] && kupferdraht) return purgeLocal(e.data.login);
  if (e.data.enable && kupferdraht) return enableRecovery(e.data.login.recoveryPin);
  if (e.data.recover && kupferdraht) return recoverWithLogin(e.data.login);
  if (e.data.resume && kupferdraht) return resumeFirstAccount(e.data.room);
  if (e.data.outage && kupferdraht) return outage(e.data.login);
  if (e.data.hold && kupferdraht) return holdFirstAccount(e.data.room);
  if (e.data.takeover && kupferdraht) return takeOverWithLogin(e.data.login);
  const { login } = e.data;
  if (!login || !kupferdraht) return;
  try {
    const pending = await kupferdraht.login(login.api, login.ws ?? '', '', login.email, login.password, undefined, 'opfs-smoke');
    const { keys, recoveryExpected } = pending;
    say(`login: ${pending.userId} — keys ${keys}, recoveryExpected ${recoveryExpected}`);
    // Step 110 (ADR-0035): without the account's keys in this browser only a
    // first setup proceeds; the smoke always sets up afresh then.
    const client = keys === 'missing'
      ? await pending.registerDevice({ newSetup: true })
      : await pending.registerDevice();
    say(`registerDevice: ok (deviceReplaced ${client.deviceReplaced}) — storageEngine ${JSON.stringify(await client.storageEngine())}`);
    await beacon(`REGISTERED keys=${keys} recoveryExpected=${recoveryExpected} deviceReplaced=${client.deviceReplaced}`);
    const summary = await client.syncOnce();
    say(`syncOnce: ${JSON.stringify(summary)}`);
    if (login.room) {
      const handle = await client.sendText(login.room, `opfs smoke ${new Date().toISOString()}`);
      say(`sendText: ${JSON.stringify(handle)}`);
      const messages = await client.messages(login.room, 5);
      say(`messages(${login.room}, 5): ${JSON.stringify(messages)}`);
    }
    say('smoke: LOGIN-OK — reload the page and resume() the account to see the store survive');
    await beacon(`LOGIN-OK keys=${keys} recoveryExpected=${recoveryExpected} deviceReplaced=${client.deviceReplaced}`);
  } catch (err) {
    say(`login vertical failed: ${err.name}: ${err.message} (evicted=${kupferdraht.evicted})`);
    await beacon(`LOGIN-FAILED ${err.name}: ${err.message}`);
  }
};

// The handler is set: a login posted from now on is not lost (the scripted
// scenario waits for this line, not for OPEN-OK).
say('smoke: READY');
