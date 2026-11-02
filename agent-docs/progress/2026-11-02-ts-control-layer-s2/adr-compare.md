---
title: S2 adr-compare — 境界 ADR に compare mode の仕様の付記
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 adr-compare — 境界 ADR に compare mode の仕様の付記

完了日: 2026-11-02（task 01M6E6HEH7CPZX9KX4Y2SAZKTG、unit adr-compare。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- [境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) に「付記 2026-11-02: S2 の compare mode」（節 S2-1〜S2-8）を足した。コードは変えていない。
- 読んだ根拠: S0 の計画（`2026-11-02-ts-control-layer-s0.md` §2 S2・§3）、S1 の未解決（`2026-11-02-ts-control-layer-s1.md` 未解決 4 と host-api.md）、
  `crates/task-core/src/model_policy.rs`（`RULES`・`decide`・`decide_for_task`）、`crates/task-core/src/llm_route.rs`（`Registry`・`route_priority`・`route_defaults_for`）、
  `crates/task-dispatch/src/dispatcher/dispatch_run.rs`（`decide_lane`）、`crates/task-dispatch/src/dispatcher/route_select.rs`（`route_priority` の 3 か所の呼び出し）、
  `crates/task-core/migrations/0002_events_global_id.sql`（`events.task_id` が NOT NULL）。

## 付記で決めたこと（項目と節の対応）
| objective の項目 | 節 | 要点 |
|---|---|---|
| (1) 対象と順序 | S2-1 | `lane_rules` → `route_priority`。`direct_route` は Rust 維持。compare は各 1 か所（`decide_lane`、`route_select_one` の最初の呼び出し・`cos == false`） |
| (2) DTO と比較欄 | S2-2 | `lane_rules` は `rule_id`・`proposed`・`condition` を byte 一致。`route_priority` は `basis` と `candidates`（順序込み）を byte 一致。正規化は両領域「なし」 |
| (3) 機密除去 | S2-3 | allowlist。DTO の欄だけを保存。credential・account・token・本文・`SourceSpec`・quota は入れない。64 KiB 超は digest だけ |
| (4) event と表 | S2-4 | `script_compare_recorded` の 17 欄と outcome 5 種の定義、`script_generations`・`script_domain_modes`（`version` の CAS）、`script_domain_mode_changed` は追記専用の `script_domain_events` に同じ transaction で書く |
| (5) mode と権限 | S2-5 | `rust`・`compare` だけ。`canary`・`ts` は 422 `script_mode_not_enabled`。`rust`→`compare` は人の token か CoS operations、戻しは誰でも・daemon の起動時復元 |
| (6) compare の約束 | S2-6 | Rust の結果を変えない（既存 event と byte 一致を試験で固定）、TS の失敗でも Rust の結果、deadline 50 ms、LLM・外部 tool なし、記録の失敗で dispatch を止めない |
| (7) 要求ごとの V8 context | S2-7 | S2 では分けない。両領域の許可表を空にし、capability を宣言した manifest の領域は load で拒否。global の残り値は fixture の正順・逆順の 2 回流しで検出。分離は S4 の前に決める |
| (8) 安全違反の数え方 | S2-8 | code（`denied_capability`・`denied_scope`・`ceiling_exceeded`・`undeclared_candidate`）、空でない event の件数を数え、分母は `outcome != unavailable` の event 数 |
| 旧 routing_shadow | S2-4 末尾 | `routing_shadow` の表・event・module・旧 config 欄を使わない。S2 は config 欄を足さない |

## 変更
- `phase 1: adr-compare: 境界 ADR に S2 の compare mode の付記、進捗を書く` — ADR の付記とこの file。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
結果は run の報告に書く（この file を書いた後に実行するため）。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ control-ts/ web/` → exit 0 を期待（受け入れ 1）
- `grep -c '^### S2-[1-8] ' agent-docs/adr/2026-11-02-ts-control-layer-boundary.md` → 8 を期待（受け入れ 0）
- `bash scripts/dev/check-doc-links.sh`・`bash scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`

## 未解決
1. `route_priority` の出力の `basis` は今の Rust に無い。route-prio unit が `route_priority` を「列と basis を返す関数」と包みに分ける（挙動不変）前提で書いた。
2. `script_compare_recorded` の重複の抑え（同じ task・領域・generation・`input_digest`・outcome）の読み出しの費用は未測定。dispatch-hook unit で events の読み出し方を決める。
3. `script_domain_events` を API・web から読む経路は決めていない（mode-api unit で `GET` に足すかを決める）。
4. 現行コードに `routing_shadow`（`task-api/src/routing.rs`・`RoutingShadowRecorded`）が残っている。compare はそれを使わないと書いたが、残りの扱いはこの unit の範囲外。

## 提案
1. compare-store unit の範囲 check に `crates/task-api/src/query.rs`（event kind の一覧）と `web/` の生成型を最初から入れる（`script_compare_recorded` を足すと event kind の一覧と生成型が変わる）。
2. host-adapter unit の「Rust の結果を変えない」試験は、常に違う答え（`lane_rules` なら常に `cheap`）を返す試験用 bundle で `routing_decided` の payload を `rust` と比べる形にする。
