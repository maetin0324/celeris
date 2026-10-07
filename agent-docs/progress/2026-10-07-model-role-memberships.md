---
title: モデルごとの複数役割と優先度（frontier / standard / cheap を個別に設定）
tasks: [01M49Z9NNAAKRJB4GJHFHGXX21]
status: done
updated: 2026-10-07
completed: 2026-10-07
---

# モデルごとの複数役割と優先度（frontier / standard / cheap を個別に設定）

設計: [ADR 2026-10-06 model-role-assignments の付記「モデルごとの複数役割と優先度」と「shadow / enforce の配線」](../adr/2026-10-06-model-role-assignments.md)。本番の `~/.config/celeris`・daemon・DB は操作していない（本番の割り当ては配送後に人が行う）。

## 実装した範囲

### phase 1（91a66bea）

- migration 0054: `(source, tier, model_id)` と非負の priority、明示的な空集合を保持する `model_role_scopes`。旧割り当て（model・メモ・更新者・日時）を priority 0 で移行。
- 役割集合の GET / PUT / preview API（`/api/v1/llm/models/assignments/roles/{tier}`）、config の互換読み取り（未編集 scope のみ）、同じ source 内の複数候補、変更イベント。旧 API は互換用に残す。
- dispatcher / llm-proxy / routing catalog の候補列を複数モデルへ拡張。legacy は priority 順に使えるモデルを選び、消失・disabled を除外、account の枯渇時は次の source / account へ fallback。
- web `/models`: 全 source のモデル一覧（検索・source / 状態の絞り込み）の各行に 3 役割の独立したトグルと priority、役割別の並べ替え、保存前の影響確認。既存の provider 追加操作を保持。

### attempt 2（この記録。shadow / enforce の配線）

- dispatcher: 候補の identity をモデルごと（`<provider>/model:<id>`）に分け、容量・account の identity は provider（`LegacyProfile.provider_id`）で共有。
- `dispatcher/routing_members.rs`: 選んだ provider の役割メンバーを task-core の `optimize` + `HeuristicEstimator`（Shadow mode の lane policy）で順位づけ。品質は daemon が差し込む routing catalog の model profile（`RoutingModelProfiles`、`bootstrap.rs` が共有 snapshot を包む）。推定不能なら priority 順。
  - enforce: kernel の先頭を run の lane 束縛に差し替えて実行（`rebind_lane_model`）。trace に全メンバー・score・`fallback_order`・`estimator_version`（`heuristic-1` / `heuristic`）。
  - shadow: primary は legacy のまま、候補 policy の選択を `DecisionShadowComparison`（`provider_id` を追加）と `candidate_model` に記録。
- llm-proxy: `EstimatorShadowInput.catalog` に要求時点の割り当てを写した catalog を渡し、起動時の catalog に `LegacyCatalog::extended_with` で重ねる（同じ family から context 上限等を継ぐ）。sidecar に役割の全メンバーを送り、kernel の選択を記録。同じ要求で 401 / 429 を受けた account は残りのモデル候補からも外す。

## 検証記録（最終 tree、attempt 2）

| 検査 | 結果 |
| --- | --- |
| `cargo test -p task-dispatch --lib routing` | 39 passed（新規: `enforce_executes_the_kernel_choice_among_role_members_and_falls_back_by_priority`、`shadow_records_the_kernel_choice_among_role_members`） |
| `cargo test -p llm-proxy --test estimator_shadow` | 3 passed（新規: `routing_estimator_shadow_scores_every_role_member_from_the_request_time_catalog`: sidecar が `legacy:qwen:model-a` / `model-b` の両方を受け取り、`candidate_model = legacy:qwen:model-b`・`differs_from_primary`） |
| `cargo test -p llm-proxy --test proxy_fallback` | 2 passed（新規: `routing_proxy_role_members_skip_an_account_rejected_in_the_same_request`: acct-a の 429 の後は acct-a の m2 を送らず acct-b の m1 で成功。全 account 429 なら m2 を一度も送らない） |
| `cargo test -p llm-proxy --lib legacy_catalog` | 5 passed（新規: `extended_with_adds_missing_members_and_inherits_family_limits`） |
| `cargo fmt --all` | 差分なし（commit 前に実行） |
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest 4252 passed / 0 failed / 14 ignored、doctest exit 0（`artifacts/rust-workspace-attempt2.log`） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web test` | exit 0（Vitest 71 files / 479 tests、gateway tests pass） |
| `pnpm -C web e2e` | exit 0（272 passed / 8 skipped。`e2e/admin/models.spec.ts` を含む） |
| `pnpm -C web e2e:nfr` | exit 0（106 passed） |
| `sh scripts/dev/progress-index.sh --check` / `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | ok |

phase 1 の記録（参考）: `cargo test -p task-api --test model_assignments` 7 passed、`pnpm -C web e2e -- admin/models.spec.ts` 8 passed、schema は docs / web / gui を再生成（GUI の `pnpm gen:types` は pnpm store の読み取り専用制約で失敗したため、同版 json2ts の CLI を直接呼んで再生成）。

## 未解決事項

- 品質の順位づけは routing catalog の model profile（`[model_routing.models]` の `quality`）に依存する。本番 config に品質が無いモデルは priority 順になる（決定的。推定器の shadow は llm-proxy 側で sidecar を使う）。
- 本番の割り当て・shadow 開始は人が配送後に `/models` 画面で行う。実 sidecar での確認は本番でのみ可能。

## 提案

- `DecisionShadowComparison` の `provider_id` は版 1 への追加欄（既定は空）。版を上げる必要が出たら ADR 2026-10-04 側で扱う。
