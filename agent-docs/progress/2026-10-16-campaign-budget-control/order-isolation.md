---
title: "campaign arm 順の巡回/無作為化と executor 失敗の隔離"
tasks: [01M526E98AFXB02ZG468J789FB]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# order-isolation

## 実装済み
- ADR `2026-10-16-campaign-arm-order-isolation`（D1〜D5）。
- manifest に `order`（`abc_rotation` 既定 | `seeded_shuffle`）と `seed`。既定値は直列化しないので既存 manifest の hash は不変。`CampaignManifest::arm_sequence` が problem 内の arm 順を決め、`plan()` が使う。依存順 `problem_order` は守る。
- trial 記録に `arm_rank`、run-meta に `order`/`seed`。
- executor の Err（sync execute／async lookup・submit・track・poll・usage）は run を止めず、その試行だけ `error/executor_error`。async は外部 task を cancel。観測が無いので予算には課金しない（`RunReport.executor_errors`、summary `budget.executor_errors`）。
- `ledger_async` の「submit 応答喪失で run が Err」試験は新挙動（A のみ error・orphan task を cancel・B/C 続行）に更新。
- MockStep::Err を追加。

## 証拠
- `cargo test -p celeris-campaign --test arm_isolation`: 7 passed（巡回・seed 再現・記録・A の Err/Wait/arm 枯渇/async submit Err で B/C 完走・checker 違反なし）。

## 未解決・提案
- `executor_error` を課金しないので、全 call が即 Err する A は arm/global 上限では止まらない。再試行しない（max_attempts_per_trial=1）ため実害は試行数が上限。
