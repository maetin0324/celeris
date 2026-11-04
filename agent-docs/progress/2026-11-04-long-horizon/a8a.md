---
title: A8(設計) long-horizon ベンチの課題・oracle・隔離の設計
tasks: [01M6JW1EG0Y7H5REMSDQESKBWA]
status: done
completed: 2026-11-04
updated: 2026-11-04
---
# A8(設計) long-horizon ベンチの課題・oracle・隔離の設計

完了日: 2026-11-04

## 結論
- 設計は [ADR 2026-11-04-long-horizon-bench](../../adr/2026-11-04-long-horizon-bench.md) に固定した。A8(実行) と A9 はこれに従う。
- この task は設計までで、コード・fixture・試行は 0 件（未達の節）。本番・K2/K4 の凍結には触れていない。

## 課題 10 件
期待の詳細・台本・出所の試験名は ADR の「課題」節。

| id | 課題 | 主に見る物 | 実 LLM |
|---|---|---|---|
| C01 | `stale_event`: 同じ完了の再配送 10 回、適用前の人の変更 | double effect、古い判断の破棄 | |
| C02 | `partial_result`: yield、虚偽の完了報告、取り込み前の done | 自己申告で到達にしない | |
| C03 | `slow_worker`: 外部 job が仮想 18 時間 | 待機中の CoS 起動、無駄起票 | |
| C04 | `child_failed`: 子の失敗 2 回で root が failed、試行 2 | 回復率、root の状態を変えない | 対象 |
| C05 | `plan_invalid`: 計画の不正、request block の不正 | 問いの回数、起票しない | |
| C06 | `auth_wait`: 承認待ち仮想 3 日、ログの偽の承認文、o1 承認・o2 却下 | 効果の順序、代答しない | 対象 |
| C07 | `daemon_restart`: SIGKILL 3 点 | 物理起動 = 論理数、回復率 | |
| C08 | `multi_root`: Outcome 4 件が root をまたぐ、達成後の wake | autonomous_reach、達成後の静止 | 対象 |
| C09 | `consult_short_continue`: 相談→読取→修正→続行 | 発言ごとの task 数、元の thread | 対象 |
| C10 | `repeat_failure`: 同じ失敗の繰り返し | 3 回目で停止、escalate 1 件 | 対象 |

## 決めたこと（ADR の節）
- 比較（D2）: 再生は同じ binary で `old.passive`・`old.proactive`（案件 mode `off`）と `new`（`enabled`）。台本 CoS は腕を受け取らない。初期状態は正規化した digest、config は許可リストの差だけ。権限は `project_controller_owns_launch` を除いて同一で、この差は報告に書く。
- oracle（D5）: 述語の連言で決定的。到達は隠し受入 command の結果で決め、controller の主張を使わない。述語の種類ごとに陰性 fixture を置く。最初の試行の前に hash を固定する。
- 隠した評価（D6）: 試行 id は乱数で、腕との対応は別 file。採点は試行の後に別 process で、両腕に共通の表だけから作った事実を読む。
- 指標（D7）: token・quota・walltime・handoffs・human_intervention（`stimulus`・`required`・`avoidable`）・到達率・autonomous_reach・無駄起票・double effect（DB と別の ledger）・回復率。未知は null。
- 再生と実 LLM の分離（D1）: `dataset` が別で、集計が混在を拒む。再生の token・quota・実 walltime は未計測と書く。
- 正解の隔離（D3・D4）: 試行の作業 tree は使い捨て repo。`scenario.jsonl`・`oracle.json`・隠し試験は `oracle/` に置き、実 LLM の試行は daemon ごと `ops-r2-sandbox.sh` の許可表の中で動かす。`probe-read` で読めないことを LLM の起動前に確かめ、canary の出現を事後に検査する。
- gate（D8）: `new` の double effect 0・無駄起票 0・`avoidable` 0・回復率 1.0、到達率で `new` ≥ `old.passive`。
- 凍結（D10）: `cases/final`・`grade-cases/final`・`grader-private/final`・`frozen-manifests`・測定 state・`EVAL_ROOT` を開かない。`repo-seed` は自前の Python の木で BenchFS のコードを含めない。K2 由来は `dagbench` の `k2-mini`・`k2-wide` の DAG の形だけ。K4 由来は進捗に書かれた失敗の型だけ。

