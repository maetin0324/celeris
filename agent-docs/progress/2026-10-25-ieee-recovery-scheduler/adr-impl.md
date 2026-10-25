---
title: IEEE recovery scheduler — ADR 実装付記（adr-impl）
tasks: [01M5SSBCP75CTVF2SHD0QQ9ZGG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# ADR 実装付記（adr-impl）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。コードは変えていない。

## やったこと

[ADR](../../adr/2026-10-25-recovery-scheduler-backend.md) の末尾に「実装付記 2026-10-25」（I1–I9）を追記した。
D1–D5 と後続段の受入境界の本文は変えていない（差分は追記のみ）。

- I1 module 配置: `crates/campaign/src/dagbench/pbs/{transport,intent,backend,raw}.rs`。observe の PBS パーサ・照合規則を共用し、ipc server を `dyn ExtBackend` 所有へ広げる。
- I2 `PbsTransport`（qsub / `qstat -x -f` / marker 検索、全 call に全体 timeout、検索 capability 未確認は `Unsupported` で停止）。
- I3 intent 台帳と状態遷移（intent_recorded→sent→accepted|unknown_submit→reconciled|unresolved_stop）、決定的 `submit_intent_id`、file lock による排他、再起動時は送り直さない。
- I4 `Lookup::Unresolved`/`SubmitError::Unresolved` を足し、baseline/celeris の再投入分岐へ戻さない。unknown を not_submitted にしない。
- I5 両 arm 共通の `RecoverySpec`（既定値なし、hash pin）。
- I6 trial 結合行と validator、再収集は read-only（qsub・消費を呼べない view）。
- I7 `celeris-qsub` は主比較 A arm 用に残し、recovery の投入経路にはしない（二重経路なし）。
- I8 偽 transport の offline fixture 18 件（応答喪失・0/1/複数候補・履歴消失・不一致・timeout・再起動・二重 submit 競合・grade 失敗ほか）。

## 証拠

- `git diff $CELERIS_WU_BASE -- agent-docs/adr/2026-10-25-recovery-scheduler-backend.md | grep '^-' | grep -v '^---'` → 0 行（削除・変更なし、追記のみ）。
- 範囲 check: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は ADR とこの進捗ファイルだけ。
- 文書のみの変更で crates の差分はない。必須検査の結果は本 WU の報告に記す。

## 未解決事項

- marker 検索の実 command（qselect/履歴保持/Variable_List の可視性）は接続後の read-only 確認まで未定（D4.4）。
- 認証・recovery 承認残枠は未確認のまま。実 scheduler への通信はしていない。

## 提案

なし。
