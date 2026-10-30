---
title: ops-r2 fixture probe — CoS 直接遂行と task 経由の両方で隔離・主体・記録が効くか（WU probe）
tasks: [01M669QB069GND2ZKA1T97QPY9]
status: done
updated: 2026-10-30
---
# ops-r2 fixture probe（WU probe）

完了日: 2026-10-30

結論: impl 段（isolate・principal・session）の成果は、B の CoS が自分で済ませる経路と task 経由（worker run）の経路の
両方に効く。根拠は fixture と実 bwrap の範囲に限る。試験は `crates/campaign/tests/ops_r2_probe.rs`（3 件）。
probe の途中で不具合を 1 件見つけて直した。`probe-read` が `errno` を `"errno:13"` という文字列で出していたが、
`check-ops-r1-leak.py` と `docs/ops/ops-r1-leak-check.md` は整数を求めるため、実 probe の JSON を `--runtime-probe` に
渡すと漏洩検査が exit 2（入力の誤り）で止まっていた。`runtime_probe::PathProbe.errno` を `Option<i32>` に直した
（OS の番号が無い error は -1）。schema 名 `celeris.runtime-probe/1` は変えていない。文書の記載に揃えた修正。
CoS の方針・reviewer の経路・本番設定は変えていない。実験 LLM・実 HPC 負荷・本物の daemon は使っていない。
基準は `$CELERIS_WU_BASE`。

## 変更
- `ops-r2 probe: CoS 直接遂行と task 経由の fixture probe、probe-read の errno を整数にする`
  - `crates/campaign/tests/ops_r2_probe.rs`（新規、3 件）
  - `crates/campaign/tests/ops_r2_probe/daemon.sh`（新規）: 測定 daemon の代わりの fixture。`ops-r2-sandbox.sh` の中で
    CoS chat run（daemon の直接の子、cwd は thread の dir）と worker run（daemon → worker → harness の孫、cwd は
    task の作業場所）を起こし、それぞれ `probe-read` を走らせる
  - `crates/campaign/src/runtime_probe.rs`・`preflight_isolation.rs`: `errno` を整数にした

## 試験の中身
1. `cos_direct_and_worker_runs_cannot_read_private_old_or_other_trial`（userns が要る）
   - 実 `ops-r2-sandbox.sh` の中の daemon fixture で、CoS run と worker run は同じ pid namespace にいる（host の外）。
   - 両経路とも private grader・旧解答（過去の campaign の out）・他試行の state の出力が readable=false・listable=false、
     許可表の外の mount source は 0 件。
   - 対照として deny list に入れた公開入力（試行の `--state` の下）は読める。拒否は偽りでなく実際に開いた結果。
   - 両経路の probe JSON をそのまま `check-ops-r1-leak.py --runtime-probe` に渡すと exit 0・pass。
2. `leaks_and_residual_deny_readable_fail_closed_on_both_paths`（userns が要る）
   - canary 入りの private の写しを許可領域（`--state/inputs`）に置くと、漏洩検査は exit 1・pass=false。結果 file に canary は書かれない。
   - 許可表に旧解答の dir が残った（`--ro` で足した）B の sandbox では、preflight と同じ `preflight_isolation::run` の
     B の probe が `deny_readable:B:<旧解答>` を返す。daemon fixture の CoS run・worker run の両方でも同じ path が
     `deny_readable` になる（exit 1）。
   - この findings が preflight の `missing` に入り、`run_gated` が有料実行・task 作成を 0 件で止めることは
     `tests/preflight.rs` の `one_readable_path_in_any_principal_stops_the_run` が固定している（この unit では変えていない）。
