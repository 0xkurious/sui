# Sui Documentation Audit Report

**Date:** 2026-05-24  
**Auditor:** Capy (automated audit)  
**Scope:** `~/code/sui/docs/content/` (509 files) against codebase at `818f8d78f222b9279cb966a5754fb0a2c21d58b1` (`818f8d78f2`) on current `main`  
**Method:** Cross-referenced docs against current source, config schema, CLI clap surfaces, docs console snippets, and recent docs/code changes since the previous audit on `capy/docs-audit-report`.

---

## Changelog Since Previous Audit (2026-05-17)

Previous audit: **10 issues**.  
Current audit: **9 issues**.

### Fixed since last audit

1. **Full node docs no longer imply that `enable-index-processing: false` disables JSON-RPC itself.** The wording now correctly talks about reclaiming storage used by legacy JSON-RPC indexing.
2. **The stale duplicate exchange-integration guide has been removed from the docs tree.** The maintained page is now `docs/content/operators/exchange-integration.mdx`, and `docs/content/references/exchange-integration-guide.mdx` is gone.

### Partially improved but still open

1. **Custom indexer startup guidance is better than last week** — it now recommends gRPC streaming as the primary path — but the canonical command still uses the 30-day HTTPS checkpoint endpoint as the fallback/backfill source.

### New / newly surfaced issues in this audit

1. **Code now ships v2alpha ledger-history list APIs and new `rpc` knobs, but the public gRPC and operator docs still do not mention them.**

---

## Summary

Found **9 issues** across operator docs, CLI docs, indexing guidance, gRPC API docs, and content-quality checks.

- **High:** 0
- **Medium:** 6
- **Low:** 3

No high-severity operator/security wording bugs surfaced in this pass. The remaining drift is concentrated in config/API discoverability and CLI/reference completeness.

---

## MEDIUM Severity

### 1. Full node docs still point to the YAML template for the “complete list” of `rpc` options, but the template does not contain that list
- **Category:** Config / operator docs
- **Files:**
  - `docs/content/operators/full-node/sui-full-node.mdx` (line 106)
  - `crates/sui-config/data/fullnode-template.yaml` (lines 1-32)
  - `crates/sui-config/src/rpc_config.rs` (lines 9-89)
- **What’s wrong:** The docs say “Refer to the full node YAML template for the complete list of available `rpc` options.” The template still has no `rpc:` block. It only shows legacy top-level fields like `json-rpc-address`. Meanwhile the actual `RpcConfig` exposes more knobs, including `max_json_move_value_response_size`, `index_initialization`, `authenticated_events_indexing`, `ledger_history_indexing`, `ledger_history`, and `display`.
- **What it should say:** Either add a real `rpc:` example block to the template or point readers to a generated config/schema reference instead of the template.
- **Impact:** Operators looking for supported RPC knobs will miss real runtime controls and assume the docs/template are exhaustive when they are not.

### 2. `client.mdx` still omits examples for several commands that ship in `sui client --help`
- **Category:** CLI commands & flags
- **Files:**
  - `docs/content/references/cli/client.mdx`
  - `docs/content/snippets/console-output/sui-client-help.mdx` (lines 17, 33, 39, 48, 50, 52, 54)
  - `crates/sui/src/client_commands.rs` (for example `ExecuteCombinedSignedTx` at line 222, `PartyTransfer` at line 317, `SendFunds` at line 384, `TestPublish` at line 458, `TestUpgrade` at line 470, `SerializedTx` at line 473, `SerializedTxKind` at line 482)
- **What’s wrong:** The help snippet advertises `execute-combined-signed-tx`, `party-transfer`, `send-funds`, `test-publish`, `test-upgrade`, `serialized-tx`, and `serialized-tx-kind`, but `client.mdx` still has no matching examples for any of them.
- **What it should say:** Add at least a one-command example for each shipped command, with link-outs where a deeper guide already exists.
- **Impact:** The main client CLI reference is still incomplete, so users hit real commands in `--help` that have no reference-page examples.

### 3. zkLogin demo still defaults to Devnet even though the main zkLogin guide was corrected to Testnet
- **Category:** Code examples
- **Files:**
  - `docs/content/sui-stack/zklogin-integration/zklogin-demo.mdx` (lines 170-172)
  - `docs/content/sui-stack/zklogin-integration/index.mdx` (lines 44-45)
- **What’s wrong:** The demo `.env` still ships with:
  ```bash
  VITE_NETWORK=devnet
  VITE_SUI_GRPC_URL=https://fullnode.devnet.sui.io:443
  ```
  while the main integration guide now correctly defaults to Testnet.
- **What it should say:** Default the demo to Testnet, with Devnet called out only as an opt-in choice for cutting-edge testing.
- **Impact:** Builders following the demo are still likely to start on the most reset-prone public network.

### 4. Custom indexer docs still make the 30-day HTTPS checkpoint endpoint part of the canonical “recommended” startup command
- **Category:** Data access / indexing docs
- **File:** `docs/content/develop/accessing-data/custom-indexer/build.mdx` (lines 390-410)
- **What’s wrong:** The page now correctly says to prefer full node gRPC as the primary source, which is an improvement. But the “Recommended” example still hardcodes `--remote-store-url https://checkpoints.testnet.sui.io`, and the same section immediately says that public HTTPS checkpoint endpoints retain only the most recent 30 days while full-retention backfills should use `--remote-store-gcs`.
- **What it should say:** Keep the gRPC-first guidance, but make the durable GCS-backed command the production/default example and label the HTTPS endpoint as recent-data-only convenience/fallback.
- **Impact:** Builders can still copy the canonical command and only discover later that their fallback/backfill source is retention-limited.

