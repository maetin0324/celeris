---
title: "[監査修正6b] 通常経路の通し受け入れ試験（正常・誤成果物・未採点・通信障害・再起動・budget 停止）"
tasks: [01M53CXMP758ZAXKXYZVBGN8A4]
status: done
updated: 2026-10-16
completed: 2026-10-16
---

# 監査修正 6b: 通し受け入れ試験

## 再確認
着手時の main は 173988bb（修正 1・2・3・4・5・6a の各ブランチは全て取り込み済み。`git merge-base --is-ancestor` で確認し、
fix4 の closeout を含む main を merge した）。監査基準の 0e73be29 以降の修正 commit が揃っていることを確かめてから試験を書いた。

## 試験の構成（`crates/celeris/tests/campaign_acceptance_e2e.rs`、7 件）
- 一時 daemon の task-api（実 HTTP・temp DB・loopback）を、campaign の `CelerisExecutor` が実 HTTP で駆動する（raw TCP の `Transport`）。
  識別・routing 固定・冪等キーは `POST /tasks` が受けた値。`task_campaigns` などへの直挿入・手動 DB 補完は無い。
- mock にしたのは実行内部だけ: dispatcher 相当の状態遷移（`apply_transition`）、worker/reviewer の run イベントと成果物の書き込み、A arm の harness、
  grader の評価 command（成果物の `result.txt` だけを見る疑似 cargo）。独立 grader は実物の `grade_out`、集計と export は実物の `checker`/`export`。
- 時計は注入（`Clock::sleep` が mock の実行内部を 1 歩進める）。実時間を待たず、負荷も掛けない。

## 場面と結果
| 場面 | 試験 | 結果 |
|---|---|---|
| 正常（POST→子 task→成果物→grader→export、決定ログの結合） | `normal_path_post_tasks_to_independent_grade_to_final_export` | ok |
| 誤成果物（executor passed でも grader 不合格は成功にしない） | `wrong_artifact_completed_run_is_not_a_success` | ok |
| 未採点（成功にも分母除外にもせず最終版を作らない／揃えると出る） | `ungraded_trials_block_the_final_export_until_graded` | ok |
| 通信障害（POST 応答喪失・GET 断で二重投入しない） | `communication_failures_keep_tracking_and_never_double_submit` | ok |
| daemon 再起動（同 port・同 DB で API 再起動、同じ task を追う） | `daemon_restart_resumes_the_same_tasks_without_resubmitting` | ok |
| budget 停止（実 API cancel、停止後は投入しない、分母は残る） | `budget_stop_cancels_through_the_api_and_stops_further_submissions` | ok（下記の不備を直して合格） |
| 全体 config 逆 mode での arm 維持 | `arm_pin_is_set_on_the_task_through_the_api_not_by_the_global_config` + 既存 `root_routing_pin_is_inherited_by_child_grandchild_and_retry_whatever_the_config`（両方向） | ok |

