// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::identity::UserId;

/// Body for `POST /v{N}/conversations` — registers a new MLS
/// conversation. Kupferdraht pins `protocol: "mls"` (ADR-0001) and uses
/// the `qualified_users` shape (preferred per the swagger).
#[derive(Debug, Clone, Serialize)]
pub struct CreateConversationRequest<'a> {
    pub name: &'a str,
    pub protocol: &'static str,
    pub group_conv_type: &'static str,
    /// Empty for the zero-invitee `create_room` path. Step 6b
    /// populates it once invite handling lands.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub qualified_users: Vec<QualifiedId>,
}

/// Response body of `GET /api-version`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiVersionResponse {
    pub supported: Vec<u32>,
    #[serde(default)]
    pub development: Vec<u32>,
}

/// Response body of `POST /v{N}/login`.
#[derive(Clone, Deserialize, Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u32,
    pub user: String,
}

impl std::fmt::Debug for LoginResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("LoginResponse")
            .field("access_token", &crate::redact::Hidden)
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("user", &self.user)
            .finish()
    }
}

/// Login outcome — JSON body fields plus the `zuid` refresh token
/// extracted from the `Set-Cookie` header. Required separately because
/// reqwest's cookie store does not reliably carry the `zuid` across
/// later requests under all server configurations, and Wire's
/// `POST /v14/access` is authenticated **only** by the cookie.
#[derive(Clone)]
pub struct LoginOutcome {
    pub access_token: String,
    pub user: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for LoginOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("LoginOutcome")
            .field("access_token", &crate::redact::Hidden)
            .field("user", &self.user)
            .field("refresh_token", &crate::redact::Hidden)
            .finish()
    }
}

/// Response body of `GET /v{N}/mls/public-keys`.
///
/// Wire groups its externally-visible MLS public keys by purpose. Today
/// only `removal` is defined — the keys Wire uses to sign administrative
/// Remove proposals on behalf of teams (ADR-0004,
/// `docs/wire/mls-deviations.md` W-5). Kupferdraht embeds the matching
/// entry into every group it creates / joins so Wire-initiated removals
/// validate.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MlsKeysByPurpose {
    pub removal: MlsKeys,
}

/// Mapping of MLS signature schemes to base64-encoded public keys.
/// Wire returns every scheme it supports; Kupferdraht picks the entry that
/// matches `MLS_PUBLIC_KEY_TYPE` (ECDSA-P256, our ciphersuite).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MlsKeys {
    pub ed25519: String,
    pub ecdsa_secp256r1_sha256: String,
    pub ecdsa_secp384r1_sha384: String,
    pub ecdsa_secp521r1_sha512: String,
}

/// Response body of `POST /v{N}/clients`.
///
/// Wire's response is larger (it echoes the full client record);
/// Kupferdraht only consumes `id`. Extras are ignored.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NewClientResponse {
    pub id: String,
}

/// Wire's `Qualified_UserId` / `Qualified_ConvId` — the federated
/// `{id, domain}` pair that v14 returns for users, conversations, and
/// every other federated identifier. Same wire shape, different
/// codomains; we share the type.
///
/// MLS identity construction needs the domain, so Kupferdraht must consume
/// `qualified_id`; the legacy flat `id` alone is not enough (Wire's
/// swagger marks `qualified_id` as required on `/self` and on each
/// conversation in the list response).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct QualifiedId {
    pub id: String,
    pub domain: String,
}

impl std::fmt::Display for QualifiedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.id, self.domain)
    }
}

/// Boundary conversion: a core [`UserId`] into the Wire `{id, domain}`
/// shape. Lives in the `wire` layer so core never imports `QualifiedId`
/// (ADR-0009) — the translation happens here, at the edge.
impl From<&UserId> for QualifiedId {
    fn from(user_id: &UserId) -> Self {
        Self {
            id: user_id.localpart().to_string(),
            domain: user_id.domain().to_string(),
        }
    }
}

