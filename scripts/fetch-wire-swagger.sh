#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Schwarz Digits KG
# SPDX-License-Identifier: GPL-3.0-only

# Fetch Wire's v14 OpenAPI document into reference/wire.swagger.json.
#
# wire-server (AGPL-3.0) generates the document and serves it at every
# Wire backend's /v14/api/swagger.json. It is Wire's, not ours, so it is
# not committed: fetch it when a task needs the spec (CLAUDE.md "Wire
# API: v14 Only"). reference/README.md records the SHA-256 of the
# document Kupferdraht was built against; the hash printed here differs
# only when Wire changed the spec.
#
#   KUPFERDRAHT_SWAGGER_URL   — required: the document on your backend,
#                               https://nginz-https.<domain>/v14/api/swagger.json
set -euo pipefail

url="${KUPFERDRAHT_SWAGGER_URL:?set KUPFERDRAHT_SWAGGER_URL to https://nginz-https.<domain>/v14/api/swagger.json}"
out="$(cd "$(dirname "$0")/.." && pwd)/reference/wire.swagger.json"

curl -fsSL --max-time 60 "$url" -o "$out"
printf '%s  %s\n' "$(shasum -a 256 "$out" | cut -c1-64)" "$out"
