---
title: "MLSys 本測定 freeze-final: 条件固定（final manifest・task-template・grade-case 対応表）"
tasks: [01M52D2AM5K6GCFGEJZW4REWM2]
unit: freeze-final
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# freeze-final

本番・実 LLM には触れていない（validate / dry-run のみ、`llm_calls_made` 0）。以下は**実測ではない**。

## 1. 固定した条件（`final.json`）
| 項目 | 値 |
|---|---|
| campaign_id / split | `mlsys-final-1` / `final`（入力は `final-input-manifest.json`、problems は split `held-out`） |
| problems（A/B 主・C subset とも） | `k2-bf-xattr-restart-campaign`、`k4-cmp4-validity-analysis`（`crates/campaign/cases/final` の 2 件。dev の case は 0 件） |
| model | 全 arm `claude-sonnet-5-5`（版 `claude-sonnet-5-5`）。fable なし（`grep -ci fable` = 0） |
| A | harness `claude-code`、policy `ops-baseline` |
| B | harness `celeris`、policy `fixed`、routing `lower_policy=mlsys-min-1`、`online_update=false` |
| C | harness `celeris`、policy_version `jev-1`（pilot-live.json と同じ） |
| repeats / parallel | 1 / 1 |
| budget | per_case tokens 120,000・wall 1,800s、per_arm tokens 360,000・wall 20,000s、max_tokens 1,000,000、wall 36,000s、disk 128GiB |
| termination | trial_timeout_secs 1,800（= per_case.max_wall_secs）、max_attempts_per_trial 1、stop_on_budget |
| snapshot | materials_sha256 `dab3ca05d01adf1da268c8ec35e24f33249ca3cc02083e0a2e6b487d0c5a800d`（`$M` = `<case-id>/` 配下に 2 件の materials を並べた木）、kb_snapshot `741f2414…`、repo_commit は文字列 `final-cases-1: k2=086c4618…, k4=none` |
| input manifest sha256 | `b40a2b0dd5519e780aacdbf005e172a138abf129e079a22b22dc3ac47f90db74`（`final-input-manifest.json`） |

## 2. 検証結果（acceptance 0・1・3）
- `celeris-campaign validate --manifest agent-docs/progress/2026-10-16-mlsys-final/final.json` → exit 0、**sha256 `9ab21360e84b183678b32d1d30c76e7bd09dad75db43ab5fdb58806ed18269ae`**（これは `<release sha12 at H1>` が未置換の hash。置換後は変わるので run-final.md §2 で控え直す）。
- `dry-run` → exit 0、**within_budget true**、**total_trials 6**（A 2・B 2・C 2。承認の 12 以内）、推定 303,195 tokens（A 100,065 / B 100,065 / C 103,065）、`llm_calls_made` 0。
- dev 混入なし: `validate`/`dry-run` が problems の split を `held-out` で照合し通過（dev 問題が混じれば拒否される）。case は `crates/campaign/cases/final` の 2 件のみ。

## 3. 係数は実測でなく保守値（acceptance 3）
- dry-run の推定は仮置き係数（`context_overhead 30,000`・`output 20,000` tokens/試行、`jev 1,500`）で、**実測値ではない**。per_case 120,000 は仮置き平均（約 50,000）の 2.4 倍、per_arm 360,000 は 2 試行の想定 ~100,000 の 3.6 倍で、いずれも**上限側の保守値**。
- **pilot の結果は存在しない**（`../2026-10-16-mlsys-pilot/run.md`: 認証なしで待機、実 LLM 未実行。`proposal.md` も「実測待ち」）。係数 a,b,c,j,ρ,μ,u は未取得。このため上限は実測較正ではなく、承認済みの枠（`max_tokens` 1,000,000、`per_case` 120,000、`per_arm` 360,000）の内側に置いた。本測定の実走行の後に実消費を集計して枠内であることを示す（grade-export WU）。

## 4. 乖離の記録（人の確認が要る）
- **A/B 各 5 問は満たせない**: `crates/campaign/cases/final` にある case は 2 件（k2・k4）。dev の case は混ぜない規則のため、**A/B 各 2 問・C subset 2 問（試行 6）**で固定した。5 問にするには final case を 3 件増やす別作業（cases WU 相当）が要る。統計的力は下がる（提案: 結果は 2 問のペア比較として報告し、結論に使わない）。
- 2 問のため試行は 6 で、承認枠 12 の半分。余りは使わない（`r>1` は新しい決定）。
- k2 の repo（BenchFS submodule 未固定）、K3 の暫定統計量（R5）、隠し試験の移植（cases.md R2・R3・R5）は未解決のまま。

## 5. task-template と grade-case 対応表
- `task-template.json`（pilot と同じ形。`project_id` は運用セッションが置換。acceptance は reviewer）。
- grade-case の置き場: `agent-docs/progress/2026-10-16-mlsys-final/grade-case/<case-id>.json`（`celeris.grade-case/1`。**このユニットでは作らない**: runbook 未接続 1・2。grade-export WU が作る。grader-private の値は試行の作業場所へ渡さない）。
| trial（`$D/final-out/trials/…`） | arm | case | grade-case file |
|---|---|---|---|
| `A/k2-bf-xattr-restart-campaign/rep1` | A | k2 | `grade-case/k2-bf-xattr-restart-campaign.json` |
| `B/k2-bf-xattr-restart-campaign/rep1` | B | k2 | 同上 |
| `C/k2-bf-xattr-restart-campaign/rep1` | C | k2 | 同上 |
| `A/k4-cmp4-validity-analysis/rep1` | A | k4 | `grade-case/k4-cmp4-validity-analysis.json` |
| `B/k4-cmp4-validity-analysis/rep1` | B | k4 | 同上 |
| `C/k4-cmp4-validity-analysis/rep1` | C | k4 | 同上 |
同じ case の 3 arm は同じ grade-case を使う。

## 6. 成果物
`final.json`・`final-input-manifest.json`・`task-template.json`・`run-final.md`・`data/`（出力置き場、空）。`crates/` は変更していない。
