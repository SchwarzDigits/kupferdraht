// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The FFI surface (Step 20c, approach B).
//!
//! Everything `UniFFI` exports to Kotlin / Swift lives here, separate from
//! the native Rust API. Stateful core types (`Client`, `Room`, …) are
//! wrapped by FFI objects that return owned, FFI-shaped values and
//! delegate to the inner native type; the core stays idiomatic Rust
//! (`&UserId` returns, `Arc<Room>`, generics, streams). Value types and
//! the callback traits the core genuinely owns (`SecureKeyStore`) keep
//! their derives on the core type and are reused here.
//!
//! **Wrapper objects are ephemeral.** Accessors like `rooms()` / `room()`
//! mint a fresh wrapper per call, so foreign-side object identity (`===`)
//! does not hold across calls — compare by `id`, not by reference. The
//! account entry point is the [`Kupferdraht`](manager) manager (`open` →
//! `login` / `resume` / `register`).
//!
//! See `docs/plan/20c-full-ffi-surface.md`.

use std::sync::Arc;

use crate::client::{Account as CoreAccount, Client as CoreClient};
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::storage::StorageEngineInfo;

mod admin;
mod manager;
mod room;
mod sync;
mod timeline;

pub use admin::{FailedJoin, JoinSummary, ResyncSummary};
pub use manager::{Kupferdraht, PendingClient, PendingLogin};
pub use room::Room;
pub use sync::{
    EventHandler, Message, MessageBody, Subscription, SyncListener, TaskHandle, TimelineListener,
    TypingEvent,
};
pub use timeline::{
    Asset, AssetMeta, FailedResend, ResendSummary, Timeline, TimelineDiff, TimelineItem,
    TimelineItemKind,
};

/// FFI liveness probe: the crate version. Lets a binding smoke-test the
/// proc-macro → `uniffi-bindgen` → Kotlin/Swift pipeline end to end.
#[uniffi::export]
#[must_use]
pub fn kupferdraht_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// FFI-facing structured form of a [`UserId`] — `localpart` and `domain`
/// as separate fields.
///
/// Foreign (Kotlin / Swift) consumers receive the user id structured, not
/// flattened to a string: a Kotlin `data class` / Swift `struct`. The
/// validated newtype [`UserId`] stays the internal, invariant-carrying
/// type; this record is the value that crosses the `UniFFI` boundary.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct QualifiedUserId {
    pub localpart: String,
    pub domain: String,
}

impl From<&UserId> for QualifiedUserId {
    fn from(id: &UserId) -> Self {
        Self {
            localpart: id.localpart().to_owned(),
            domain: id.domain().to_owned(),
        }
    }
}

/// Validate a flat `<localpart>@<domain>` string and return it structured.
///
/// Validation (exactly one `@`, non-empty parts) happens in Rust via
/// [`UserId::parse`], so an invalid id surfaces as
/// `KupferdrahtError::InvalidInput` rather than a silently-malformed record.
///
/// # Errors
/// `KupferdrahtError::InvalidInput` if `id` is not a well-formed
/// `<localpart>@<domain>`.
#[uniffi::export]
pub fn parse_user_id(id: &str) -> Result<QualifiedUserId, KupferdrahtError> {
    UserId::parse(id).map(|uid| QualifiedUserId::from(&uid))
}

/// The signed-in user's own profile snapshot — the FFI value form of the
/// native [`CoreAccount`](crate::client::Account).
///
/// A frozen snapshot loaded at login (`GET /v14/self`); a plain record,
/// not an object, because it carries no behaviour. `user_id` is
/// structured ([`QualifiedUserId`]); `domain` lives inside it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Account {
    pub user_id: QualifiedUserId,
    pub display_name: String,
    pub handle: Option<String>,
    pub email: Option<String>,
}

impl From<&CoreAccount> for Account {
    fn from(account: &CoreAccount) -> Self {
        Self {
            user_id: QualifiedUserId::from(account.user_id()),
            display_name: account.display_name().to_owned(),
            handle: account.handle().map(str::to_owned),
            email: account.email().map(str::to_owned),
        }
    }
}

/// Authenticated session — the FFI wrapper around the native
/// [`CoreClient`](crate::client::Client).
///
/// Holds the inner `Arc<Client>` and exposes FFI-shaped methods (owned
/// returns) that delegate to it. Foreign consumers see this as `Client`.
#[derive(uniffi::Object)]
pub struct Client {
    inner: Arc<CoreClient>,
}

impl Client {
    pub(super) fn wrap(inner: Arc<CoreClient>) -> Arc<Self> {
        Arc::new(Self { inner })
    }
}

#[uniffi::export]
impl Client {
    /// The user id, structured as a [`QualifiedUserId`], or `None` if not
    /// logged in. FFI counterpart of the native `Client::user_id`.
    #[must_use]
    pub fn user_id(&self) -> Option<QualifiedUserId> {
        self.inner.user_id().map(QualifiedUserId::from)
    }

    /// The hex device id from `POST /v14/clients`, or `None` if not
    /// logged in.
    #[must_use]
    pub fn device_id(&self) -> Option<String> {
        self.inner.device_id().map(str::to_owned)
    }

    /// The signed-in user's profile snapshot.
    #[must_use]
    pub fn account(&self) -> Account {
        Account::from(self.inner.account().as_ref())
    }

    /// The Wire backend HTTPS base URL.
    #[must_use]
    pub fn homeserver_url(&self) -> String {
        self.inner.homeserver_url().to_owned()
    }

    /// The Wire backend WebSocket URL (event stream).
    #[must_use]
    pub fn websocket_url(&self) -> String {
        self.inner.websocket_url().to_owned()
    }

    /// The human-readable backend label.
    #[must_use]
    pub fn server_title(&self) -> String {
        self.inner.server_title().to_owned()
    }

    /// The negotiated Wire API version (always 14, ADR-0008).
    #[must_use]
    pub fn api_version(&self) -> u32 {
        self.inner.api_version()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// Engine, cipher scheme and header mode of the app-data store's
    /// whole-DB encryption (Step 90, ADR-0021), read off the live
    /// connection; `None` when the backend reports no engine. FFI
    /// counterpart of the native `Client::storage_engine`.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` if the store cannot answer.
    pub async fn storage_engine(&self) -> Result<Option<StorageEngineInfo>, KupferdrahtError> {
        self.inner.storage_engine().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_record_maps_every_field() {
        let core = CoreAccount::new(
            UserId::new("user-uuid", "example.test"),
            "Alice".to_owned(),
            Some("alice".to_owned()),
            Some("alice@example.test".to_owned()),
        );
        let record = Account::from(core.as_ref());
        assert_eq!(record.user_id.localpart, "user-uuid");
        assert_eq!(record.user_id.domain, "example.test");
        assert_eq!(record.display_name, "Alice");
        assert_eq!(record.handle.as_deref(), Some("alice"));
        assert_eq!(record.email.as_deref(), Some("alice@example.test"));
    }

    #[test]
    fn account_record_keeps_absent_optionals_none() {
        let core = CoreAccount::new(UserId::new("u", "d"), "n".to_owned(), None, None);
        let record = Account::from(core.as_ref());
        assert!(record.handle.is_none(), "missing handle stays None");
        assert!(record.email.is_none(), "missing email stays None");
    }
}
