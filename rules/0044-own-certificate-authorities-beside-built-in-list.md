# An app's own certificate authorities join the built-in list, on the builders

Condensed from `docs/adrs/0044-own-certificate-authorities-beside-built-in-list.md`.

- **Do** take an app's own certificate authorities as PEM on the builders —
  `ClientBuilder::add_root_certificates`, `KupferdrahtBuilder::add_root_certificates`,
  FFI `openWithRootCertificates`, CLI `login --ca-file` — and apply them to every
  native TLS path: both reqwest clients and the WebSocket.
- **Don't** replace the built-in Mozilla list, read the system store, or offer any
  way to switch the certificate check off — not even for tests.
- **Do** use `rustls` / `webpki-roots` directly only to configure the transports'
  trust, on reqwest's rustls version with the `ring` provider; **don't** use them
  for cryptography of Kupferdraht's own.
- A certificate the trust does not cover is `CertificateRejected` where a
  connection to a host is first made; **don't** store the anchors (the app passes
  them at every start).
