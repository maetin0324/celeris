---
title: 健全性v2 R5 adjust-worker — worker 結合試験の束ね
status: blocked
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
updated: 2026-11-11
---
# 健全性v2 R5 adjust-worker — worker 結合試験の束ね

基準は `$CELERIS_WU_BASE`。R2・R3 の main 取り込み後に実施。R4 は含まず、R4 後の調整は R6 が行う。

## 実装前 brief
1. 根本原因: 独立 test target ごとの build/link が必要になる。
2. 責務 owner: `crates/task-worker/tests/` の target 構成。architecture-map の worker と Browser の関連行。
3. 既存の再利用先: 既存試験の本体、inventory の capability・fixture 分類、cases.py。
4. 選択肢: 全件維持と ADR の束ね。隔離が必要な target を維持し、同じ capability・fixture の対象を束ねる。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 移動する6 target の旧 root。試験は削除しない。
7. 不変条件: case 数・検出能力・userns gate・ssh group・実 browser の独立 process を維持。cases.py と束の同一 process 実行で確認する。

## 変更
- `phase 1: bundle task-worker integration test targets`。
- `browser_eval_surface_suite`: browser_control_gate_wire・browser_live_cross・browser_loopback_prod_build・browser_prod_admission・browser_shared_cdp_events。
- `browser_eval_surface_browser_action_recovery_py_suite`: browser_action_recovery。fixture が違うため他の束に混ぜず、Python fixture の include に `../` を足す。Python file 自体は動かさない。
- browser_restore_deliver は実 browser・userns を使うため keep。reap_finished_children は waitpid(-1) が同じ process の子を回収するため keep。その結果、相手の無くなった ssh_cluster_manual も keep。inventory の分類・理由・summary を更新した。ADR の更新は write set 外なので基準版に戻し、実装との差はこの進捗に記録する。検査器と base の走査結果は変えていない。
- browser_live_cross は偽 Chrome・偽 SessionBackend の試験で実 browser・userns は使わない。userns_gate を使う全 target と ssh_localhost は元の path のまま。nextest の binary_id filter は変更不要。
- source 内は変更しない。試験関数は削除・追加せず、契約・assertion は全て維持するため限定 mutation は不要。

## 前回の証拠（再試行前）
基準は `$CELERIS_WU_BASE`。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf adjust-worker agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0。6 target が移動、keep/r6 266 target が元の path、error 0。
- 同じ script の `--check` → exit 0。352 target、18束、error 0。検査器は未変更。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-worker --base "$CELERIS_WU_BASE"` → exit 0。試験関数1292→1292、表1106→1106、名前で追った移動31、不変条件582件（31 file）、removals 0。
- `cargo metadata --no-deps --format-version 1` → exit 0。task-worker の kind=test target は21→17。fixture の include path 以外は6 file の本文が base と完全一致することも確認（exit 0）。
- `cargo test --workspace -p task-worker --test browser_eval_surface_suite --test browser_eval_surface_browser_action_recovery_py_suite` → exit 0。workspace の同名束も対象になる指定で、task-worker の2束は30件・1件すべて合格。他 package の同名束も7件・22件・19件合格。全体試験は走らせていない。
- `cargo nextest run --workspace -E 'binary_id(task-worker::browser_eval_surface_suite) | binary_id(task-worker::browser_eval_surface_browser_action_recovery_py_suite)'` → exit 0。2 binary、31件すべて合格、skip 0（他312 binary は filter で実行対象外）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `git diff --name-only "$CELERIS_WU_BASE" | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0、write-set R5: ok。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 担当の移動6 file・main.rs 2 file・inventory・この進捗・sync-main 進捗のリンク修正のみ。src と nextest config は base から差分無し。
- `sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-adr-numbers.sh` → exit 0。
- `bash scripts/dev/test-affected.sh --list` → exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。
- `bash scripts/dev/test-affected.sh` → exit 1。開始時の health が `doc.links` で fail。nextest・doc test は開始されていない。
- `sh scripts/dev/check-doc-links.sh` → exit 1。同じ文書リンク切れ1件。


## 再試行の修正 brief
1. 根本原因: 前回は ADR の変更が WU の範囲外で、sync-main 進捗の引用文の相対リンクも誤っていた。
2. 責務 owner: ADR の差分はこの WU が撤去する。リンクの参照先は記載元の進捗で修正する。
3. 既存の再利用先: 基準版の ADR、既存の doc-links と範囲検査。
4. 選択肢: 検査の緩和はせず、ADR を基準版に戻しリンク先だけを修正する。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 範囲外の ADR 差分と誤った相対参照。
7. 不変条件: 検査器・baseline・試験本体を変更せず、同じ検査を通す。

## 再試行の証拠（提出時の HEAD で実行）
基準は `$CELERIS_WU_BASE`。
- `classify.py --verify-leaf adjust-worker` → exit 0、6 target 移動、keep/r6 266件維持、error 0。
- `classify.py --check` → exit 0、352 target・18束、error 0。
- `cases.py --crate task-worker --base "$CELERIS_WU_BASE"` → exit 0、試験関数1292→1292、表1106→1106、不変条件582、removals 0。
- `sh scripts/dev/check-doc-links.sh` → exit 0、check-doc-links: ok。sync-main.md の引用文中の相対リンクだけを修正した。
- 計画と同じ範囲検査 → exit 0。ADR は基準版と差分無し。
- B0 `slices-check.py --write-set R5` → exit 0、write-set R5: ok。
- `bash scripts/dev/test-affected.sh` → exit 100。health は pass（doc.links findings=0、fail_grade=0）。nextest は283.047秒、5872件中5869 passed・3 failed・22 skipped。doc test は未到達。
- `cargo nextest run --workspace -E 'package(=task-worker) & test(launch_notice_tests::)'` → exit 100。4件中1 passed・3 failed、同じ失敗を再現。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo test --workspace -p task-worker --test browser_eval_surface_suite --test browser_eval_surface_browser_action_recovery_py_suite` → exit 0。task-worker の2束は1件・30件すべて合格。同名束の他 package も7件・22件・19件合格。
- 修正 commit: `phase 2: repair adjust-worker scope and progress link`。

## 未解決事項
受け入れ条件1の test-affected は未合格。src 内の既存試験 launch_notice_tests の a_spawn_failure_keeps_the_notice・a_failed_notice_prevents_the_spawn・notice_is_called_before_the_process_is_spawned が codex の通知0回で失敗する。対象 file と codex.rs は WU 基準から差分無し。Codex fixture は既定の環境を使い、codex.rs は起動前に CODEX_HOME/rules へ規則を書き込む。一方 codex/tests.rs の stub_codex は CODEX_HOME を一時ディレクトリに隔離している。継承環境での書き込み失敗が疑われるが、run の戻り値を最初の試験が捨てるため詳細なエラーは未確認。src は Objective で変更禁止なので変更しない。plan_issue として src の fixture の調査・隔離修正を許可する別 leaf を求める。

## 提案
段 record の owner が、上記 keep の理由と6 target の移動を ADR の付記に反映する。
