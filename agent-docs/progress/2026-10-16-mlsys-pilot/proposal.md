---
title: "MLSys 本測定案: 消費係数の式と代入（実測なし。推定のみ）"
tasks: [01M52BTDJVYDZT6KGPNQV2KE6H]
unit: proposal
status: waiting-for-measurement
updated: 2026-10-16
---
# 本測定の件数・反復・上限の提案

**結論: 実測が 1 件も無い（pilot は人の決定 auth-ready=認証なしで待機、`run.md`）。係数 a,b,c,j,ρ,μ,u_x は未取得で、本測定の n・m・r・各上限は確定できない。** 下の「推定」は dry-run の仮置き係数から機械的に出した較正前の値で、実測ではない。

## 1. 係数表（runbook「本測定の件数・反復・上限の提案式」）
| 記号 | 意味 | 値 | 区分 |
|---|---|---|---|
| a | A の 1 試行あたり平均 tokens | 未取得 | 実測待ち |
| b | B の同 | 未取得 | 実測待ち |
| c | C の同 | 未取得 | 実測待ち |
| j | 試行あたり共有オーバーヘッド（Jev・CoS・planner・reviewer 按分） | 未取得 | 実測待ち |
| ρ | 再試行倍率（子 task・再試行込み / 初回） | 未取得 | 実測待ち |
| μ | tokens 欠測率 | 未取得 | 実測待ち |
| u_codex, u_claude | 7 日使用率 / token | 未取得（U0 も未取得） | 実測待ち |
| wall 最大 | pilot 最大 wall | 未取得 | 実測待ち |
| 参考: 平均 tokens/試行 | dry-run 251,673 ÷ 5 試行 = 50,335 | **推定**（仮置き係数、実測でない） | 推定 |

## 2. 式と代入
`E = r·(n·a + n·b·ρ + m·(c·ρ + j) + n·j)`、`s = 1.25`、制約 `s·E ≤ 1,000,000` かつ `U0_x + u_x·s·E ≤ U_x`（codex 21%、claude 24%）、`r·(2n+m) ≤ 12`。

**推定（実測でない）**: a=b=c=50,335、ρ=1、j=0 と仮定し r=1, n=5, m=2（12 試行）を代入:
E ≈ 12 × 50,335 = 604,015、s·E ≈ 755,019 ≤ 1,000,000 → tokens 制約は**推定上**満たす。ρ>1 や j>0 で余裕は減る（s·E ≤ 1,000,000 となる ρ の上限は a,b,c 実測なしには出せない）。使用率制約は u_x,U0 が無く**判定不能**。

上限の式（実測後に埋める）: `per_case.max_tokens=⌈1.5·max(a,b,c)⌉`、`per_case.max_wall_secs=⌈1.5·最大wall⌉`、`per_arm.max_tokens=⌈s·r·n·(arm平均·ρ)⌉`（≥ per_case）、`max_tokens=1,000,000`、`trial_timeout_secs=per_case.max_wall_secs`。現時点の値は未確定。

## 3. 承認範囲の判定
- 承認範囲: r=1、n≤5、m≤2、試行 12、tokens 1,000,000、fable 不使用。
- 暫定の候補 (r=1, n=5, m=2) は範囲内の上限そのもの。**範囲内かは係数の実測後に再判定**（現時点では tokens は推定上は内、使用率は不明）。
- 範囲超過（r>1 や n>5）は提案しない。統計上それが要ると実測で分かれば `final-measure-budget` の新しい決定として人に出す（承認待ち）。現時点では新しい決定は要求しない。

## 4. 不足データ（実測に必要）
1. H1 昇格・H4 token・認証確認と U0（`run.md` §4–6）。
2. `summary.json` の A/B 別 tokens、`tokens_missing`、wall、`max_overshoot_*`、子 task・再試行数、Δ使用率。
3. C の Jev smoke（jev_ok/fallback、lane、費用・版）、B/C の routing_override 記録。
実測後、本ファイルの §1・§2 を実値で更新し、「推定」印を外す。
