// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Step 94 (ADR-0026): the embedder's worker script for the multi-tab page —
// the whole of it. `serveTab` queues this tab's candidate for the origin's
// lease and, on the grant, opens Kupferdraht here (the hand-off) and serves
// every tab; until then the tab is a thin client of whoever leads. This
// file is the only place that knows where the wasm package lives.
import init, { Kupferdraht } from './pkg/kupferdraht.js';
import { serveTab } from '../kupferdraht-tabs/worker.js';

serveTab({
  load: async () => { await init(); return Kupferdraht; },
  permitHttp: true,
});
