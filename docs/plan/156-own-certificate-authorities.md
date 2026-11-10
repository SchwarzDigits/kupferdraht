# Step 156 — Own certificate authorities: extra trust anchors for a self-hosted server

**Status: commissioned 2026-11-10** (Volker), from a note of the wire-server-rs session
the same day. Before Step 154, whose ACME and CRL fetches meet the same certificate
authority in a company deployment. Decided on commissioning: the extra anchors are
set on the builder (matrix-rust-sdk's `ClientBuilder::add_root_certificates`), not in
`ServerConfig`.

## Problem

Kupferdraht trusts only the Mozilla list it is built with: reqwest with `rustls-tls`
and tokio-tungstenite with `rustls-tls-webpki-roots` (`kupferdraht-core/Cargo.toml`),
no `add_root_certificate`, no own `RootCertStore`; the WebSocket opens through
`tokio_tungstenite::connect_async` (`wire/websocket.rs`). The system store and
`SSL_CERT_FILE` play no part. A self-hosted wire-server behind a company's own
certificate authority, which the README names as the supported target, cannot be
reached, and that invites workarounds up to switching the check off. The local
wire-server-rs stack is such a server (`https://localhost:8444`, a CA its installer
makes): Kupferdraht's own builds and tests cannot reach it, and the wire-server-rs
side builds a CLI copy with `rustls-tls-native-roots` added so that `SSL_CERT_FILE`
takes effect.

## Scope

1. **Extra anchors, added to the Mozilla list,** never replacing it: PEM text, one or
   more certificates. Without them nothing changes.
2. **On the builders:** `ClientBuilder::add_root_certificates` and the same on
   `Kupferdraht`'s builder, so a login and a resume both get them; the app passes them
   at every start (from its managed configuration, say). Nothing is stored.
3. **Every native TLS path to the server:** the reqwest clients `wire/http/mod.rs`
   builds, the asset download that follows the 302 by hand (Step 141, its redirect
   leads to another host or port of the same deployment), and the WebSocket
   (`connect_async_tls_with_config` with a rustls `ClientConfig` whose root store is
   the Mozilla list plus the extras). `storage/remote` needs nothing: it is compiled
   for the browser and for tests only, and the browser checks with its own list.
4. **Browser:** no effect; `docs/bindings.md` says so.
5. **FFI** (Kotlin, Swift) mirrors the builder methods; existing callers compile
   unchanged, since a builder method is optional by nature.
6. **CLI:** `--ca-file <path>` at login, repeatable; the CLI keeps the PEM in its home
   next to the server URL, since every later command builds the client anew. No
   environment variable, as for `permit_http`.
7. **A certificate failure is recognisable** as one in the error the caller sees.

## Out of scope

- The system store (`rustls-platform-verifier`): it would trust whatever the device
  has installed and give up the explicit configuration.
- Switching the check off (`danger_accept_invalid_certs` and the like): never, also
  not for tests.
- Pinning per server (THREAT_MODEL A4 "ideally with pinning"): a step of its own,
  only when a customer asks for it, with an operating guide for a key change.

## Decisions phase 1 brings to Volker

- PEM text or DER bytes across FFI (matrix-rust-sdk takes DER).
- Whether the trust decision gets an ADR (additive anchors on the builder, no system
  store, no off switch): the Step Procedure runs `new-adr` for a surfaced decision.

## TDD sketch

- A hermetic TLS server with a CA made in the test (for instance `rcgen` and
  `tokio-rustls` as dev-dependencies, admitted by `cargo deny`), for HTTP, the
  WebSocket and the asset redirect:
  - without the extra anchor the connection fails, and the error says certificate;
  - with it HTTP, the WebSocket and the redirected download succeed;
  - with it a server under a Mozilla-listed CA is still trusted (the list stays).
- FFI: the builder methods exist on the generated Kotlin and Swift.
- Smoke against the local stack (`SERVER_2_*` in `.env.dev-backend`, git-ignored):
  all three users log in with `--ca-file`, talk in one room, and a message arrives
  over the WebSocket.

## Docs

`docs/library.md`, `docs/bindings.md`, `docs/cli.md`, `docs/THREAT_MODEL.md` (A4:
extra anchors exist, pinning still not), `docs/api-mapping-matrix-rust-sdk.md`,
`CHANGELOG.md`, `README.md`.

## Afterwards, on the wire-server-rs side

The CLI build with `rustls-tls-native-roots` in
`wire-server-rs-deploy/docker/scripts/build-kupferdraht-cli.sh` and the feature trick
in `wire-server-rs/tools/kd-probe/Cargo.toml` can go; that session takes it over.
