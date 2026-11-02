---
title: S2 lane-rules-ts — control-ts に lane_rules 領域の TS 実装・単体試験・fixture 比較 script
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 lane-rules-ts

## 結論
- `control-ts/src/domains/lane_rules.ts` に `crates/task-core/src/model_policy.rs` の `match_rule` と同じ順・同じ文字列の規則表（5 規則）を書いた。入力 `{ features: TaskFeatures }`、出力 `{ rule_id, proposed, condition }`（欄の順は付記 S2-2 の宣言順）。
- `LaneCeiling::clamp`・人の明示・System・reasons・policy_version は TS に入れていない（Rust に残る）。
- 入力の検査: input の余計な欄、features の余計な欄、欠けた軸、`low|medium|high` 以外の値（大文字・null を含む）は `Error` を throw する。Rust の `deny_unknown_fields` と同じ受け入れ範囲。
- 規則に当たらないときの fallback（`no rule matched`）は Rust と同じ値を返す形で残した（最後の `standard/default` が必ず当たるので到達しない）。policy-split の提案への答え: 型で到達不能を示すより、Rust と同じ形を保つ方を選んだ。
- `src/domains/index.ts` と `bundle.config.json` の domains に `lane_rules` を登録した。capability は空（host の op を呼ばない）。
- `control-ts/scripts/compare.mjs` を作り、`package.json` に `"compare": "node scripts/compare.mjs"` を足した。Node 24 の型除去で `src/domains/lane_rules.ts` を直接 import し、fixture の各行の `features` を `laneRules` に通し、3 欄を欄ごとの JSON 直列化の byte 列で比べる。TS の戻り値に余計な欄があっても不一致にする。不一致・throw があれば最初の 5 件を出して exit 1、引数の誤りは exit 2。`--fixture <path>` で別の fixture を渡せる。
- `scripts/build-bundle.test.mjs` の manifest の domains の期待に `lane_rules` を足した（登録したので変わる）。

## commit（件名）
1. `phase s2: lane-rules-ts: lane_rules 領域の TS 実装・単体試験・fixture 比較 script`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `corepack pnpm@12.6.0 run compare lane_rules`（control-ts）— exit 0、`lane_rules: 19683/19683 matched (fixtures/lane_rules/all-features.jsonl)`。
- `corepack pnpm@12.6.0 run typecheck` — exit 0。
- `corepack pnpm@12.6.0 run test` — exit 0。vitest 14 passed（lane_rules 10 件: 規則ごとの境界 4・順序 1・既定 1・未知の値・欠けた軸・余計な欄・dispatcher 経由の 3 欄、ほか domains 4 件）、node --test 25 passed（compare.test.mjs 4 件を含む）。
- `corepack pnpm@12.6.0 run bundle` — exit 0。
- `bash scripts/dev/test-affected.sh` — exit 0（対象は control-ts の typecheck+test だけ）。
- `cargo clippy --workspace -- -D warnings` — exit 0。`cargo fmt --all -- --check` — exit 0。
- `git diff --stat "$CELERIS_WU_BASE" -- crates/` — 差分なし。

## 未解決事項
- 入出力の型は lane_rules.ts に手で書いた。付記 S2-2 は `docs/api/script-host/host-v1.schema.json` に DTO を足して TS の型を生成するとしているが、schema への追加は Rust の DTO（`LaneRulesInput`・`LaneRulesOutput`）を足す unit（dispatch-hook / host-adapter）の範囲。その時に手書きの型を生成型へ置き換える。

## 提案
- なし。