/// Build a Wire qualified user id `<uuid>@<domain>` from raw parts.
///
/// Same shape as [`QualifiedId::Display`]; use this when you already
/// hold the pieces as `&str` rather than a [`QualifiedId`] struct.
/// Both arguments must be non-empty — Wire never emits empty `id` /
/// `domain` and the qualified form is meaningless without them.
pub(crate) fn qualified_user_id(uuid: &str, domain: &str) -> String {
    debug_assert!(!uuid.is_empty(), "uuid must be non-empty");
    debug_assert!(!domain.is_empty(), "domain must be non-empty");
    format!("{uuid}@{domain}")
}

/// Request body of `POST /v{N}/conversations/list-ids`. Both fields
/// are optional on the wire; Kupferdraht always sends `size` and sends
/// `paging_state` only on follow-up pages.
#[derive(Debug, Clone, Serialize)]
pub struct ConversationIdsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paging_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
}

/// Response body of `POST /v{N}/conversations/list-ids`.
/// Follow-up pages echo `paging_state`; the first page sends none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConversationIdsPage {
    pub qualified_conversations: Vec<QualifiedId>,
    pub has_more: bool,
    pub paging_state: String,
}

/// Request body of `POST /v{N}/conversations/list` — up to 1000
/// qualified IDs at a time per Wire's schema.
#[derive(Debug, Clone, Serialize)]
pub struct ListConversationsRequest<'a> {
    pub qualified_ids: &'a [QualifiedId],
}

/// Response body of `POST /v{N}/conversations/list`. Wire partitions
/// the input into three buckets: metadata-we-could-fetch,
/// metadata-we-could-not-fetch (transient federation / network),
/// and the-id-does-not-exist-or-is-deleted.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConversationsResponse {
    /// Step 99: decoded entry by entry — a conversation of a kind Kupferdraht
    /// does not know (a new `type` or `protocol`) is logged and skipped.
    #[serde(deserialize_with = "skip_undecodable_entries")]
    pub found: Vec<OwnConversation>,
    pub not_found: Vec<QualifiedId>,
    pub failed: Vec<QualifiedId>,
}

/// Wire's `ConvType` integer — the discriminator on every
/// conversation row. Kupferdraht maps this into `rooms::RoomType` at the
/// `wire::dto` → `rooms` boundary; consumers never see the integer
/// directly.
///
/// Values per Wire's swagger (confirmed against the `ConvType` enum):
///
/// | Wire | Kupferdraht `RoomType`    |
/// |------|----------------------|
/// | 0    | `Group`              |
/// | 1    | `SelfRoom`           |
/// | 2    | `DirectMessage`      |
/// | 3    | `Channel`            |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum ConvType {
    Group = 0,
    SelfRoom = 1,
    DirectMessage = 2,
    Channel = 3,
}

impl TryFrom<u8> for ConvType {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Group),
            1 => Ok(Self::SelfRoom),
            2 => Ok(Self::DirectMessage),
            3 => Ok(Self::Channel),
            other => Err(format!("unknown Wire ConvType integer: {other}")),
        }
    }
}

impl From<ConvType> for u8 {
    fn from(value: ConvType) -> Self {
        value as Self
    }
}

/// Wire's per-conversation protocol marker. Kupferdraht filters rows whose
/// protocol is `"proteus"` out at the `wire::dto` → `Room` boundary;
/// `"mls"` and `"mixed"` rooms pass through.
///
/// Schema: `reference/wire.swagger.json#/components/schemas/Protocol`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Proteus,
    Mls,
    Mixed,
}

