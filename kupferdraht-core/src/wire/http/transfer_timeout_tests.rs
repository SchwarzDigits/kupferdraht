// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 98 — asset transfers are not cut off by the request timeout
//! (review finding H1). TDD red.
//!
//! `reqwest`'s client-wide `timeout` is a total deadline "from when the
//! request starts connecting until the response body has finished" — it
//! also cuts off a transfer whose bytes keep flowing. A loopback HTTP
//! server written here streams or reads slowly (httpmock can only delay a
//! whole response); the timeouts are shrunk to milliseconds.

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use super::assets::AssetMultipart;
use super::{HttpClient, HttpTimeouts};
use crate::error::KupferdrahtError;

const CHUNK: usize = 4 * 1024;

/// A request timeout of 150 ms — far below every transfer here.
fn short(request: Duration) -> HttpTimeouts {
    HttpTimeouts {
        connect: Duration::from_secs(2),
        request,
        transfer_idle: Duration::from_millis(300),
    }
}

async fn loopback() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    (listener, base)
}

/// Read the request head (up to the blank line); returns it and whatever
/// body bytes arrived with it.
async fn read_head(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1024];
    loop {
        let n = stream.read(&mut byte).await.expect("read head");
        assert!(n > 0, "client closed before the head ended");
        buf.extend_from_slice(&byte[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            return (head, buf[end + 4..].to_vec());
        }
    }
}

fn content_length(head: &str) -> usize {
    head.lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().expect("content-length"))
        })
        .unwrap_or(0)
}

/// Serve one download: `chunks` × [`CHUNK`] bytes, `pause` apart; with
/// `stall_after` the server goes silent after that many chunks.
async fn serve_download(
    listener: TcpListener,
    chunks: usize,
    pause: Duration,
    stall_after: Option<usize>,
) {
    let (mut stream, _) = listener.accept().await.expect("accept");
    let _ = read_head(&mut stream).await;
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\n\r\n",
        chunks * CHUNK
    );
    stream.write_all(head.as_bytes()).await.expect("head");
    for i in 0..chunks {
        if stall_after == Some(i) {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
        stream.write_all(&[0xAB; CHUNK]).await.expect("chunk");
        stream.flush().await.expect("flush");
        tokio::time::sleep(pause).await;
    }
}

/// Serve one upload: read the body `CHUNK` bytes per `pause`, then
/// answer 201.
async fn serve_upload(listener: TcpListener, pause: Duration) {
    let (mut stream, _) = listener.accept().await.expect("accept");
    let (head, early) = read_head(&mut stream).await;
    let mut remaining = content_length(&head).saturating_sub(early.len());
    let mut buf = [0u8; CHUNK];
    while remaining > 0 {
        let n = stream
            .read(&mut buf[..remaining.min(CHUNK)])
            .await
            .expect("body");
        assert!(n > 0, "client closed mid-body");
        remaining -= n;
        tokio::time::sleep(pause).await;
    }
    let body = r#"{"key":"3-1-slow","token":"tok","domain":"example.test"}"#;
    let reply = format!(
        "HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(reply.as_bytes()).await.expect("reply");
}

async fn upload(http: &HttpClient, file: &Path, size: u64) -> Result<(), KupferdrahtError> {
    http.upload_asset(
        14,
        "tok",
        AssetMultipart {
            conv_id: None,
            filename: Some("slow.bin"),
            filetype: Some("application/octet-stream"),
            ciphertext_path: file,
            ciphertext_size: size,
            md5_b64: "AAAAAAAAAAAAAAAAAAAAAA==",
        },
    )
    .await
    .map(|_| ())
}

fn ciphertext_file(dir: &tempfile::TempDir, size: usize) -> std::path::PathBuf {
    let path = dir.path().join("ct.bin");
    std::fs::write(&path, vec![0x5A; size]).expect("write ciphertext");
    path
}

/// H1 itself: a download whose bytes keep flowing for ~0.5 s must not be
/// cut off by a 150 ms request timeout.
#[tokio::test]
async fn a_download_that_keeps_flowing_outlives_the_request_timeout() {
    let (listener, base) = loopback().await;
    let server = tokio::spawn(serve_download(listener, 8, Duration::from_millis(60), None));
    let http =
        HttpClient::with_timeouts(&base, true, short(Duration::from_millis(150))).expect("client");
    let dir = tempfile::tempdir().expect("tempdir");
    let written = http
        .download_asset(
            14,
            "tok",
            "example.test",
            "key-1",
            None,
            &dir.path().join("out"),
        )
        .await
        .expect("a flowing download completes");
    assert_eq!(written, (8 * CHUNK) as u64);
    server.abort();
}

/// A download that goes silent fails after the idle timeout (300 ms) —
/// not only when a much longer request bound (5 s) runs out.
#[tokio::test]
async fn a_download_that_stalls_fails_after_the_idle_timeout() {
    let (listener, base) = loopback().await;
    let server = tokio::spawn(serve_download(
        listener,
        4,
        Duration::from_millis(10),
        Some(1),
    ));
    let http =
        HttpClient::with_timeouts(&base, true, short(Duration::from_secs(5))).expect("client");
    let dir = tempfile::tempdir().expect("tempdir");
    let started = Instant::now();
    let result = http
        .download_asset(
            14,
            "tok",
            "example.test",
            "key-1",
            None,
            &dir.path().join("out"),
        )
        .await;
    assert!(
        matches!(result, Err(KupferdrahtError::Network(_))),
        "a stalled download fails: {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "it fails after the idle timeout, took {:?}",
        started.elapsed()
    );
    server.abort();
}

/// An upload the server reads for ~1.3 s must not be cut off by a
/// 150 ms request timeout — uploads have no total bound.
#[tokio::test]
async fn an_upload_slower_than_the_request_timeout_completes() {
    let (listener, base) = loopback().await;
    let server = tokio::spawn(serve_upload(listener, Duration::from_millis(20)));
    let http =
        HttpClient::with_timeouts(&base, true, short(Duration::from_millis(150))).expect("client");
    let dir = tempfile::tempdir().expect("tempdir");
    let size = 256 * 1024;
    let file = ciphertext_file(&dir, size);
    upload(&http, &file, size as u64)
        .await
        .expect("a slow but steady upload completes");
    server.abort();
}