3. `relay_is_sourced_reconciled_and_counted_once_for_both_paths`（userns は要らない。実 task-api router を in-process で使う）
   - 2 試行（`B-cos-direct`・`B-task-worker`）とも operator principal（`operator:ops-r1`）の relay が出典
     （`source.decision_id`・依頼文 sha256）付きで記録され、`decided_by`・`approved_by` は human、`executed_by`・`relayed_by` は operator。
   - D5 の照合: thread の operator の message と human-ops の external_operator の op 行（`celeris_event_ref`）は 1 対 1。
     op 行の無い operator の message を 1 件足すと、照合はその id を「対応しない操作」として返す。
   - human-ops: relay の op は人の能動時間に数えない。人の時間は人が記録した区間（direct の試行の承認 120 秒）だけ。
     人の op 行が無い task 経由の試行には人の時間の値を作らない。relay は試行ごとに 1 op（start・end の 2 行で 1 件）。
   - operator-usage: `ops-r2-session-map.py base` → `human-op list` の介入で `final` → 親と subagent の transcript を
     2 回ずつ取り込んでも message ごとに 1 行。relay の区間の message は intervene、他は observe。`summary` の合計は
     operator session の 1600 だけ。snapshot に紛れた CoS run の session の行（5000）は shared_unattributable で、どの試行の operator 消費にも入らない。
   - 3 つの台帳の鍵は交わらない。human-ops の op は人か external_operator だけで、試行に帰属した operator-usage の session は
     実験専用 session だけ。CoS run・worker run の消費（daemon の run）とは重ならない。

## 証拠（提出時の HEAD で実行）
- `CELERIS_USERNS_TESTS=1 cargo test -p celeris-campaign --test ops_r2_probe --test runtime_probe` → exit 0（3 passed、6 passed）
- `cargo test -p celeris-campaign --test ops_r2_probe -- --nocapture`（CELERIS_USERNS_TESTS なし）→ 3 passed。
  userns の 2 件は `skip: cos/worker isolation probe requires CELERIS_USERNS_TESTS=1 (or require)` と
  `skip: leak fail-closed probe requires CELERIS_USERNS_TESTS=1 (or require)` を出す
- `python3 -I -m unittest discover -s scripts/campaign -p test_check_ops_r1_leak.py` → OK
- `bash scripts/dev/test-affected.sh` → exit 0（2882 tests run: 2882 passed、9 skipped）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 範囲 check（`agent-docs/(adr|progress)/`・`crates/campaign/` の外の変更 0 件）→ 範囲外 0 件

## 未検証の範囲
- 実 LLM: CoS・planner・worker・外部 operator の実モデルでは走らせていない。CoS run・worker run は fixture の shell の子 process で、
  本物の daemon（task-dispatch・task-worker の起動経路、db_guard の namespace）を bwrap の中で起動していない。
  ADR が挙げた `crates/celeris/tests/ops_r2_isolation_probe.rs`（stub harness の測定 daemon）はこの unit の範囲（`crates/campaign/`）の外で作っていない。
- 実 runtime の userns 下: bwrap はこの host で動いたが、測定 daemon・worker の bwrap（二重の namespace）と実 Claude Code の起動は未確認。
- L1（別 uid）: `celeris-eval` uid は使っていない。fixture の private は同じ uid の持ち物なので、漏洩検査は same_uid の limit を残し canary で判定している。
  他 uid で拒否される確認は、人が `celeris-eval` と `EVAL_ROOT` を準備した後（close 段の手順）。
- CoS 消費: CoS chat run・worker run の消費は固定の行で模した。daemon の usage telemetry（runs・cos chat usage）からの読み出しは通していない。
- 人の時間の 0 件 → null: report 側（K4 証拠確定 task 01M669QAZE の cov-not-recorded）との照合はしていない。この試験は「人の op 行が無い試行に値を作らない」までを確かめた。

## 提案
- B の daemon を bwrap の中で起動する通し試験（stub harness、CoS run と worker run の両方）を、`crates/celeris/tests/` を範囲に含む unit か task で足す。
- probe-read の JSON（`celeris.runtime-probe/1`）を `check-ops-r1-leak.py` に渡す結合は、この試験の 1 件目が固定した。K2 の開始前チェックでもこの順（probe → 漏洩検査）で使える。