## 指摘 1〜6 の受け入れ条件と試験
| 指摘 | 受け入れ条件 | 試験 | 区分 |
|---|---|---|---|
| 1 grader 結合 | executor passed + 不合格 grade は最終成功 0／未採点は成功でも除外でもない／別 case・split・campaign・古い hash・重複を拒否 | 通し: `wrong_artifact_…`・`ungraded_…`。単体: `campaign/tests/grade_binding.rs` | mock 検証済み（grade_out・checker・export は実物） |
| 2 識別・接続 | 通常の POST /tasks の識別が子・孫・再試行・planner/reviewer・CoS・Jev・介入まで結合し、`verify_connected` が通る | 通し: `normal_path_…`（B/C の木の全 run が campaign に結合、token が一致）。既存: `campaign_wiring_e2e.rs`・`campaign_connected.rs` | mock 検証済み（run は mock、API は実 HTTP） |
| 3 routing 継承・実測照合 | 全体 config が逆でも arm の mode/根拠表/online_update が root・子・孫・再試行で維持。実測の別 model/harness/fallback は違反 | 通し: `normal_path_…`（子・孫の継承を API 経由で確認）・`arm_pin_…`。単体: `lane_assist.rs`（両方向）・`routing_pin.rs`・`harness_exec.rs` | mock 検証済み。dispatcher の解決自体は単体（通し試験は daemon に dispatcher を載せていない） |
| 4 通信障害・cancel・予算 | 応答喪失・通信断で追跡を失わず二重投入しない／cancel 失敗は隠さず／予約と実消費の整合／再起動で同じ task | 通し: `communication_failures_…`・`daemon_restart_…`・`budget_stop_…`。単体: `ledger_async.rs`・`budget_reserve.rs`・`watch_driver.rs`・`celeris_exec.rs` | mock 検証済み（HTTP は実）。cancel 失敗の実注入は単体のみ |
| 5 final.json/case/preflight | placeholder・alias・routing 欠落の拒否、K case の grade-case 固定、preflight が有料実行 0 件で止める | 既存: `final_resolved.rs`・`k_cases_grade.rs`・`grade_case_gen.rs`・`preflight.rs`（本タスクでは通し化していない） | mock 検証済み |
| 6a 入力隔離・実験証拠 | 試行 KB を workspace 内に接続、通常 KB・grader 私有値が試行の木に無い、実験証拠を評価側の取得記録と照合 | 既存: `trial_inputs.rs`・`grader_independent.rs`（通し試験の試行は `kb/` を持つ seed から作られる） | mock 検証済み |

## 発見した不備と対処（差し戻しに相当）
- **修正 4（budget-reserve）と checker の不整合**（本タスクで直した）: 予約 gate が `Exhausted`（in-flight 無しで残り < per_case 予約）で
  `budget_stop` を記録するのに、`checker` は確定消費 >= max_tokens しか正当な停止と認めず、正当な予約停止の記録を export が
  「cancelled for budget although budget remained」で拒否した。`checker::budget_gate_closed`（残り < per_case）に揃えた
  （3 行の局所修正。ADR 2026-10-16-campaign-budget-reservation に付記）。通し試験 `budget_stop_…` が回帰固定。
  修正 4 の単体試験は gate 側だけを見ており、checker との結合が漏れていた。
- 通し試験が使う `evidence_kind` は executor が simulated でないため `real` と出る（実行内部は mock）。この試験の数値を実性能として扱わないこと。

## 区分（項目ごと）
- 実装済み: 本タスクは試験の追加と checker の局所修正のみ（commit は本ブランチ）。
- mock 検証済み: 上の 7 件（実 HTTP の task-api + 実 checker/export/grade_out、実行内部のみ mock）。
- 実経路検証済み: なし。実 LLM・実 cluster job・本番 daemon の再起動・実 dispatcher での routing 解決は未検証。
- 承認待ち: 大規模本測定の開始・再実行・予算増額（承認されていない。予算は token_basis=all、全体 300 万/case 30 万/arm 130 万、並列 1 のまま）。
  実経路の小規模確認（1 case・並列 1 の `watch`）は人の承認後。

## 未解決・提案
- daemon に dispatcher を載せた通し（実 dispatcher の routing 解決・run イベントの自動生成）は未実施。必要なら承認後の小規模実経路で。
- submit_unknown の人による解消 CLI、task への token 上限受け渡しは fix 4 の提案のまま。
- identity 側に残る未適用 patch の取り込み／破棄理由は本タスクでは確認していない（要確認）。

## 証拠コマンド
- `cargo test -p celeris --test campaign_acceptance_e2e` → 7 passed / 0 failed。
- `bash scripts/dev/test-parallel.sh` → exit 0（nextest passed 5595, failed 0, ignored 15）。
- `cargo clippy --workspace -- -D warnings`（および `--all-targets`）→ exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → ok（fix 3 の progress front matter も直した）。
