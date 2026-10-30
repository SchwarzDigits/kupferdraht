# ADR-0005: Web delivery via WebAssembly only

**Status:** Superseded by [ADR-0015](0015-browser-architecture-wasm-indexeddb-bindgen.md) (2026-07)
**Date:** 2026-04-21
**Deciders:** Volker Zöpfel

---

## Context

Kupferdraht is written in Rust and targets multiple platforms via Mozilla UniFFI — native bindings for Android (Kotlin), iOS (Swift), macOS (Swift), Windows and Linux (C ABI), and Web. For the Web target, there are two fundamentally different delivery modes.

**WebAssembly (WASM).** The Rust code is compiled to WASM and runs in the browser's WASM runtime. Data types cross the boundary to JavaScript via a generated binding layer (wasm-bindgen or equivalent). WASM has its own linear memory, isolated from the JavaScript heap. JavaScript code cannot read WASM memory directly; it can only interact through the exported API surface.

**JavaScript port.** The Rust code is either reimplemented in JavaScript (or TypeScript) or a Rust-to-JavaScript transpiler is used. The cryptographic and application logic runs as native JavaScript, directly in the browser's JavaScript runtime, with full access to the DOM and other page scripts.

The question for Kupferdraht is which of these to target for Web — or whether to support both. The decision affects not just the engineering investment but the security posture of the Web deployment.

---

## Options Considered

### Option A: Deliver via WASM only

All Web deployments of Kupferdraht use the WASM-compiled Rust code. There is no JavaScript port. Consumer applications must have a build pipeline that loads and initializes WASM.

- **Pro:** Sensitive data (MLS keys, decrypted message content during processing) lives in WASM linear memory, which JavaScript on the same page cannot directly read.
- **Pro:** A single Rust codebase serves every platform. No separate JavaScript implementation to maintain, audit, or synchronize with the Rust version.
- **Pro:** Cryptographic correctness is easier to reason about when the same code runs everywhere. Platform-specific crypto bugs are structurally ruled out.
- **Con:** Consumers cannot use Kupferdraht from static HTML pages without a WASM runtime. Embedding as a drop-in script tag is not supported.
- **Con:** WASM requires a binary asset load, which has implications for Content Security Policy, Subresource Integrity, and startup latency.

### Option B: Deliver via WASM and maintain a JavaScript port as an alternative

Provide both: WASM as the preferred path, and a separately maintained JavaScript/TypeScript port for environments where WASM is inconvenient.

- **Pro:** Broader consumer reach. Static sites, environments with strict WASM restrictions, or embeddings where a WASM runtime is impractical can still use Kupferdraht.
- **Pro:** Some debugging workflows are easier in native JavaScript.
- **Con:** Two implementations to keep synchronized. Every feature addition, bug fix, and security patch must be applied to both, or one will silently drift.
- **Con:** The JavaScript port cannot provide the memory-isolation benefit of WASM. Plaintext would live in the JavaScript heap alongside every other script on the page. An XSS vulnerability, a malicious third-party script, or a compromised CDN dependency could read that plaintext directly.
- **Con:** Audit surface doubles. Reviewers must evaluate two codebases.

### Option C: JavaScript port as primary Web delivery

Abandon WASM for Web and build a pure JavaScript implementation.

- **Pro:** Maximum compatibility — works in any browser environment, including restrictive ones.
- **Pro:** Familiar toolchain for Web developers.
- **Con:** All of Option B's drawbacks without any of its WASM-based benefits. Plaintext in the JavaScript heap has no memory isolation from the rest of the page.
- **Con:** Two codebases (Rust for native + JavaScript for Web) with the same synchronization problem.

---

## Decision

**We deliver Kupferdraht for Web exclusively as WebAssembly (Option A). There is no JavaScript port and we will not accept one.**

The memory isolation that WASM provides is a structural security property, not a convenience. It raises the bar against a class of attacks — malicious scripts in the same page, XSS-introduced code, compromised third-party dependencies — that a pure-JavaScript implementation cannot defend against. Since Kupferdraht handles cryptographic keys and message plaintext, and since Web deployments are a primary use case (especially for government agency workers at browser-based workstations), this isolation is non-negotiable.

The cost — that consumers must have a WASM runtime available — is acceptable. Every modern browser supports WASM. Consumers who want a lightweight embeddable script should use a different library; Kupferdraht is not that library.

The Web consumer environment has its own security responsibilities that Kupferdraht cannot fulfill. WASM isolation ends when Kupferdraht returns a decrypted value to the JavaScript side for display. From that point on, the consumer's Content Security Policy, Subresource Integrity hashes, and origin isolation are the relevant defenses. Kupferdraht documents this boundary in `THREAT_MODEL.md` (N8, L2).

---

## Consequences

### Positive

- Structural memory isolation between Kupferdraht's sensitive data and other scripts on the page.
- One codebase for all platforms. Security audits, tests, and fixes apply everywhere at once.
- Consistent cryptographic behavior across native and Web deployments — the same Rust code, the same openmls version.
- Smaller attack surface: no alternative implementation to reason about or audit.

### Negative

- Web consumers must have a build pipeline capable of loading WASM. Static HTML pages without a runtime cannot integrate Kupferdraht directly.
- WASM asset load and initialization adds to Web application startup time.
- Some debugging workflows that assume pure-JavaScript tooling must be adapted.

### Neutral

- The benefit of WASM isolation ends at the JavaScript boundary. When Kupferdraht returns a decrypted message for display, the plaintext enters the JavaScript heap and is subject to the same-origin risks of any Web application. This is documented in `THREAT_MODEL.md` as limitation L2.
- WebCrypto (which Kupferdraht uses for Web-side key operations) is called from Rust through UniFFI and WASM bindings — this path is internal to Kupferdraht and does not introduce a JavaScript dependency.

---

## Implementation Notes

Kupferdraht's Web build produces a WASM module with UniFFI-generated TypeScript bindings. The consumer loads the module during application initialization and interacts with it through the generated API.

On the Web platform, SQLite runs inside the WASM module via `sql.js` (SQLite compiled to WebAssembly). Persistent storage uses IndexedDB, with the SQLite database serialized as a single binary blob. Encryption of sensitive fields uses WebCrypto via the browser, called from Rust through the bindings layer.

WebCrypto key handles (non-extractable) are stored in IndexedDB directly — they never leave the browser's crypto subsystem.

---

## References

- `THREAT_MODEL.md` — N8 (no protection against malicious same-origin scripts), L2 (WASM protection ends at JavaScript boundary)
- `secure-storage.md` — Web storage architecture using sql.js and IndexedDB
- ADR-0002 — single-Rust-crate architecture that WASM-only delivery preserves across platforms

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-21 | Initial ADR |
| 1.1 | 2026-07-17 | Status changed to Superseded by ADR-0015 (Browser architecture — WASM-only delivery, IndexedDB per-record persistence, hand-written wasm-bindgen surface). The WASM-only core decision is carried forward unchanged; the Implementation Notes here described a design (sql.js blob, UniFFI-TS bindings) that was never built. |
