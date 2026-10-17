---
task: 01M54HYGQNYDP2MGXXS7AY7164
status: done
completed: 2026-10-17
---
# evidence_mode（none / pinned / latest）の一貫化 — 全体の状態

設計は ADR `2026-10-17-evidence-mode-none-pinned-latest`。WU 別の記録は `2026-10-17-evidence-mode/`（api・campaign・e2e）と `2026-10-17-evidence-mode-wire/dispatch.md`。

## 実装済み / mock 検証 / 実経路検証 / 未検証
| 区分 | 内容 |
|---|---|
| 実装済み | `RoutingOverride.evidence_mode`（旧 JSON は serde で読み替え）、POST /tasks・GET /tasks/{id}（DTO）・schema、retry の override 複製、dispatcher `fill_route_stage`（none は lookup しない）、route_decision / policy_version への記録、campaign の POST 本文・preflight（暗黙 latest 拒否）・record・checker |
| mock 検証 | campaign crate（Fake transport）の `celeris_exec` / `routing_pin` 試験、dispatch 単体の `evidence_mode::none_never_reads_the_table...` |
| 実経路検証（無課金・LLM stub） | `crates/celeris/tests/evidence_mode_e2e.rs`: 実 task-api router（HTTP）で起票 → 子・孫・retry → 実 Dispatcher の route 判断と tick（fake harness）。根拠表を v1→v2 に差し替えても none は同判断・lookup 0 回（`lock_counts` 差分が lookup 無しの基準と一致、latest はそれより多い）。`routing_decided` event と DTO で none / pinned(v1) / latest(v2) が判別できる。矛盾指定は 4xx で task を作らない |
| 未検証 | 実 LLM・実 harness での本測定（開始しない）。本番 daemon での mode 記録の確認。`routing_guardrail` の stage 昇降格は task 単位 mode の対象外（latest 前提） |

## 本番への移行
- DB migration 不要。`evidence_mode` 欄なしの既存行・event は、`evidence_table_id` あり→pinned、なし→latest として読まれ、旧挙動が変わらない。
- 既存の campaign manifest は変更不要（`no_evidence_table` / `evidence_table_id` をそのまま読む）。manifest に `latest` を書く手段は無く、C arm で両方無い場合は preflight が拒否する。`no_evidence_table=true` と `evidence_table_id` の併記は起動前に拒否される（旧設定でこの併記があれば直す）。
- 本番 config・release・daemon には触れていない。反映は通常の release 昇格（人）で行う。
