---
title: 候補別の構造化除外理由で枠のみの待ちを判定する（launch-and-wait・reason unit）
tasks: [01M6SQJF9YD3WPTD8NSB191PZW]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# launch-and-wait（reason unit）

完了日: 2026-11-07（この unit の範囲。notify・launch-wire・slot-wait・cross は別 unit）

正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md) D2・D4。新しい設計判断はなく、ADR は足していない。

## 変更（commit は件名で列挙）
- `phase 5: 候補別の構造化除外理由で枠のみの待ちを判定する`

- `task-core/llm_route.rs`: `ExclusionReason` に `AuthFailed`・`CosReservation`・`CosRunLimit` を足し、`is_slot()`（網羅 match。枠は `SourceConcurrency`・`AccountConcurrency`・`CosRunLimit`）を追加。`QuotaSnapshot.auth_failed` を追加。`admit_source` は、1 つの候補の account に枠以外の理由があればそれを報告する（account の並びで結果が変わらない）。
- `task-dispatch/dispatcher/route_select.rs`: `slot_only_wait(&RouteFailure, &RouteTrace) -> bool`（失敗が `NoEligible`・理由 1 件以上・全部が枠理由のとき真。文字列は見ない）。auth 失敗（`ExcludedReason::Rejected`）は quota 枯渇ではなく `auth_failed` に入れる。暗黙 account（プールを持たない source）に CoS の +1 を付けない（実 account は +1 のまま）。失敗時の `RouteTrace.candidates` に route 順を残す。
- `eval_trials.rs`: 新しい 2 理由を quota 側の保留に分類（exhaustive match のため）。
- 生成物: `docs/api/v1/{event,api-v1}.schema.json`、`web/api/generated/{schema.json,types.ts}` を再生成。
- 旧い文字列判定: この範囲の route 選択には無く、消すものは無し。`cos_chat/routes.rs` の `supply_failure` は起動後の失敗の分類で、dispatcher の待機配線と同じく別 unit の範囲。
- 未接続: `CosReservation`・`CosRunLimit` は列挙と判定の入力としては閉じたが、route_select 自身はまだ出さない（Claude 予約は現状 route_state に未実装、`max_cos_runs` は capacity 側）。slot-wait unit が接続する。

## 修正前の回帰
試験だけを先に適用（`slot_only_wait` は常に偽の stub、`+1` は暗黙 account にも付く状態）。
- コマンド: `cargo test -p task-dispatch --lib slot_wait`
- exit 101、8 件中 4 件失敗: `every_candidate_excluded_by_slots_only_waits`（`assertion failed: slot_only_wait(..)`）、`closed_enumeration_of_reasons`（`SourceConcurrency left: false right: true`）、`implicit_account_gets_no_extra_cos_slot`（left 3、right 2）、`same_input_gives_the_same_order_and_result`（失敗時の trace.candidates が空）。

## 修正後
- 同じコマンド: 8 passed, 0 failed。
- 固定するもの: 全候補が枠のみで真（受け入れ 0）、cooldown・quota・auth・未ログイン・未宣言・候補ゼロ・理由なしで偽、同一候補の枠 account と cooldown account の混在は account の並びを変えても偽（1）、暗黙 account は +1 無し・実 account は +1（2）、同じ入力を繰り返しても route 順と結果が同じ・枠が空けば同じ先頭候補が通る（3）、全理由の枠判定の表。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh`: exit 0。7794 tests run, 7794 passed, 22 skipped（nextest）、web unit 104 files・711 tests passed、`progress_index_commit_hashes` ok。途中で既存試験 `route_select_admission_excludes_full_and_exhausted_accounts` が落ちた（ca 枠・cb quota 枯渇の候補で、枠以外の理由を優先して報告する新しい規則に合わせ、期待を `AccountQuota` に更新）。`dispatcher.rs` の行数 guardrail（production_size）は再 export を既存行に収めて通した。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決
- `CosReservation`・`CosRunLimit` を route_select が出す配線、`slot_only_wait` を使う Waiting への接続、期限（`slot_wait_expired`）は slot-wait unit。

## 提案
- slot-wait unit は `slot_only_wait(&failure, &trace)` を routes.rs の失敗分岐で呼び、`supply_failure` の文字列判定とは分ける。`max_cos_runs` に達した CoS run は route_select の前に `CosRunLimit` 相当で待つ。
