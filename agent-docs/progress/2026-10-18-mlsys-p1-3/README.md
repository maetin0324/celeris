---
task: 01M56HCNTPN0H2VGP4JE0JYRM9
title: 本測定移行 P1-3 の進捗（K2/K4 実資産・能力 probe）
status: 無 LLM 検証まで完了・実経路は人手順待ち
updated: 2026-10-18
---
# P1-3 進捗

区分: 実装済み / 無 LLM 検証済み / 実経路確認済み(人手順待ち) / 承認待ち / 未検証。

| WU | 内容 | 区分 | 根拠 |
|---|---|---|---|
| cap-probe | 必要能力の固定（case.json）、A と同一経路の probe、B/C 同等、不足は env_failure で preflight 停止 | 実装済み・無 LLM 検証済み（fixture/stub runner で `cap_probe` 6 件） | 2026-10-18-mlsys-cap-probe/cap-probe.md |
| 〃 実 A sandbox 実走 | userns 要。人が host で | 実経路確認済み(人手順待ち) | cap-runbook §1 |
| 〃 cap env 実値（UCX prefix・sirius 管理 socket） | host 側の承認が要る | 承認待ち | cap-runbook §0 |
| grader-real | env/model 失敗の分離、K2 正負例・K4 再計算の script と試験 | 実装済み・無 LLM 検証済み（fixture 9 件。実 asset で組立・lock・patch・K4 再計算 ok） | 2026-10-18-p1-3-real-asset-grade/grader-real.md |
| 〃 K2 実 build の正負例 | async-ucx 入れ子 submodule 欠落で env_failure 停止 | 未検証（asset 追加が先） | cap-runbook §2 |
| 〃 known-good patch（p0-final.patch）の asset store 配置 | 無い | 承認待ち | cap-runbook §2 |
| k2-scope | K2 の judged_scope、報告の主張制限 | 実装済み・無 LLM 検証済み（`k2_scope` 6 件） | 2026-10-18-p1-3-real-asset-grade/k2-scope.md |
| close | cap-runbook・本記録・全体検査 | 実装済み | 本 dir |
| Sirius 接続到達確認 | `ssh -O check` のみ | 実経路確認済み(人手順待ち) | cap-runbook §3 |

有料ジョブ・本測定・LLM 呼出しは起動していない。

## K2 範囲の選択と理由
選択肢 2「判定範囲を事前明示」を採用（ADR agent-docs/adr/2026-10-18-k2-judged-scope.md）。K2 は implementation・experiment のみ判定し、analysis・review は not_judged。
理由: 分析/レビューの最小判定を足すには既知の誤り list と模範解答の新設が要り、結果を見た後に grader を変えない・巨大な問題集を作らない方針に反する。
K2 の成功は「実装と実験の判定を通った」ことだけを意味する。grade_case_ref を更新したので lock は新 hash で作り直し、旧試行と混ぜない。

## 旧 pilot 結果を流用しない
旧 pilot（取消済み task を含む）の結果・試行・lock は本 P1-3 の実経路証拠に使わない。取消を新しい実測承認とみなさない。
無 LLM 検証は fixture・stub・実 asset の決定的検査のみで、実経路確認は人手順（cap-runbook）の結果で初めて上がる。

## 本番への移行
- 本 task は本番 host・config・release・DB を変更していない。
- 人が行うこと: cap-runbook §0〜§3（cap env の承認と作成、実 A sandbox での cap-probe、asset store への入れ子 submodule と p0-final.patch の追加と lock 固定、`verify-real-assets.sh --build`、Sirius 接続確認）。
- 本測定の開始は別途の承認（新しい測定 ID で campaign lock を作り直す）。

## 全体検査
- `bash scripts/dev/test-parallel.sh`: exit 0（passed 5739 / failed 0 / ignored 15）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 未解決
- 実 build の asset 不足（上記）。実 A sandbox 実走の結果。