/// Subset of `OwnConversation` from Wire's swagger that Kupferdraht
/// consumes at Step 2 — enough to materialize `Room` + members with
/// counterpart resolution. Extra Wire fields (`cells_state`,
/// `receipt_mode`, …) are intentionally ignored via serde's default
/// behaviour. Step 8b surfaces `access` + `access_role` so the
/// public API can show / change guest-link permissions.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OwnConversation {
    pub qualified_id: QualifiedId,
    #[serde(rename = "type")]
    pub conv_type: ConvType,
    pub protocol: Protocol,
    #[serde(default)]
    pub epoch: u64,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub members: OwnConvMembers,
    /// Wire's `access` field — list of join modes the conversation
    /// permits. Common values: `"invite"` (member-only),
    /// `"code"` (reusable guest link via Step 8 / `Room::create_join_link`),
    /// `"link"`, `"private"`. Empty `Vec` when Wire omits the field
    /// (legacy 1:1 conversations).
    #[serde(default)]
    pub access: Vec<String>,
    /// Wire's `access_role` field — which user classes may join.
    /// Common values: `"team_member"`, `"non_team_member"`,
    /// `"guest"`, `"service"`.
    #[serde(default, rename = "access_role")]
    pub access_role: Vec<String>,
    /// Wire's `team` field — the owning team's UUID (unqualified) for
    /// team conversations, `None` for non-team / 1:1 conversations.
    /// Required by team-conversation delete
    /// (`DELETE /teams/{team}/conversations/{id}`).
    #[serde(default)]
    pub team: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OwnConvMembers {
    #[serde(rename = "self")]
    pub self_member: ConvMemberSelf,
    pub others: Vec<ConvMemberOther>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConvMemberSelf {
    pub qualified_id: QualifiedId,
    /// Wire's `conversation_role` (`RoleName`): `wire_admin` /
    /// `wire_member`, or a custom name. Absent on some legacy rows.
    #[serde(default)]
    pub conversation_role: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConvMemberOther {
    pub qualified_id: QualifiedId,
    /// See [`ConvMemberSelf::conversation_role`].
    #[serde(default)]
    pub conversation_role: Option<String>,
}

/// Whether a Wire `conversation_role` value grants conversation
/// administration. The `wire_admin` literal is a Wire name and stays
/// inside `wire/` (ADR-0009) — callers outside map the bool onto
/// their own vocabulary. Absent roles and custom role names are not
/// admin (Step 72a; custom roles are out of Kupferdraht's scope).
#[must_use]
pub fn conversation_role_is_admin(role: Option<&str>) -> bool {
    role == Some("wire_admin")
}

/// Request body of `POST /v{N}/list-users`. The endpoint accepts
/// either `qualified_ids` or `qualified_handles` (but not both);
/// Kupferdraht uses ids during `sync_metadata`'s display-name backfill,
/// and switches to handles for Step 6-II-g's @-mention resolver.
#[derive(Debug, Clone, Serialize)]
pub struct ListUsersRequest<'a> {
    pub qualified_ids: &'a [QualifiedId],
}

/// Wire's `qualified_handles` element shape. Step 6-II-g.
#[derive(Debug, Clone, Serialize)]
pub struct QualifiedHandle {
    pub handle: String,
    pub domain: String,
}

/// Request body of `POST /v{N}/list-users` when querying by handle.
/// Step 6-II-g.
#[derive(Debug, Clone, Serialize)]
pub struct ListUsersByHandlesRequest<'a> {
    pub qualified_handles: &'a [QualifiedHandle],
}

/// Response body of `POST /v{N}/list-users`. `found` carries the
/// `UserProfile` of each resolved user; `failed` is the subset of
/// ids that could not be fetched (typically remote-federation
/// timeouts). Kupferdraht keeps whatever is in `found` and leaves the
/// `failed` ids without a display-name backfill.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ListUsersResponse {
    /// Step 99: decoded entry by entry — a profile that does not decode
    /// is logged and skipped (the name backfill is best-effort).
    #[serde(deserialize_with = "skip_undecodable_entries")]
    pub found: Vec<UserProfile>,
    #[serde(default)]
    pub failed: Vec<QualifiedId>,
}

