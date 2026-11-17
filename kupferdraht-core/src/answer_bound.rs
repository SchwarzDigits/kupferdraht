// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Every answer read into memory goes through a bound on its decoded bytes
//! (Step 159). A server's oversized answer, or a compressed one that inflates,
//! stops at the bound instead of filling memory. The browser's asset download
//! reads through the same [`read_bounded`], bounded by the size its sender
//! declared (Step 160).

use futures_util::StreamExt as _;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::error::KupferdrahtError;

/// The most decoded bytes of one answer: 16 MiB, the bound `wire-server-rs`
/// sets on a decoded request body at galley and gundeck.
pub(crate) const MAX_ANSWER_BYTES: usize = 16 * 1024 * 1024;

/// An answer that could not be read.
#[derive(Debug, Error)]
pub(crate) enum AnswerError {
    /// More decoded bytes than the bound.
    #[error("answer over {0} bytes")]
    OverTheBound(usize),
    /// The transport or the body's decoding failed while reading.
    #[error("{0}")]
    Read(reqwest::Error),
    /// The body is not the JSON the caller expects.
    #[error("{0}")]
    Malformed(serde_json::Error),
}

impl AnswerError {
    /// As a [`KupferdrahtError`] naming `what`: past the bound
    /// `BackendRejected`, as the avatar's own bound; a failed read or a
    /// malformed body the caller's variant `otherwise`.
    pub(crate) fn into_error(
        self,
        what: &str,
        otherwise: impl FnOnce(String) -> KupferdrahtError,
    ) -> KupferdrahtError {
        let detail = format!("{what}: {self}");
        match self {
            Self::OverTheBound(_) => KupferdrahtError::BackendRejected(detail),
            Self::Read(_) | Self::Malformed(_) => otherwise(detail),
        }
    }
}

/// The answer's body, up to [`MAX_ANSWER_BYTES`].
pub(crate) async fn read_answer(resp: reqwest::Response) -> Result<Vec<u8>, AnswerError> {
    read_bounded(resp, MAX_ANSWER_BYTES).await
}

/// The answer's body as text, up to [`MAX_ANSWER_BYTES`]; invalid UTF-8 is
/// replaced, as a backend's error text is only ever shown.
pub(crate) async fn read_text(resp: reqwest::Response) -> Result<String, AnswerError> {
    let body = read_answer(resp).await?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// The answer's JSON body as `T`, up to [`MAX_ANSWER_BYTES`].
pub(crate) async fn read_json<T: DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, AnswerError> {
    let body = read_answer(resp).await?;
    serde_json::from_slice(&body).map_err(AnswerError::Malformed)
}

/// The body, up to `max_bytes` decoded bytes: reading stops at the first
/// chunk past it, natively and in the browser alike (Step 160). The declared
/// length, capped at the bound, reserves the buffer once. A read error drops
/// its URL, which can carry a secret (a join code, a signed asset URL).
pub(crate) async fn read_bounded(
    resp: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, AnswerError> {
    debug_assert!(max_bytes > 0, "a bound admits an answer");
    let declared = resp.content_length().and_then(|n| usize::try_from(n).ok());
    let mut body = Vec::with_capacity(declared.unwrap_or(0).min(max_bytes));
    let mut chunks = resp.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| AnswerError::Read(e.without_url()))?;
        if body.len() + chunk.len() > max_bytes {
            return Err(AnswerError::OverTheBound(max_bytes));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
