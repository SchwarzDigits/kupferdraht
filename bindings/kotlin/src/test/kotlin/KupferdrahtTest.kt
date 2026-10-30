// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.runBlocking
import uniffi.kupferdraht.QualifiedUserId
import uniffi.kupferdraht.SecureKeyStore
import uniffi.kupferdraht.ServerConfig
import uniffi.kupferdraht.Kupferdraht
import uniffi.kupferdraht.KupferdrahtException
import uniffi.kupferdraht.SyncListener
import uniffi.kupferdraht.SyncSummary
import uniffi.kupferdraht.parseUserId
import uniffi.kupferdraht.kupferdrahtVersion

// Step 20b: exercise the published Kotlin/JVM package — the same
// surface Natrium's JVM layer consumes — through the JNA-loaded native
// library, proving the proc-macro -> uniffi -> Kotlin -> native bridge
// runs end to end.
class KupferdrahtTest {
    @Test
    fun versionFlowsThroughFromRust() {
        assertEquals("0.1.0", kupferdrahtVersion())
    }

    @Test
    fun parseUserIdReturnsStructuredParts() {
        val id: QualifiedUserId = parseUserId("alice@example.test")
        assertEquals("alice", id.localpart)
        assertEquals("example.test", id.domain)
    }

    @Test
    fun parseUserIdRejectsMalformedId() {
        assertFailsWith<KupferdrahtException.InvalidInput> {
            parseUserId("no-domain")
        }
    }
}

/// Compile-time pin of the `SyncListener` contract: `onError` returns a
/// Boolean (true = keep the sync loop alive, retry after backoff). A
/// runtime exercise needs a logged-in client, which the offline test
/// suite cannot provide.
@Suppress("unused")
class PinnedSyncListener : SyncListener {
    override fun onSummary(summary: SyncSummary): Boolean = true
    override fun onError(description: String): Boolean = false
}

/// A foreign-implemented `SecureKeyStore` — the `with_foreign` callback
/// the embedder normally backs with the Android Keystore. Here an
/// in-memory map, single-threaded for the test.
class InMemoryKeystore : SecureKeyStore {
    private val store = HashMap<String, ByteArray>()
    override fun getSecret(id: String): ByteArray? = store[id]
    override fun setSecret(id: String, secret: ByteArray) {
        store[id] = secret
    }
    override fun deleteSecret(id: String) {
        store.remove(id)
    }
}

// Step 20c: exercise the manager surface end to end — a Kotlin-implemented
// `SecureKeyStore` (the JNA-backed `with_foreign` callback, called from
// Rust), `Kupferdraht.open` (which keys `global.db` through that callback), and
// suspend methods, all without a network round-trip.
class KupferdrahtManagerTest {
    @Test
    fun managerOpensAndStartsEmpty() = runBlocking {
        val dir = Files.createTempDirectory("kupferdraht-test").toFile()
        try {
            Kupferdraht.open(dir.absolutePath, InMemoryKeystore()).use { kupferdraht ->
                assertTrue(kupferdraht.accounts().isEmpty(), "a fresh device has no accounts")
                assertNull(kupferdraht.activeAccount(), "no active account on a fresh device")
            }
        } finally {
            dir.deleteRecursively()
        }
    }

    // Registration is a manager method (no free function, no
    // caller-supplied keystore): the https-only guard fires before any
    // network I/O, so the test runs offline.
    @Test
    fun managerRegisterRejectsPlainHttp() = runBlocking {
        val dir = Files.createTempDirectory("kupferdraht-test").toFile()
        try {
            Kupferdraht.open(dir.absolutePath, InMemoryKeystore()).use { kupferdraht ->
                assertFailsWith<KupferdrahtException.InvalidInput> {
                    kupferdraht.register(
                        ServerConfig(
                            apiBaseUrl = "http://insecure.example",
                            websocketUrl = "ws://insecure.example",
                            title = "smoke",
                        ),
                        "Alice",
                        "alice@example.test",
                        "hunter2hunter2",
                    )
                }
                Unit
            }
        } finally {
            dir.deleteRecursively()
        }
    }
}
