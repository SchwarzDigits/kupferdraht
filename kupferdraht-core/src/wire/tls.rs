// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 156: the certificate authorities an app adds to the built-in Mozilla
//! list for Kupferdraht's native TLS connections — never in its place, and
//! nothing turns the check off. The browser checks with its own list: there
//! the anchors have no effect and are not parsed.

use crate::error::KupferdrahtError;

/// The app's own trust anchors, parsed from PEM; empty by default.
#[derive(Clone, Debug, Default)]
pub(crate) struct TrustAnchors {
    /// Each added certificate, DER.
    certificates: Vec<Vec<u8>>,
}

impl TrustAnchors {
    /// Parse each PEM text of `pems`, each holding one or more certificates.
    ///
    /// # Errors
    /// As [`Self::from_pem`].
    pub(crate) fn from_pems(pems: &[String]) -> Result<Self, KupferdrahtError> {
        let mut certificates = Vec::new();
        for pem in pems {
            certificates.extend(Self::from_pem(pem)?.certificates);
        }
        Ok(Self { certificates })
    }

    /// Parse PEM text holding one or more certificates.
    ///
    /// # Errors
    /// `InvalidInput` for text without a certificate, or with one that does
    /// not parse.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn from_pem(pem: &str) -> Result<Self, KupferdrahtError> {
        use rustls::pki_types::CertificateDer;
        use rustls::pki_types::pem::PemObject as _;
        let invalid =
            |why: String| KupferdrahtError::InvalidInput(format!("root certificates: {why}"));
        let mut parsed = rustls::RootCertStore::empty();
        let mut certificates = Vec::new();
        for der in CertificateDer::pem_slice_iter(pem.as_bytes()) {
            let der = der.map_err(|e| invalid(format!("not PEM ({e})")))?;
            parsed
                .add(der.clone())
                .map_err(|e| invalid(format!("not a certificate ({e})")))?;
            certificates.push(der.to_vec());
        }
        if certificates.is_empty() {
            return Err(invalid("no certificate in the PEM text".into()));
        }
        Ok(Self { certificates })
    }

    /// In the browser the anchors have no effect — the browser checks with
    /// its own list — so nothing is kept; the text must still hold a
    /// certificate, as natively.
    ///
    /// # Errors
    /// `InvalidInput` for text without a certificate.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn from_pem(pem: &str) -> Result<Self, KupferdrahtError> {
        if pem.contains("-----BEGIN CERTIFICATE-----") {
            Ok(Self::default())
        } else {
            Err(KupferdrahtError::InvalidInput(
                "root certificates: no certificate in the PEM text".into(),
            ))
        }
    }

    /// How many anchors the app added.
    #[cfg(test)]
    pub(crate) fn added(&self) -> usize {
        self.certificates.len()
    }

    /// The anchors as reqwest adds them beside its built-in roots.
    ///
    /// # Errors
    /// `InvalidInput` for a certificate reqwest does not take.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn reqwest_certificates(
        &self,
    ) -> Result<Vec<reqwest::Certificate>, KupferdrahtError> {
        self.certificates
            .iter()
            .map(|der| {
                reqwest::Certificate::from_der(der)
                    .map_err(|e| KupferdrahtError::InvalidInput(format!("root certificates: {e}")))
            })
            .collect()
    }

    /// The Mozilla list plus the added anchors.
    #[cfg(not(target_arch = "wasm32"))]
    fn root_store(&self) -> rustls::RootCertStore {
        let mut store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        for der in &self.certificates {
            let added = store.add(rustls::pki_types::CertificateDer::from(der.clone()));
            debug_assert!(added.is_ok(), "every anchor parsed in from_pem");
        }
        store
    }

    /// The WebSocket's TLS configuration when the app added anchors;
    /// `None` keeps tokio-tungstenite's own, over the Mozilla list alone.
    ///
    /// # Errors
    /// `Network` if rustls refuses its default protocol versions.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn websocket_config(
        &self,
    ) -> Result<Option<std::sync::Arc<rustls::ClientConfig>>, KupferdrahtError> {
        if self.certificates.is_empty() {
            return Ok(None);
        }
        let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| KupferdrahtError::Network(format!("websocket TLS configuration: {e}")))?
        .with_root_certificates(self.root_store())
        .with_no_client_auth();
        Ok(Some(std::sync::Arc::new(config)))
    }

    /// The WebSocket's root store size.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn websocket_root_count(&self) -> usize {
        self.root_store().len()
    }
}

/// The reason, when `err` or an error it wraps is rustls refusing the
/// server's certificate (Step 156): an unknown issuer, an expired
/// certificate, one for another host.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn certificate_rejection(err: &(dyn std::error::Error + 'static)) -> Option<String> {
    if let Some(rustls::Error::InvalidCertificate(why)) = err.downcast_ref() {
        return Some(format!("{why:?}"));
    }
    // `io::Error::source` skips the error an `io::Error` carries, and hyper
    // nests one `io::Error` in another: look inside first.
    let carried = err
        .downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::get_ref)
        .map(|inner| inner as &(dyn std::error::Error + 'static));
    carried
        .and_then(certificate_rejection)
        .or_else(|| err.source().and_then(certificate_rejection))
}
