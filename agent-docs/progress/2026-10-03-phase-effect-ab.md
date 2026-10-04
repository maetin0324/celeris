---
title: phase_effect_ab: A/B 試験の最終検証（ab-verify）
tasks: [01M412JERQ4T85RDY32TZAHE9M]
wu: ab-verify
status: done
updated: 2026-10-03
completed: 2026-10-03
---
# phase_effect_ab: A/B 試験の最終検証（ab-verify）

Phase 3〜5 の有無（continuation の session resume、atomic_route の直行経路、review_sync の review 前同期）で、run 数・wall time・入力 token・新 session 数を比べる決定的な A/B 試験の最終検証。実装・試験コードは変更していない。

## ab-metric（`cargo test -p task-dispatch --lib phase_effect_ab -- --nocapture`）

```
ab-metric continuation off runs=3 wall_secs=360 input_tokens=120000 fresh_sessions=3
ab-metric continuation on runs=3 wall_secs=280 input_tokens=48000 fresh_sessions=1
ab-metric atomic_route off runs=3 wall_secs=360 input_tokens=120000 fresh_sessions=3
ab-metric atomic_route on runs=2 wall_secs=240 input_tokens=80000 fresh_sessions=2
ab-metric review_sync off runs=4 wall_secs=420 input_tokens=160000 fresh_sessions=4
ab-metric review_sync on runs=3 wall_secs=330 input_tokens=120000 fresh_sessions=3
```

## off → on の差（短い読み）

| scenario | runs | wall_secs | input_tokens | fresh_sessions |
|---|---|---|---|---|
| continuation | 3 → 3 | 360 → 280 | 120000 → 48000 | 3 → 1 |
| atomic_route | 3 → 2 | 360 → 240 | 120000 → 80000 | 3 → 2 |
| review_sync | 4 → 3 | 420 → 330 | 160000 → 120000 | 4 → 3 |

- continuation: on は同じ session を resume するため、新 session は 1 本だけになり、入力 token は約 60% 減る。run 数は変わらない。
- atomic_route: planner を省く直行経路で run が 1 つ減り、wall time と入力 token が下がる。
- review_sync: review 前に target へ同期するため、古い base での review と統合後の repair・再 review が 1 回ずつ減る。
- 値は偽アダプタの決定的な勘定（新 session 40000 token・resume 4000 token、SimClock の秒数）による。実 LLM の値ではない。

## 証拠

| 条件 | 実行したコマンド | 結果 |
|---|---|---|
| ab-metric 6 行と試験 pass | `cargo test -p task-dispatch --lib phase_effect_ab -- --nocapture` | exit 0。phase_effect_ab 4 passed / 0 failed。ab-metric 行 6 本 |
| workspace test | `cargo test --workspace` | exit 0。passed 3623 / failed 0 / ignored 13（全 binary の合計）。log は `artifacts/workspace-test.log` |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0。警告・エラーなし（`Finished dev profile`）。log は `artifacts/clippy.log` |
| 文書検査 | `sh scripts/dev/check-doc-links.sh` | exit 0（ok） |
| 文書検査 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（ok, 129 files） |
| 文書検査 | `sh scripts/dev/progress-index.sh --check` | exit 0（ok） |

## 各 WU の記録

- [ab-base](2026-10-03-phase-effect-ab/ab-base.md): 土台と scenario continuation
- [ab-direct](2026-10-03-phase-effect-ab/ab-direct.md): scenario atomic_route
- [ab-sync](2026-10-03-phase-effect-ab/ab-sync.md): scenario review_sync

## 未解決事項

- review_sync は review・repair の run がすべて新 session のため、fresh_sessions は runs と同数になる。resume による差分 token はこの scenario では出ない（ab-sync.md と同じ）。
- 数値は偽アダプタによる決定的な模擬値。実 LLM・実 wall time での効果は測っていない。
- 統合（main への取り込み）は試験内の `git merge --no-ff` で模擬している（review_sync）。配送側の統合経路は試験していない。

## 提案

- 実 LLM の 1 件ずつの実行で同じ 6 行を取り、模擬値と比べる。LLM 呼び出しを伴うため、認証の使える環境で人が実行して証跡を残すこと。
- review_sync の試験入口を task-dispatch から直接通せるようにし、統合衝突の模擬（Rereview と try_integration_repair の直呼び）を置き換える。
- ab-metric の行を CI の検査に入れ、6 行が出ることと off ≥ on を機械的に確かめる。