/// Subset of Wire's `UserProfile` that Kupferdraht consumes. Extra fields
/// (`accent_id`, `legalhold_status`, `assets`, ...) are ignored via
/// serde's default behaviour.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserProfile {
    pub qualified_id: QualifiedId,
    pub name: String,
    #[serde(default)]
    pub handle: Option<String>,
}

/// Response body of `GET /v{N}/self`.
///
/// Field names match the Wire Swagger; Wire's response includes more
/// fields than we consume. `serde`'s default behaviour ignores the
/// extras.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SelfUser {
    pub id: String,
    pub qualified_id: QualifiedId,
    #[serde(default)]
    pub handle: Option<String>,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    /// The user's own team UUID (unqualified), `None` for personal
    /// accounts. Wire's conversation-list response often omits a
    /// conversation's owning `team`, so this is the fallback source for
    /// the team id when deleting a team conversation (Step 23).
    #[serde(default)]
    pub team: Option<String>,
}

/// One element of `GET /v{N}/clients`. Wire's `Client` schema carries
/// more fields (`mls_public_keys`, `prekeys`, `cookie`, …) — Kupferdraht's
/// 1d device-management surface only consumes the user-visible ones.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WireClient {
    pub id: String,
    #[serde(rename = "type", default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub last_active: Option<String>,
}

/// Body of `PUT /v{N}/conversations/{domain}/{cnv}/access` — Step 8b.
///
/// Kupferdraht uses `Vec<String>` (rather than typed enums) on the wire
/// boundary because the lists' valid values change as Wire rolls
/// out new access modes / roles. Validation lives at the
/// public-API boundary (`Client::set_room_access` rejects empty
/// lists; the rest is passthrough so a fresh enum value works
/// without a Kupferdraht release).
#[derive(Debug, Clone, Serialize)]
pub struct ConversationAccessData<'a> {
    pub access: &'a [String],
    pub access_role: &'a [String],
}

/// Body of `POST /v{N}/conversations/{cnv}/code` — Step 8.
///
/// `password` is optional; supplying it produces a password-protected
/// link, omitting it (or sending `null`) produces an open link. Wire's
/// schema requires `min_length=8` when the field is present —
/// validation lives at the Kupferdraht public-API boundary, not here.
#[derive(Clone, Serialize, Default)]
pub struct CreateConversationCodeRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<&'a str>,
}

impl std::fmt::Debug for CreateConversationCodeRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("CreateConversationCodeRequest")
            .field(
                "password",
                &crate::redact::hidden_option(self.password.as_ref()),
            )
            .finish()
    }
}

/// Response of `POST /v{N}/conversations/{cnv}/code` (200 when the
/// code already existed) and `GET /v{N}/conversations/{cnv}/code`
/// (200 when one is on file). The Kupferdraht-public `JoinLink` is a
/// projection of this — `uri` becomes `JoinLink::url`,
/// `has_password` becomes `JoinLink::password_protected`. `key` and
/// `code` are echoed for callers that build URLs themselves.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ConversationCodeInfo {
    pub key: String,
    pub code: String,
    pub uri: String,
    pub has_password: bool,
}

impl std::fmt::Debug for ConversationCodeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("ConversationCodeInfo")
            .field("key", &crate::redact::Hidden)
            .field("code", &crate::redact::Hidden)
            .field("uri", &crate::redact::Hidden)
            .field("has_password", &self.has_password)
            .finish()
    }
}

/// Answer of `GET /v{N}/conversations/join?key&code` — Step 122: the
/// conversation a guest link names (`ConversationCoverView`, a limited
/// view; `name` and `has_password` are not read). `id` is unqualified: a
/// link names a conversation of the answering backend.
#[derive(Debug, Clone, Deserialize)]
pub struct ConversationCoverView {
    pub id: String,
}

