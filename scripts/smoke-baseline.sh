#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Schwarz Digits KG
# SPDX-License-Identifier: GPL-3.0-only

#
# Live regression baseline against the backend the CLI is logged into.
#
# Run this BEFORE testing a new Step's CLI subcommand. It verifies that
# every Step that has shipped still works end-to-end after code changes
# — login session, metadata sync, event drain, room listing,
# bootstrap-into-rooms, and a Step 4 send_text round-trip. Any failure
# means the new Step's live test is invalid until the baseline is
# restored.
#
# Prerequisites:
#   - `target/release/kupferdraht-cli` is built.
#   - `kupferdraht-cli login` was run interactively (TTY) less than ~14 min ago.
#
# Stage 5 (`rooms-join-all`) bootstraps the local MLS state for every
# room the user is a Wire-side member of. It's idempotent — already-
# joined rooms are skipped — so warm runs and fresh-`rm -rf` runs both
# converge on the same state. This is what makes the baseline self-
# recovering: pre-Step-9 the script crashed at Stage 5 (send) on every
# fresh-device run because the local openmls store was empty for the
# target room. Inserting `rooms-join-all` makes the same script work
# both warm and cold, which matches the smoke-day cadence after
# Steps 7c, 8b, 9d each forced an `rm -rf` to validate clean-state
# behaviour.
#
# Defaults can be overridden via env vars:
#   KUPFERDRAHT_CLI            — path to the binary (default: ./target/release/kupferdraht-cli)
#   KUPFERDRAHT_BASELINE_ROOM  — room id to send into; must already be at epoch ≥ 1
#                           on the Wire side (required, no default)
#   KUPFERDRAHT_BASELINE_TEXT  — message text (default: "smoke baseline <ISO ts>")
#
# Exits non-zero on the first failure with the offending stage labelled.

set -euo pipefail

CLI="${KUPFERDRAHT_CLI:-./target/release/kupferdraht-cli}"
ROOM="${KUPFERDRAHT_BASELINE_ROOM:?set KUPFERDRAHT_BASELINE_ROOM to <uuid>@<domain>, a room at epoch >= 1 on the backend}"
TEXT="${KUPFERDRAHT_BASELINE_TEXT:-smoke baseline $(date -u +%Y-%m-%dT%H:%M:%SZ)}"

if [[ ! -x "$CLI" ]]; then
    echo "smoke-baseline: $CLI not found or not executable" >&2
    echo "smoke-baseline: build first via \`cargo build --release -p kupferdraht-cli\`" >&2
    exit 2
fi

step() {
    printf '\n=== %s ===\n' "$1"
}

step "1/5 status"
"$CLI" status

step "2/5 sync (metadata refresh + event drain — Step 14b)"
"$CLI" sync

step "3/5 rooms"
"$CLI" rooms | head -40

step "4/5 rooms-join-all (bootstrap local MLS state — idempotent)"
"$CLI" rooms-join-all

step "5/5 send into $ROOM"
"$CLI" send --id "$ROOM" -m "$TEXT"

printf '\n=== baseline OK ===\n'
