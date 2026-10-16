---
title: "campaign 独立反復と内部再試行の分離・分母を減らさない集計"
tasks: [01M526E98AFXB02ZG468J789FB]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# repeat-denominator

## 実装済み
- ADR `2026-10-16-campaign-repeats-denominator`（D1〜D6）。
- manifest `repeats`（既定 1、1 は直列化しないので既存 hash 不変）。`validate` は `max_attempts_per_trial != 1` と `repeats == 0` を Err。
- 反復は別 ID `rep`（TrialRecord / TrialKey / TrialRequest / TrialResult）、out は `trials/<arm>/<problem>/rep<k>`、台帳の冪等キーも rep 別（rep 1 は従来と同じ）。`trial_no` は常に 1。
- 依存判定は同じ rep どうし。plan は既存経路のまま (arm, problem, rep) を並べる。
- 分母は計画された試行総数（problems × repeats、C は subset × repeats）。aggregate / checker の recompute / runner の summary を (config, problem, rep) 単位に直し、`trial_no == 1` 決め打ちを除去。checker は rep 範囲外・重複を違反にする。dryrun の見積は repeats 倍。

## 証拠
- `cargo test -p celeris-campaign --test denominator`: 3 passed（attempts≠1 / repeats=0 拒否、別 out・別 ID で 15 試行、cancelled+missing を含む cell が n=6 割り）。
- `bash scripts/dev/test-parallel.sh`: 5483 passed, 0 failed。`cargo clippy --workspace --all-targets -- -D warnings` 通過、`cargo fmt --all -- --check` 差分なし。

## 未解決・提案
- 既存の `trials/<arm>/<problem>/1` 形式の out は読めない（checker は workspace 一意性のみ見る）。進行中の campaign があれば作り直す。