/// Body of `POST /v{N}/conversations/join` — Step 8 redeem path.
/// `password` is required when the link is password-protected and
/// rejected (`403 invalid-conversation-password`) otherwise.
#[derive(Clone, Serialize)]
pub struct JoinConversationByCode<'a> {
    pub key: &'a str,
    pub code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<&'a str>,
}

impl std::fmt::Debug for JoinConversationByCode<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("JoinConversationByCode")
            .field("key", &crate::redact::Hidden)
            .field("code", &crate::redact::Hidden)
            .field(
                "password",
                &crate::redact::hidden_option(self.password.as_ref()),
            )
            .finish()
    }
}

/// Body of `POST /register` (unversioned) — Step 16a.
///
/// Wire's `NewUser` schema carries 19 optional fields; only `name` is
/// required. Kupferdraht's personal-registration flow always supplies
/// `name + email + password`. `sso_id` is plumbed through but
/// unconsumed by Step 16 (Step 17 wires SAML on top). Every other
/// `NewUser` field (`team`, `team_id`, `picture`, `assets`,
/// `accent_id`, `invitation_code`, `expires_in`, `managed_by`,
/// `supported_protocols`, `uuid`, `email_code`) is intentionally not
/// exposed.
#[derive(Clone, Serialize, Default)]
pub struct NewUserRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sso_id: Option<UserSSOId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl std::fmt::Debug for NewUserRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("NewUserRequest")
            .field("name", &self.name)
            .field("email", &self.email)
            .field(
                "password",
                &crate::redact::hidden_option(self.password.as_ref()),
            )
            .field("sso_id", &self.sso_id)
            .field(
                "team_code",
                &crate::redact::hidden_option(self.team_code.as_ref()),
            )
            .field("locale", &self.locale)
            .field("label", &self.label)
            .finish()
    }
}

/// Wire's `UserSSOId` schema — all three components optional.
/// Externally constrained by Wire's swagger; the `PascalCase` name and
/// `snake_case` fields match the schema verbatim (ADR-0009).
///
/// Plumbed through `NewUserRequest.sso_id` and `ActivationResponse.sso_id`
/// but never constructed by Kupferdraht today — Step 17 wires the SAML
/// surface that produces and consumes it.
#[allow(dead_code)] // production constructor arrives with Step 17 (SAML/SSO)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UserSSOId {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scim_external_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
}

/// Response body of `GET /sso/settings` — Wire's `SsoSettings` schema
/// (Step 17β). Carries the team's **default** SSO code (a UUID) when one
/// `IdP` is configured; absent when the backend has no default provider.
/// Wire v14 exposes no per-email-domain discovery, so this single default
/// is all `ssoLogin(email)` can resolve — multi-`IdP` backends must supply an
/// explicit code via `ssoLoginWithCode`.
#[derive(Debug, Clone, Deserialize)]
pub struct SsoSettings {
    #[serde(default)]
    pub default_sso_code: Option<String>,
}

/// Response body of `POST /register` (HTTP 201) — Wire's `User`
/// schema, subset of fields Kupferdraht consumes today.
///
/// `qualified_id` is required per swagger and becomes Kupferdraht's
/// canonical handle for the freshly-registered account.
/// Schema-fidelity fields (`name`, `status`, `email`) are decoded
/// and exposed for inspection but not threaded into `PendingClient`
/// today — `register` keeps just `qualified_id.id` and echoes the
/// caller's `email` input.
#[allow(dead_code)] // schema-fidelity fields surface in inline tests only
#[derive(Debug, Clone, Deserialize)]
pub struct RegisterResponse {
    pub qualified_id: QualifiedId,
    pub name: String,
    pub status: AccountStatus,
    #[serde(default)]
    pub email: Option<String>,
}

/// Wire's `AccountStatus` enum (swagger component). Kebab-case wire
/// representation: `pending-invitation` maps to `PendingInvitation`.
/// The set is closed; an unknown variant decodes to a serde error.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountStatus {
    Active,
    Suspended,
    Deleted,
    Ephemeral,
    PendingInvitation,
}

