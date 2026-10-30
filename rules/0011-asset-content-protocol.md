# Asset content protocol

Condensed from `docs/adrs/0011-asset-content-protocol.md`.

- **Move files on the two-channel pattern:** ciphertext to `POST /v14/assets`; the reference (key, sha256, locator) inside an MLS `GenericMessage.Asset`. Files never travel through MLS.
- **Encrypt assets with AES-256-CBC + PKCS7**, fresh 32-byte key + 16-byte IV per asset; layout `iv(16) || aes-cbc(plaintext)`; integrity via SHA-256 over the full blob in `Asset.RemoteData.sha256`.
- **Do not emit AES-GCM or CBC+HMAC** — the construction is a Wire-protocol given; anything else is undecryptable by other Wire clients.
- **Stream encrypt/upload and download/decrypt through tempfiles** — never buffer a whole asset in RAM.
- **Never persist plaintext asset bytes** — cache only ciphertext, decrypt on demand.
- **Inbound AES-GCM asset → warn-and-skip:** surface an "unsupported asset" timeline item, still acknowledge the MLS message.
- Out of scope: asset previews (`Asset.Preview`), AES-GCM emission, voice/video/calling.