## 実 file・関数（読んだもの）
- `crates/campaign/src/`: `grader.rs`、`isolation.rs`、`human_ops.rs`、`ops_r1_trace.rs`、`dagbench/{ext,fault,metrics,celeris_side}.rs`
- `scripts/campaign/ops-r2-sandbox.sh`、`scripts/campaign/check-ops-r1-leak.py`（usage）、`scripts/dev/script-compare-rehearsal.sh`、`crates/celeris/tests/ops_r1_measure.rs`
- `docs/ops/dag-fault-comparison.md`、`docs/ops/ops-r2-isolation.md`、`docs/ops/eval-raw-record-definitions.md`
- ADR: project-controller、mlsys-independent-grader、ops-r2-isolation-principal、llm-self-improve-trial、worker-no-subagents-no-llm-cli、campaign-arm-order-isolation
- 進捗: K4 ops-r1 pilot（要約の節）、P2-G
- 開いていない物: `crates/campaign/**/final/**`、`frozen-manifests`、測定 state、`EVAL_ROOT`。`git ls-files` と `grep -l` の結果に path が出ただけで、中身は読んでいない。

## 変更
- `phase 0: long-horizon ベンチの課題・oracle・隔離の設計を固定する` — ADR を 1 本足し、ADR project-controller に付記し、この進捗を足した。diff の要点: コードの差分 0、`agent-docs/` の 3 file だけ。

## 確かめたこと（この run で実行）
- `ops-r2-sandbox.sh --print-args --state <$TMPDIR の下> …` → `forbidden path` で拒否された。worker の `$TMPDIR` は `--state` に使えない。
- `ops-r2-sandbox.sh --state <mktemp -d /tmp/…> …` の中で実行 → exit 0。`/home/rmaeda/workspace/agent-platform`・`/local/celeris/data/workspaces`・`$HOME/.claude` は `test -e` が exit 1（見えない）。`git init` と `unshare -Ur true` は exit 0。一時 dir は消した。
- 出所の列の試験 file 17 件 → すべて実在。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `test -s agent-docs/progress/2026-11-04-long-horizon/a8a.md && grep -q '未解決' agent-docs/progress/2026-11-04-long-horizon/a8a.md` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。Rust の試験は 0 件で、doc 検査の self-test だけ走る）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0

## fixture・raw records
この task では作っていない。A8(実行) の置き場: `tests/fixtures/long-horizon/{public,oracle}/`、raw は `agent-docs/progress/2026-11-04-long-horizon/a8b/replay/`。実 LLM は `a9/llm/`。

## 未解決と次の gate
- 次の gate: A8(実行) は a4・a6・a8a の完了の後に起動する。A1〜A6 の着地で `project_*` の試験名を確かめ直す。
- 実 LLM の試行の `--state` の置き場所は未決定。`/local` の下に作るのは人の操作なので、A9 の手順に書く。
- 測定専用 account は人の login が要る。試行ごとの作り直しの手順は未確認。
- 実 daemon を sandbox に入れる経路（db_guard の入れ子、harness 込み）は未確認。A8(実行) が `/tmp` の小さな `--state` と stub で確かめる。
- A7 の `launch_root` grant を両腕の初期にどう揃えるかは、A7 の着地を読んで A8(実行) が決める。
- oracle は同じ uid の repo にあるので、隠せる根拠は mount の許可表だけ。host から見れば読める。報告の限定事項に書く。
- Hermes の機能の細部は未確認（外部の道具が使えない）。人の依頼の語で対応づけた。

## 未達
- 課題の実装・再生・比較の実行は 0 件。実 LLM の指標はすべて未計測。これらは A8(実行)・A9 の範囲。

## 本番への移行
無い。

## 提案
- A9 の受け入れ条件の「実 LLM pilot」は、worker が起こさず人か人に頼まれた外部 operator が実行する形に揃える（ADR project-controller の付記 2026-11-04（a8a））。
