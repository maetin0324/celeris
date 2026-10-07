---
title: catalog のモデルを legacy の役割（frontier / standard / cheap）へ画面で割り当て、その割り当てで routing を動かす
tasks: [01M49T2H94J93KPF9SF7CW28AG]
status: done
updated: 2026-10-07
completed: 2026-10-07
---

# catalog のモデルを legacy の役割（frontier / standard / cheap）へ画面で割り当て、その割り当てで routing を動かす

設計は [ADR 2026-10-06 model-role-assignments](../adr/2026-10-06-model-role-assignments.md)（付記に実装突き合わせ）。前提は [ADR 2026-10-06 opencode go と model catalog](../adr/2026-10-06-opencode-go-and-model-catalog.md)。

## phase 1（2026-10-07、commit dae4c31a → 86d8b8cc → 本 commit）

- **割り当ての表**: migration `0053_model_role_assignments.sql`（`source × tier → model_id, note, updated_at, updated_by`）、`task_core::model_catalog::assignments`（`RoleAssignment` / `AssignmentView::build` / `apply_to_bindings` / `RoleAssignmentReader` / `StaticAssignments`）、`ModelCatalogStore::model_role_assignment_{set,delete,view}`、`Event::ModelRoleAssignmentChanged`（actor 付き、catalog の疑似 task に追記）。catalog の自動更新（`model_catalog_apply`）は表を触らない（試験あり）。
- **routing への反映**: dispatcher（`set_role_assignment_reader`、`effective_tier_models`、provider 選択の除外 `assignment_excluded`、run / reviewer の adapter を `with_tier_models` で包む）、llm-proxy（`normalize_legacy_config_with`、`ProxyState::with_role_assignments`）、routing catalog（`apply_role_assignments` + `ProviderLaneSeed`、割り当て → catalog の順）。daemon は `Arc<SqliteStore>` を両方に渡す。config の再読み込みは不要。
- **opencode go**: `POST/PATCH /api/v1/providers` の `account_pool` が `AccountPoolSetting`（`"opencode-go"` 文字列）を受ける。web の opencode-go カードの「opencode go を使う（provider を追加）」が acp 行を作る。model なしの acp 行は割り当てのある lane だけ候補になり、pool の全 account が Exhausted なら次の provider へ落ちる（試験あり）。
- **API**: `GET /api/v1/llm/models/assignments`、`PUT/DELETE …/assignments/{source}/{tier}`、`POST …/assignments/preview`（影響の確認）、`GET /llm/models` の `assigned_tiers`。`celerisctl models assign / unassign`。schema と web / gui の生成型を再生成。docs: `docs/api/v1/gui-api.md`、`docs/ops/opencode-go-and-model-catalog.md`、`docs/architecture-map.md`。
- **web** `/models`: source ごとの役割カード（現在のモデル・由来 badge・availability・最終確認・枠）、catalog からの選択 → preview の影響表示 → 割り当て、解除、opencode go の provider 追加。override editor から `tier` を外した。変更は `web/features/ops/`・`web/api/`・e2e fixture に留めた（shell / nav / screens.ts は不変）。

## 証拠（2026-10-07、最終 tree）

| 条件 | コマンド | 結果 |
| --- | --- | --- |
| 0, 1, 2 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest 4219 passed / 0 failed / 14 ignored（132 binaries）、doc-test exit 0 |
| 3 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 3 | `cd web && pnpm typecheck && pnpm lint && pnpm test` | すべて exit 0（vitest 71 files / 479 tests、node:test 59 pass） |
| 3 | `cd web && pnpm e2e` | exit 0、272 passed |
| 3 | `cd web && pnpm e2e:nfr` | exit 0、106 passed（parity-x axe 全画面を含む） |

主な新規試験:
- task-core `model_catalog::assignments::tests`（6）、store の set / delete / event / apply が表を消さない（2）。
- task-dispatch `dispatcher::tests::role_assignments`（7: 割り当て > config、未割り当て lane は config、Excluded lane の除外と fallback、全除外で unroutable、reader なし、opencode-go 行の候補入りと Exhausted の fallback、model なし acp 行は割り当てだけで routing）。
- llm-proxy `legacy_catalog::tests`（4）。celeris `config::model_catalog::tests`（5）と結合 `tests/model_role_assignments_consistency.rs`（dispatcher・llm-proxy・routing catalog が同じ割り当てを使う。消えた config モデルの lane を割り当てで生き返らせる、opencode-go acp 行の `provider:opencode-go/standard`）。
- task-api `tests/model_assignments.rs`（6）、`tests/accounts_admin.rs`（acp + `account_pool: "opencode-go"`、2）。web `model-assignments.test.tsx`（13）、e2e `e2e/admin/models.spec.ts`。

初回の gate で celeris e2e の account pool 試験 3 本が 60 秒 timeout で落ちた。原因は「model も tier_models も無い行は割り当てだけで routing」を全 adapter に掛け、model 無しの claude-code 行が unroutable になったこと。acp 行だけに絞って再実行し全件通過（ADR 付記）。

## 未解決事項

- 本番での割り当て・opencode-go provider の追加・shadow 開始は配送後に人が `/models` 画面で行う（手順は `docs/ops/opencode-go-and-model-catalog.md`）。本 run は本番 config / daemon / DB に触れていない。
- run 中の opencode go 429 の窓別記録は ADR 2026-10-06 D2 の TODO のまま。
- migration 0053 は並行 branch の `0053_cos_triage.sql` と番号が交差する。後から main に入る側が空き番号へ振り直す（RESERVED_VERSIONS も見直す）。
- 割り当て中の proxy lane は解除 preview の `after` が `null`（config 値を判別できない）。

## 提案

- `ProviderCandidateOutcome` に `assignment_excluded` の variant を足して除外理由を構造化する（今は `Unsupported` + detail 文字列）。
- `ProviderLive` に `llm_proxy.models` 由来の lane 値も載せて、API の config 由来の値を 1 か所から取る。
