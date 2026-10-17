---
title: "ADR: routing_override の根拠表の扱いを evidence_mode（none/pinned/latest）で明示する"
status: accepted
date: 2026-10-17
tasks: [01M54HYGQNYDP2MGXXS7AY7164]
---
# routing_override の evidence_mode（none / pinned / latest）

- 関連: 2026-10-16-mlsys-measure-task-routing（RoutingOverride・D1/D2）

## 文脈
campaign manifest の `no_evidence_table=true` は「根拠表を使わない」意味だが、`RoutingOverride` には
`evidence_table_id` が無い（None）ことしか表せず、None は dispatcher では「最新版を読む」だった。
そのため「使わない」が黙って「最新版を使う」に変わり得た。

## 決定
- D1 `RoutingOverride.evidence_mode: EvidenceMode`（`none` / `pinned` / `latest`、JSON は snake_case）を追加する。
  - `none`: 実測根拠表を参照しない（lookup 0 回）。固定した候補順・割当規則と既存の利用可否・安全上限だけを使う。
  - `pinned`: `evidence_table_id` で指定した版だけを読む。
  - `latest`: 通常運用の最新版。本測定で暗黙に選ばれてはならない（campaign の preflight が拒否・記録する。別 unit）。
- D2 互換（migration 不要、serde で読む）: `evidence_mode` 欄なしの旧 JSON（`task_routing_overrides.override_json`・
  追記専用 event `routing_override_pinned`）は、`evidence_table_id` あり→`pinned`、なし→`latest`（旧挙動）と読む。
  行・events は書き換えず、読む側が正規化する。書くときは常に `evidence_mode` を出力する。
- D3 矛盾は `RoutingOverride::validate()`（公開。API・campaign が起動前に呼べる）で拒否する:
  `none` + `evidence_table_id`、`pinned` で id なし/空白、`latest` + `evidence_table_id`。
- D4 継承: 子・孫・retry は最も近い祖先の行を丸ごと継ぐ（`task_routing_override`）ため mode も保たれる。
  `ResolvedRouting` にも `evidence_mode` を載せ、`none` を `latest` に落とさない。
  API・dispatcher・campaign への接続は同じ task の後続 unit が行う。
