---
title: IEEE recovery scheduler — 実行票の判定と smoke 段の総括（smoke-close）
tasks: [01M5TCGEVJF1QW03CDWPJRRKZ5]
status: done
updated: 2026-10-26
completed: 2026-10-26
---
# 実行票の判定と smoke 段の総括（smoke-close）

完了日: 2026-10-26。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。コードは変えていない。
Sirius・claude_max_lab への接続はしていない（利用者のコメント 2026-10-25: 保守と login 不可の間は接続しない）。

## やったこと

- [実行票](../../../docs/experiments/recovery-scheduler-smoke.md)の「判定」節を [live-smoke](live-smoke.md) の結果に合わせて書き直した:
  `hpc_ready = false`、接続（実装）は完了・実機受入は未完了と分けて書き、残る人の判断 3 件（`place` の決め直し、
  課金単位の換算、再実行の時期）と残る実装の不足（marker 検索 `Unsupported`、拒否の分類）を挙げた。

## 受け入れ条件と証拠（提出時の HEAD で実行）

- 条件 0「判定節が live-smoke.md の hpc_ready と一致」: live-smoke.md の判定は `hpc_ready = false`、実行票の判定節も
  `hpc_ready = false`。`grep -n 'hpc_ready = false' docs/experiments/recovery-scheduler-smoke.md agent-docs/progress/2026-10-25-ieee-recovery-scheduler/live-smoke.md` で両方 1 件以上。
- 条件 1「文書検査 3 本・test-affected・clippy・fmt が exit 0」: 下の表。

CHECKS_TABLE

## smoke 段の総括

- 実行数: smoke の trial 起動 1（baseline/normal）、受理 job 0、新規の比較試行 0。12 試行比較・本番 controller 停止・
  無関係 job の取消はしていない。
- P0-2 identity 照合: 実行票の「P0-2 identity 照合」節。ADR D3 の 6 行のうち一致 4、部分一致 1（marker の account_id、U1）、
  照合対象なし 1（外部操作、U3）。
- 10/29 JST は目標日で、期限ではない。実機受入は人の判断（place）と Sirius の復旧の後になる。

## smoke 段の commit（件名）

- 「smoke-adr: ADR appendix I15 for the real-cluster smoke path (approval gate, SshSite, close entry, payload)」
- 「payload: recovery smoke job.sh, fixed input, expected hash, spec template and selftest (ADR I15 (4))」
- 「ssh-entry: approval gate, SshSite and close_from_env for the ssh PBS path (ADR I15 (1)-(3))」
- 「ssh-entry: progress and checks at the submitted HEAD」
- 「runbook: recovery smoke runbook and P0-2 identity reconciliation」
- 「runbook: record checks at the submitted HEAD」
- 「campaign: pass RECOVERY_SMOKE_DIR to the payload via qsub -v (ADR I15 (4))」
- 「live-smoke: record Sirius smoke stop (qsub rejected place=exclhost), hpc_ready=false」
- 「live-smoke: record checks at the submitted HEAD」
- 「smoke-close: align the runbook verdict with live-smoke and summarize the smoke stage」

## 未解決事項

- 実機受入（同一 job 継続・回収・二重投入/消費 0・独立 grade・raw 再集計一致）は未検証。`place=exclhost` が gen の
  1 vnode 要求で拒否される。測定 ID `rcv-smoke-20261026-1` は再利用しない。
- 承認枠と PBS 課金単位（1/3 host-hour）の換算が未照合。unknown submit の marker 検索は `Unsupported`。
- U1（marker の account_id）・U2（recovery 専用 manifest 未作成）・U3（P0-2 の実 job 不在）は実行票の記載のまま。

## 本番への移行

本番 host・config の変更は無い。再実行は人が行う手順として live-smoke.md の「人が行う手順」節のとおり
（配置の決定 → ADR D2・`spec.template.json`・資源表の更新 → 新 measurement_id で live-smoke）。

## 提案

- 再実行の配置は `place=excl`（live-smoke.md の提案と同じ）。
- qsub の明示的拒否（非 0、`Job rejected`）を unknown submit から分け `rejected` とする（ADR D4 の分類の見直し）。
  拒否で ledger に sent が残り測定 ID を使い捨てる今の挙動もこれで避けられる。