### 5. DeepBook Predict docs still carry a stale freshness marker and remain pinned to a dated Testnet branch
- **Category:** Documentation freshness / coordination-required content
- **Files:**
  - `docs/content/onchain-finance/deepbook-predict/deepbook-predict.mdx` (lines 16-17, 22, 26)
  - `docs/content/onchain-finance/deepbook-predict/contract-information.mdx` (lines 17, 30-36, 120-124)
- **What’s wrong:** The docs are explicit that they are pinned to `predict-testnet-4-16`, which is good, but the frontmatter still says `last_verified: 2025-04-16`. More than a year later, the freshness marker undermines trust rather than increasing it.
- **What it should say:** Re-verify the package IDs / public server / source branch with the DeepBook Predict owners and refresh the verification date, or move the content under a more obviously temporary preview label.
- **Impact:** Readers get a stale “verified” signal on content that already depends on dated Testnet-specific branch pins.

### 6. New v2alpha ledger-history list APIs and their required full-node knobs are effectively undocumented
- **Category:** API / operator docs drift
- **Files:**
  - `crates/sui-rpc-api/src/grpc/v2alpha/ledger_service/mod.rs` (lines 27-62)
  - `crates/sui-config/src/rpc_config.rs` (lines 68-89, 123-139, 364-423)
  - `docs/content/develop/accessing-data/grpc/using-grpc.mdx` (lines 113-145)
  - `docs/content/develop/accessing-data/grpc/what-is-grpc.mdx` (lines 44-49)
- **What’s wrong:** Code now ships streamed `list_checkpoints`, `list_transactions`, and `list_events` handlers under the v2alpha ledger service, plus `ledger_history_indexing` / `ledger_history` config knobs. The public gRPC docs still only describe `sui.rpc.v2.LedgerService` examples and do not mention the new list APIs or the node configuration needed to enable their historical indexes.
- **What it should say:** Add a dedicated docs section (or a clear expansion of the existing gRPC guide) covering the v2alpha list APIs, when to use them, retention expectations, and the full-node `rpc` settings required to support them.
- **Impact:** Operators and client developers cannot discover or correctly enable a newly shipped API surface from the docs alone.

---

## LOW Severity

### 7. `completion` still has no dedicated docs page
- **Category:** CLI commands & flags
- **Files:**
  - `docs/content/snippets/console-output/sui-help.mdx` (line 23)
  - `crates/sui/src/sui_commands.rs` (lines 432-438)
  - `docs/content/references/release-notes.mdx` (lines 361-370)
  - `docs/content/references/cli/` (no dedicated `completion` page)
- **What’s wrong:** `sui completion` is a real top-level command and has release-note examples, but there is still no corresponding page under `docs/content/references/cli/`.
- **What it should say:** Add a short CLI reference page with supported shells and output-file examples.
- **Impact:** Users still only discover the feature through `--help` or old release notes.

### 8. `fire-drill` still has no docs page or “internal-only” note
- **Category:** CLI commands & flags
- **Files:**
  - `docs/content/snippets/console-output/sui-help.mdx` (line 20)
  - `crates/sui/src/sui_commands.rs` (lines 392-396)
  - `docs/content/references/cli/` (no dedicated `fire-drill` page)
- **What’s wrong:** `sui --help` still exposes `fire-drill`, but the public docs tree has no page for it and no note explaining whether it is internal/operator-only.
- **What it should say:** Either document it briefly or explicitly hide/annotate it as an internal tool.
- **Impact:** Low, but it still makes the public CLI surface look partially undocumented.

### 9. `MystenLabs` GitHub URL casing is still inconsistent in multiple docs
- **Category:** Content quality / canonical URLs
- **Files:**
  - `docs/content/onchain-finance/examples-patterns/loyalty-tokens.mdx:33`
  - `docs/content/snippets/quick-install.mdx:12`
  - `docs/content/getting-started/onboarding/sui-install.mdx:58`
  - `docs/content/sui-stack/walrus/sui-stack-walrus-sites.mdx:21,114`
- **What’s wrong:** These pages still use `mystenLabs` or `Mystenlabs` instead of canonical `MystenLabs` in GitHub/raw GitHub URLs.
- **What it should say:** Normalize all GitHub and raw GitHub URLs to canonical `MystenLabs` casing.
- **Impact:** Low. The URLs currently resolve, but the inconsistency is sloppy.

---

## Notes

### What improved since last audit
- The full-node JSON-RPC indexing wording is no longer misleading.
- The stale duplicate exchange-integration guide has been removed from the docs tree.
- The exchange integration page itself was rewritten around gRPC and GraphQL.
- The custom indexer guide now explicitly prefers gRPC streaming as the primary source, even though its fallback example still needs tightening.

### Systemic patterns worth addressing
1. **Operator/config docs still drift whenever the source schema grows.** The `rpc` docs are lagging both the template and newly added ledger-history knobs.
2. **CLI reference coverage still lags behind shipped command surfaces.** `client.mdx`, `completion`, and `fire-drill` are the obvious examples.
3. **Docs freshness markers need enforcement, not decoration.** `last_verified` only helps if stale values are revisited promptly.
4. **New API surfaces need docs in the same release window as the code.** The new v2alpha ledger-history list APIs are already in code, but effectively invisible to readers.

### Recommended follow-up order
1. Document the full `rpc` surface, including ledger-history settings and the new v2alpha list APIs.
2. Fill the missing `client.mdx` examples.
3. Switch the zkLogin demo to Testnet.
4. Tighten the custom indexer recommended command so durable backfill guidance is unambiguous.
5. Re-verify DeepBook Predict content with the owning team.
6. Add `completion` docs and decide whether `fire-drill` should be publicly documented or hidden.