/// Body of `POST /activate` (unversioned) — Step 16b.
///
/// Wire's `Activate` schema requires both `code` and `dryrun`, plus
/// **at least one** of `key`, `email`, `phone`. Kupferdraht-CLI's flow is
/// the typed-in-code variant against a known email address; we always
/// supply `code + email + dryrun: false`. The `key` and `phone` fields
/// stay in the DTO for future flexibility but are not wired today.
#[derive(Clone, Serialize)]
pub struct ActivateRequest<'a> {
    pub code: &'a str,
    pub dryrun: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<&'a str>,
}

impl std::fmt::Debug for ActivateRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("ActivateRequest")
            .field("code", &crate::redact::Hidden)
            .field("dryrun", &self.dryrun)
            .field("email", &self.email)
            .field("key", &crate::redact::hidden_option(self.key.as_ref()))
            .field("phone", &self.phone)
            .finish()
    }
}

/// Response body of `POST /activate` (HTTP 200) — Wire's
/// `ActivationResponse` schema. All fields are optional. `first: true`
/// indicates the very first activation for the account (i.e. account
/// activation, not a follow-up email-change confirmation). `sso_id` is
/// populated only for accounts that pre-registered with SAML/SCIM —
/// Step 17 territory; in Step 16 we just thread it through.
///
/// 16c's `PendingClient::activate` discards the decoded response (the
/// follow-up login pipeline produces the authoritative `Client`). The
/// fields are kept for schema-fidelity inline tests today; Step 17
/// promotes `sso_id` into the post-activate flow.
#[allow(dead_code)] // schema-fidelity fields surface in inline tests only
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ActivationResponse {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub first: Option<bool>,
    #[serde(default)]
    pub sso_id: Option<UserSSOId>,
}

/// Body of `POST /activate/send` (unversioned) — Step 16b resend path.
/// Wire's `SendActivationCode` schema makes `email` required; `locale`
/// is optional and we leave it unset (Wire defaults to "en").
#[derive(Debug, Clone, Serialize)]
pub struct SendActivationCodeRequest<'a> {
    pub email: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<&'a str>,
}

/// Decode a JSON array entry by entry (Step 99): an entry that does not
/// decode as `T` is logged with its `qualified_id`, when it has one, and
/// skipped — one entry of a kind Kupferdraht does not know must not fail the
/// whole list, and with it the sync.
fn skip_undecodable_entries<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let total = raw.len();
    let decoded: Vec<T> = raw.into_iter().filter_map(decode_or_skip).collect();
    debug_assert!(decoded.len() <= total, "skipping never adds entries");
    Ok(decoded)
}

/// One list entry as `T`, or `None` after a warning naming it.
fn decode_or_skip<T: DeserializeOwned>(entry: serde_json::Value) -> Option<T> {
    let id = entry
        .get("qualified_id")
        .and_then(|q| q.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    match serde_json::from_value::<T>(entry) {
        Ok(decoded) => Some(decoded),
        Err(e) => {
            tracing::warn!(id = id.as_deref().unwrap_or("?"), error = %e, "skipping an undecodable list entry");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_id_display_formats_as_id_at_domain() {
        let qid = QualifiedId {
            id: "abc123".into(),
            domain: "example.com".into(),
        };
        assert_eq!(format!("{qid}"), "abc123@example.com");
    }

    #[test]
    fn qualified_id_to_string_matches_display() {
        let qid = QualifiedId {
            id: "user-uuid".into(),
            domain: "wire.com".into(),
        };
        assert_eq!(qid.to_string(), "user-uuid@wire.com");
    }

    #[test]
    fn qualified_id_from_user_id_splits_localpart_and_domain() {
        let qid = QualifiedId::from(&UserId::new("user-uuid", "wire.com"));
        assert_eq!(qid.id, "user-uuid");
        assert_eq!(qid.domain, "wire.com");
    }
}
