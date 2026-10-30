#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Schwarz Digits KG
# SPDX-License-Identifier: GPL-3.0-only

# Step 91 (ADR-0022): run kupferdraht-core's browser tests in headless Firefox.
#
# The wasm tests are integration tests only (`kupferdraht-core/tests/*.rs` with
# `#![cfg(target_arch = "wasm32")]`): the crate's unit-test harness pulls
# native-only dev-dependencies (httpmock, tempfile) and is never built for
# wasm, which is also why `wasm-pack test` (it builds `--tests`) is not the
# entry point. This script builds each wasm test target with cargo and runs
# it through `wasm-bindgen-test-runner` — the OPFS tests in a dedicated
# worker, the keystore/fetch tests and the Step-94 tabs scenarios (the
# `bindings/web/kupferdraht-tabs/` module, loaded as wasm-bindgen snippets) on
# the main thread.
#
# Needs: the `wasm32-unknown-unknown` target, a wasm-capable clang for the
# SQLite3MC C (Apple clang has none — `brew install llvm` and export
# CC_wasm32_unknown_unknown / AR_wasm32_unknown_unknown, see
# docs/bindings.md), Firefox + geckodriver, and `wasm-bindgen-test-runner`
# at the lockfile's wasm-bindgen version (on PATH, or wasm-pack's cache).
set -euo pipefail

cd "$(dirname "$0")/.."

WASM_BINDGEN_VERSION="$(awk '/^name = "wasm-bindgen"$/{getline; sub(/version = "/, ""); sub(/"/, ""); print; exit}' Cargo.lock)"
RUNNER="${WASM_BINDGEN_TEST_RUNNER:-}"
if [ -z "$RUNNER" ]; then
  if command -v wasm-bindgen-test-runner >/dev/null 2>&1; then
    RUNNER="$(command -v wasm-bindgen-test-runner)"
  else
    for cache in "$HOME/Library/Caches/.wasm-pack" "$HOME/.cache/.wasm-pack"; do
      candidate="$cache/wasm-bindgen-cargo-install-$WASM_BINDGEN_VERSION/wasm-bindgen-test-runner"
      [ -x "$candidate" ] && RUNNER="$candidate" && break
    done
  fi
fi
if [ -z "$RUNNER" ]; then
  echo "wasm-bindgen-test-runner $WASM_BINDGEN_VERSION not found: cargo install --locked wasm-bindgen-cli --version $WASM_BINDGEN_VERSION" >&2
  exit 1
fi
if ! "$RUNNER" --version 2>/dev/null | grep -q "$WASM_BINDGEN_VERSION"; then
  echo "warning: $RUNNER is not wasm-bindgen $WASM_BINDGEN_VERSION (the lockfile's) — the run may fail on a version mismatch" >&2
fi

# Firefox via geckodriver (version-tolerant, unlike chromedriver): on PATH,
# else wasm-pack's cache. Without GECKODRIVER the runner falls through to
# whatever driver it finds first — on macOS that is safaridriver, unconfigured.
if [ -z "${GECKODRIVER:-}" ]; then
  if command -v geckodriver >/dev/null 2>&1; then
    GECKODRIVER="$(command -v geckodriver)"
  else
    for cache in "$HOME/Library/Caches/.wasm-pack" "$HOME/.cache/.wasm-pack"; do
      for candidate in "$cache"/geckodriver-*/geckodriver; do
        [ -x "$candidate" ] && GECKODRIVER="$candidate" && break 2
      done
    done
  fi
fi
if [ -z "${GECKODRIVER:-}" ]; then
  echo "geckodriver not found: install it (brew install geckodriver) or run wasm-pack test --firefox once to cache it" >&2
  exit 1
fi
export GECKODRIVER

# The wasm integration tests, by file name (each configures its own context).
TESTS=(step_111a_local_vfs_storage step_111a_eviction_reason step_91_main_thread_refused step_92_single_active_tab step_94_leader_tab step_115_browser_push_doorbell step_118_browser_sync_pace step_119_takeover_event step_122_browser_surface step_110_browser_error_names indexeddb_keystore wasm_fetch)

export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="$RUNNER"
export WASM_BINDGEN_TEST_ONLY_WEB=1
for t in "${TESTS[@]}"; do
  echo "== wasm test: $t"
  cargo test -p kupferdraht-core --target wasm32-unknown-unknown --test "$t" -- "$@"
done
