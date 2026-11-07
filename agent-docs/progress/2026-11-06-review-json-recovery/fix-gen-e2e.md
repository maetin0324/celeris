---
title: fix-gen-e2e — script_generation_e2e の lane_rules compare 待ち（300 秒）の落ちの原因
tasks: [01M6S325NYYJS9YWNHDH757E08]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# fix-gen-e2e — script_generation_e2e の lane_rules compare 待ち（300 秒）の落ちの原因

完了日: 2026-11-06

## 結論
落ちた原因は試験の競合ではなく、検査の間に host の `/tmp`（tmpfs 63G、全 run で共有）が満杯になったこと。
試験用の一時 DB の daemon の `disk_watch` は host の `/`・`/local`・`/tmp`（`crates/task-dispatch/src/disk_watch.rs` の
`DEFAULT_PATHS`）を測り、使用率が `critical_pct`（既定 95%）以上だと新しい coding run を起こさない（ADR 2026-10-07-build-tmp-hygiene 付記 A4）。
そのため `stage_a_lane_rules_events` が入れた task は dispatch されず、`ScriptCompareRecorded` が出ないまま 300 秒の保険に当たった。
generalize の差分（task-worker の result_location の一般化）とは関係しない。試験・本体のコードは変えていない
（直す対象のコードの不具合が無いため。skip・緩和・待ちの延長もしていない）。

`/tmp` に空きが戻った状態で、integrate-core と同じ基準（`git merge-base HEAD main`）で `test-affected.sh` を流すと、
script_generation_e2e を含む 5559 件がすべて通った。

## 根拠
integrate-core の検査ログ（workspace の `integration-checks/integrate-core/` の 1 回目 `…-1.log`・2 回目 `…-3.log`）から確かめた。

- 2 回目（`…-3.log`）で落ちたのは script_generation_e2e だけではない。56 件（`Summary … 5559 tests run: 5503 passed, 56 failed`）が落ち、
  すべて本物の daemon や dispatcher で task を流す試験だった（`long_run_soak_*`「routing_decided missing」、
  `script_compare_lane_rules`・`script_compare_route_priority`・`script_compare_execution_gate`・`instance_handoff`・
  `root_done_summary`・`variant_propagation`・`cap_probe_real_daemon`・lib の `a_publickey_cluster_…_phase_81`「connector に 0 回」など）。
  共通点は「task が dispatch されない」。
- 同じログに `WARN task_dispatch::disk_watch: disk watch: critical; new coding runs are held (ADR 2026-10-07-build-tmp-hygiene A4)` が
  20:01Z に複数の daemon から出ている。
- 同じログに `No space left on device`（`Io(Os { code: 28, kind: StorageFull })`、dagbench の tar 展開の `Cannot write: No space left on device`）が
  約 2000 行ある。`test-affected.sh` は試験の TMPDIR を `/tmp/celeris-test-affected.XXXXXX/tmp` に置くので、これは `/tmp` が満杯だったことを示す。
- 1 回目（`…-1.log`）は nextest が走り始めた後、`[celeris] timed out after 600s; killed the process group` で打ち切られた（build に 6 分 32 秒）。
  SIGKILL なので `test-affected.sh` の EXIT trap（`rm -rf "$tmproot"`）が走らず、1 回目の `/tmp/celeris-test-affected.urZ2UO`（4.2G）が今も残っている。
  1 回目のログの script_generation_e2e の失敗（`FAIL [ 301.794s]`、1188 行の panic）は、この打ち切りの後の再実行（同じログの 2 本目の nextest）のもので、
  その再実行でも 19:51〜19:52Z に `disk watch: critical` が出ており、`No space left on device` が 150 行ある。2 回とも同じ原因で落ちた。
- この run の開始時点で `/tmp/celeris-test-affected.*` が 27 個・合計 26G 残っていた（11-06 14:37〜20:43 の各 run の分）。
  打ち切られた検査の残りが溜まり、並行する他の run の試験（dagbench の展開など）と合わせて tmpfs を使い切ったと考える
  （20:01Z の各 dir の大きさの内訳は未確認）。
- この run で `/tmp` が 43%（空き 36G）のときに試験だけを流すと 9.4 秒で通った。`merge-base HEAD main` 時点の挙動と比べる必要のある差分も無い
  （HEAD と `$CELERIS_WU_BASE` の差は 0、`git merge-base HEAD main` からの差分は task-worker・task-dispatch の result_location の呼び出しと ADR・進捗だけで、
  script 層・lane_rules の compare・disk_watch に触れていない）。

## 変更
commit（件名。基準は `$CELERIS_WU_BASE`）:
- progress: review-json-recovery fix-gen-e2e

コードの変更は無い（この進捗ファイルだけ）。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p celeris --test script_generation_e2e` → 1 passed（9.4 秒）
- `CELERIS_WU_BASE=$(git merge-base HEAD main) bash scripts/dev/test-affected.sh` → exit 0、`5559 tests run: 5559 passed (19 slow), 22 skipped`、
  `script bundle SKIP: 0 件`（bundle は control-ts の source から作った）。前後の `/tmp` は 43%。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-affected.sh`（`$CELERIS_WU_BASE` 基準）→ exit 0（この WU の差分は進捗ファイルだけなので文書検査のみ）

## 未解決
- integrate-core を再実行するときに `/tmp` が再び 95% を超えていれば、同じ 56 件が同じ形で落ちる。再実行の前に `df -h /tmp` を確かめる。
- 残っている `/tmp/celeris-test-affected.*`（26G）は他の task の run のものを含むので、この run では消していない。

## 提案
- 検査の打ち切り（SIGKILL）で `test-affected.sh` の `/tmp/celeris-test-affected.*` が残る。celeris の check runner が打ち切りの後に
  その run の tmproot を消す（例: tmproot を run ごとの既知の path にして runner が片付ける）か、`test-affected.sh` が起動時に
  持ち主の process が居ない古い dir を消す仕組みが要る。
- integrate-core の build が 6 分半かかり、600 秒の check の上限に近い。上限の見直しか、build を check の前に済ませる工夫が要る。
- 試験用の daemon の `disk_watch` が host の `/`・`/local`・`/tmp` を測るので、host の容量で 56 件が一斉に落ち、原因が「not seen after 300s」
  などに見えにくい。試験の daemon では待ちの失敗 message に `disk watch: critical` の有無を出すと、次に同じ事が起きたとき原因がすぐ分かる。
