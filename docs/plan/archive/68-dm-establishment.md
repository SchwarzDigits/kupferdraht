# Step 68 — create_dm: establish the MLS 1:1

> **Status:** merged to `main` as `c102b35` (2026-08-14) — archived.
> **Live smoke 2026-08-14** against the own instance: all three
> branches proven (epoch-0 establish → epoch 1 + immediate `list-ids`
> visibility; epoch>0 external join; idempotent resume), full message
> round-trip both directions after the peer's join. Side finding:
> the unabsorbed Welcome costs the peer the first message
> (observations §67, gap-B.24).
> From the Step-62 smoke finding (observations §66): current
> wire-server hides unestablished MLS 1:1 conversations from
> `list-ids`, and Kupferdraht's `create_dm` only creates the **local**
> group — no peer claim, no commit upload — so the conversation never
> materializes ("not in cache after sync"). Same family as the
> permanently failing `mls-missing-group-info` room in every
> `rooms-join-all`.
>
> **Historical names:**
> - "Step 68" — proposed 2026-08-14.
>
> **Branch / commit prefix:** `step/68-*`, `feat(step-68): …`.

---

## Goal

`create_dm` establishes the 1:1 group instead of only mirroring it
locally. Flow after the one2one fetch (which returns the
deterministic conversation + `group_id`):

1. **Local group already present** → resume unchanged (sync + return
   the cached room).
2. **`conv.epoch == 0`** (nobody established yet) → create the local
   group under the conversation's `group_id`, then run the invite
   machinery against the peer: `claim_key_packages_for_users([peer])`
   (also claims own other devices, empty-pool error included) →
   `add_members_commit` → `upload_and_merge_commit_bundle` (staged:
   upload before merge). The server then has the group info, the peer
   gets its Welcome, and `list-ids` lists the conversation. Any
   failure after the local create **discards the group** (Step-66
   Option-A pattern) so a retry starts clean.
3. **`conv.epoch > 0`** (peer established first / new own device) →
   join via external commit (the `join_room` mechanics keyed to the
   conversation).

Then `sync_metadata` → the room is in the cache → return it. The
"(Wire metadata race?)" error text dies with this step.

## Side effect worth smoking

Establishing claims one of the peer's key packages — the first real
claim source, which makes Step 63's low-pool auto-top-up provable
live (drain user 2's pool via DMs, watch the refill).

## TDD list (red)

1. `create_dm_establishes_and_is_idempotent` — epoch-0 one2one:
   exactly one claim + one commit-bundle upload; a second `create_dm`
   reuses the local group (hits stay 1/1).
2. `create_dm_joins_when_the_peer_already_established` — epoch-2
   one2one: external-commit join (groupinfo fetched), **no** claim.
3. `create_dm_discards_on_empty_peer_pool_so_retry_reclaims` — claim
   returns an empty pool: clear error, local group discarded, a
   retry claims again instead of short-circuiting on local state.

## Out of scope

- The self-conversation (own deterministic room, `open.md` §63) —
  related family, its own step.
- Backfilling/healing pre-existing unestablished conversations
  (e.g. the webapp-created `mls-missing-group-info` room) — a
  follow-up once this step proves the establish flow live.
