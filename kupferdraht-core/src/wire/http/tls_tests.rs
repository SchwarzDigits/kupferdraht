// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 156: a server under a certificate authority made in the test —
//! refused without the authority, reached with it, by both HTTP clients (the
//! asset redirect follows the transfers' one) and by the WebSocket; and the
//! Mozilla list stays beside the added authorities.

use futures_util::StreamExt as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;

use super::HttpClient;
use crate::error::KupferdrahtError;
use crate::wire::tls::TrustAnchors;
use crate::wire::websocket::EventSocket;

const API_VERSION: &str = r#"{"supported":[14],"development":[]}"#;

/// A certificate authority and a server config under it for 127.0.0.1.
struct TestCa {
    pem: String,
    server: Arc<rustls::ServerConfig>,
}

fn test_ca() -> TestCa {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };
    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "Kupferdraht test CA");
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca.self_signed(&ca_key).expect("ca cert");
    let issuer = Issuer::new(ca, ca_key);
    let leaf_key = KeyPair::generate().expect("leaf key");
    let leaf = CertificateParams::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])
        .expect("leaf params")
        .signed_by(&leaf_key, &issuer)
        .expect("leaf cert");
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into()),
    )
    .expect("server config");
    TestCa {
        pem: ca_cert.pem(),
        server: Arc::new(server),
    }
}

/// An HTTPS server on 127.0.0.1 answering every request `200` with `body`.
async fn https_server(config: Arc<rustls::ServerConfig>, body: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let acceptor = TlsAcceptor::from(config);
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    addr
}

/// A WebSocket server over TLS on 127.0.0.1 that keeps each socket open.
async fn wss_server(config: Arc<rustls::ServerConfig>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let acceptor = TlsAcceptor::from(config);
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Ok(mut socket) = tokio_tungstenite::accept_async(tls).await else {
                    return;
                };
                while let Some(Ok(_)) = socket.next().await {}
            });
        }
    });
    addr
}

fn base_url(addr: SocketAddr) -> String {
    format!("https://127.0.0.1:{}", addr.port())
}

#[tokio::test]
async fn without_the_authority_the_server_is_refused_by_name() {
    let ca = test_ca();
    let addr = https_server(ca.server.clone(), API_VERSION).await;
    let http =
        HttpClient::with_trust(&base_url(addr), false, &TrustAnchors::default()).expect("client");

    let result = http.api_version().await;

    assert!(
        matches!(result, Err(KupferdrahtError::CertificateRejected(_))),
        "{:?}",
        result.err()
    );
}

#[tokio::test]
async fn with_the_authority_both_http_clients_reach_the_server() {
    let ca = test_ca();
    let addr = https_server(ca.server.clone(), API_VERSION).await;
    let trust = TrustAnchors::from_pem(&ca.pem).expect("anchors");
    let http = HttpClient::with_trust(&base_url(addr), false, &trust).expect("client");

    http.api_version()
        .await
        .expect("the API client trusts the authority");
    let transfer = http
        .transfer
        .get(format!("{}/asset", base_url(addr)))
        .send()
        .await
        .expect("the transfers' client, which follows the asset redirect, trusts it");

    assert!(transfer.status().is_success());
}

#[tokio::test]
async fn the_websocket_trusts_the_same_authorities() {
    let ca = test_ca();
    let addr = wss_server(ca.server.clone()).await;
    let url = url::Url::parse(&format!("wss://127.0.0.1:{}/v14/events", addr.port())).expect("url");

    let refused = EventSocket::connect_trusting(
        url.clone(),
        Duration::from_secs(5),
        &TrustAnchors::default(),
    )
    .await;
    assert!(
        matches!(refused, Err(KupferdrahtError::CertificateRejected(_))),
        "{:?}",
        refused.err()
    );

    let trust = TrustAnchors::from_pem(&ca.pem).expect("anchors");
    EventSocket::connect_trusting(url, Duration::from_secs(5), &trust)
        .await
        .expect("the socket trusts the authority");
}

/// The added authorities join the Mozilla list; they never replace it.
#[test]
fn the_mozilla_list_stays() {
    let trust = TrustAnchors::from_pem(&test_ca().pem).expect("anchors");

    assert_eq!(trust.added(), 1);
    assert_eq!(
        trust.websocket_root_count(),
        webpki_roots::TLS_SERVER_ROOTS.len() + 1
    );
}

#[test]
fn pem_text_may_hold_several_authorities_and_must_hold_one() {
    let two = format!("{}{}", test_ca().pem, test_ca().pem);
    assert_eq!(TrustAnchors::from_pem(&two).expect("two").added(), 2);

    for junk in [
        "",
        "not a certificate",
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    ] {
        assert!(
            matches!(
                TrustAnchors::from_pem(junk),
                Err(KupferdrahtError::InvalidInput(_))
            ),
            "{junk:?}"
        );
    }
}
