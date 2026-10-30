// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

import XCTest

import Kupferdraht

// Step 20b: exercise the published `Kupferdraht` package — the same surface
// Natrium's iOS layer consumes — through SwiftPM, proving the
// xcframework links and the UniFFI bridge runs end to end.
final class KupferdrahtTests: XCTestCase {
    func testVersionFlowsThroughFromRust() {
        XCTAssertEqual(kupferdrahtVersion(), "0.1.0")
    }

    func testParseUserIdReturnsStructuredParts() throws {
        let id = try parseUserId(id: "alice@example.test")
        XCTAssertEqual(id.localpart, "alice")
        XCTAssertEqual(id.domain, "example.test")
    }

    func testParseUserIdRejectsMalformedId() {
        XCTAssertThrowsError(try parseUserId(id: "no-domain")) { error in
            guard let kupferdrahtError = error as? KupferdrahtError,
                  case .InvalidInput = kupferdrahtError
            else {
                return XCTFail("expected KupferdrahtError.InvalidInput, got \(error)")
            }
        }
    }
}

/// Compile-time pin of the `SyncListener` contract: `onError` returns a
/// Bool (true = keep the sync loop alive, retry after backoff). A
/// runtime exercise needs a logged-in client, which the offline test
/// suite cannot provide.
final class PinnedSyncListener: SyncListener, @unchecked Sendable {
    func onSummary(summary: SyncSummary) -> Bool { true }
    func onError(description: String) -> Bool { false }
}

/// A foreign-implemented `SecureKeyStore` — the `with_foreign` callback
/// the embedder normally backs with the iOS Keychain. Here an in-memory
/// dictionary, single-threaded for the test.
final class InMemoryKeystore: SecureKeyStore, @unchecked Sendable {
    private var store: [String: Data] = [:]
    func getSecret(id: String) throws -> Data? { store[id] }
    func setSecret(id: String, secret: Data) throws { store[id] = secret }
    func deleteSecret(id: String) throws { store.removeValue(forKey: id) }
}

// Step 20c: exercise the manager surface end to end — a Swift-implemented
// `SecureKeyStore` (the `with_foreign` callback, called from Rust),
// `Kupferdraht.open` (which keys `global.db` through that callback), and an
// async method, all without a network round-trip.
final class KupferdrahtManagerTests: XCTestCase {
    func testManagerOpensAndStartsEmpty() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let kupferdraht = try Kupferdraht.open(baseDir: dir.path, keystore: InMemoryKeystore())

        let accounts = try await kupferdraht.accounts()
        XCTAssertTrue(accounts.isEmpty, "a fresh device has no registered accounts")

        let active = try await kupferdraht.activeAccount()
        XCTAssertNil(active, "no active account on a fresh device")
    }

    // Registration is a manager method (no free function, no
    // caller-supplied keystore): the https-only guard fires before any
    // network I/O, so the test runs offline.
    func testManagerRegisterRejectsPlainHttp() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let kupferdraht = try Kupferdraht.open(baseDir: dir.path, keystore: InMemoryKeystore())
        let server = ServerConfig(
            apiBaseUrl: "http://insecure.example",
            websocketUrl: "ws://insecure.example",
            title: "smoke"
        )

        do {
            _ = try await kupferdraht.register(
                server: server,
                name: "Alice",
                email: "alice@example.test",
                password: "hunter2hunter2"
            )
            XCTFail("plain-http server url must reject via the https guard")
        } catch let error as KupferdrahtError {
            guard case .InvalidInput = error else {
                return XCTFail("expected KupferdrahtError.InvalidInput, got \(error)")
            }
        }
    }
}
