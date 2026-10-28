---
title: actor trace report照合の補修
tasks: [01M60KY19FKVMTYM7AFPK32W4V]
status: done
updated: 2026-10-28
---
# actor trace report照合の補修

完了日（流れ4）: 2026-10-28

## 流れ4（flow4-report）

- `phase 1: 流れ4の配備観測をhuman-ops reportまで照合` — 一時DBのhuman:webの回答と、隔離fixtureのpromote成功・rollback成功・start失敗からhuman-ops /2の8生記録を作る。`Harness::outputs`から既存report builderへ渡す。既存の成功summary・失敗ファイル・status/exit_code・verifyの検査を維持した。
- 人の判断はhuman/manualの1秒、実行者はexternal_operator:flow4。固定時計はauditのbeginで10秒、finishで12秒を返す。配備の実所要時間は各2秒、計6秒がhistoryに残り、人の時間は1秒・観測区間1件のまま。要求単位の主体別件数はhuman 1・external_operator 1、除外操作は3件、人の介入はsafety_approval 1件。
- report historyはtargets・outcomeを直接出さないため、logical_op_idで入力生記録のtargets・outcomeへ結び、deployment_auditのfrom/to・decision_refs・task_refs・statusと照合する。request_refはrequestsの実要求、判断者はexportの実介入と照合する。policyはdecision参照で一致する。期待report JSONは作らない。
- 消費・人待ちは未観測のnull、exportはusage未取得・quota nullを維持する。配備にrunを推測で付けず、export/requestsも配備前後で変わらない。製品コード・共有helper・root tests/support・scriptsを変更しない。新しい設計判断は無い。
- 初回検査ではログ変数名の衝突、必須credentialの不足、auditのUTC表記（+00:00）を生記録が要求するZ終端へ変換する不足を検出して修正した。operatorのcredentialは観測できないためunknownとする。

### 証拠（提出時のHEADで実行。基準は $CELERIS_WU_BASE）

- `cargo test -p celeris --test actor_trace_flow4` → exit 0、1 passed・0 failed。
- `bash scripts/dev/test-affected.sh --list` → exit 0、celeris・celerisctl。
- `bash scripts/dev/test-affected.sh` → exit 0、813 passed・0 skipped（55 binaries）、doc test 0件・exit 0。前段check-healthも通過。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分なし。
- `sh scripts/dev/check-doc-links.sh` → exit 0。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0、329 files。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `git diff --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0。流れ4試験と本進捗の2ファイルのみ。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/celeris/src crates/task-core crates/task-api crates/task-ops crates/task-worker tests/support scripts` → exit 0。製品コード・root tests/support・共有scriptの差分なし。

### 未解決

流れ5とmain基準の最終全5target検査・ADR/親進捗更新は後続WorkUnitの担当。

### 提案

なし。
