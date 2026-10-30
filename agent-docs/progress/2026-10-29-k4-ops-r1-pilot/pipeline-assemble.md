---
title: ops-r1-assemble 入口と ops-r1 manifest の雛形（pipe-assemble）
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
status: done
updated: 2026-10-30
---
# ops-r1-assemble 入口と ops-r1 manifest の雛形（pipe-assemble）

完了日: 2026-10-30

## 変更
- commit 件名: `pipe-assemble: ops-r1-assemble 入口と ops-r1 manifest の雛形`
- `celeris-campaign ops-r1-assemble`（`crates/campaign/src/ops_r1_assemble.rs`）を追加。入力は `--manifest`・`--input-manifest`・
  `--a-raw`・`--a-artifact`・`--b-intake`・`--b-trace`・`--b-attribution`・`--b-artifact`・`--b-observed-versions`・
  `--b-workspace-sha256-initial`・lock の材料（`--cases-root`・`--grader-root`・`--grade-case-root`・`--measurement-id`）・
  `--out`（空 dir）・`--records-out`。runner と同じ形の out（campaign.json・input-manifest.json・run-meta.json〈executor
  `ops-r1-assemble`〉・trials.jsonl・ledger.jsonl・summary.json・lock.json・snapshot/snapshot.json・`workspaces/<arm>/<problem>/rep<n>/`）と
  decision-log/1 形の records.jsonl を作る。時刻・host・実行 path は出さない（A の observed_versions の claude_code.path は空）。
- summary.json は runner の `write_summary` を `runner::write_assembled_summary` 経由で使う（別実装にしない）。
- `contract.conditions.B.cos_model`（任意欄、無ければ直列化しない）を足した。A に書くと validate が拒否する。assemble は
  attribution の chat_runs 行の model がこの値と違う・欠測なら「condition deviation」で止め、out に trials.jsonl を書かない。
  cos_model が無い manifest は assemble が拒否する。
- `ops-r1-attribution/2` の chat_runs 行の model・harness を `chat_runs.resolved_config_json` の `$.model`・`$.harness` から
  埋めるようにした（schema 名は変えない。`docs/ops/ops-r1-trace-attribution.md` の表も直した）。
- 雛形 `docs/ops/ops-r1-manifest.template.json`。`REPLACE:` の欄は freeze が固定票から埋める（input の sha256、snapshot 3 欄、
  claude 実体の sha256、pricing_version、fixed.initial_info・success）。fixed は models・initial_info・success だけで、
  policy・harnesses・role_models は書かない。
- 両腕の初期 workspace hash が違えば assemble は止める（checker は両腕を snapshot の同じ seed hash と照合するため）。
- `--input-manifest` の sha256 欄が manifest の `input.sha256` と違えば止める。

## 設計 §4 からの差（要確認）
- B の試行の token_usage・tokens・cost は attribution の `source=runs` 行（planner・worker・reviewer）の和にした。CoS の
  chat_runs 行は records.jsonl に `role=cos`・`shared_overhead=true`（元の role は `cos_role`）の usage 行として書き、run 行は書かない。
  理由: final-accounting の recheck は「by_role（worker・planner・reviewer）の和 = 照合が合った cell の tokens の和」を要求し、
  CoS を試行の値に含めると `by_role tokens 22440 differ from the matched cells 24090` で exit 1 になった。CoS の消費は
  final-accounting の `shared_unattributable.cos`（state observed）に出る。設計 §4.2 の「attribution の行の和」とは違う。
- B の case_budget（cancelled）は、測定者の停止記録の入力が無いので「task run の tokens が per_case.max_tokens を超え、timeout でない」
  ときに付ける。
- B の wall: passed は report.at、failed・cancelled は outcome.at までの秒数（trial_timeout で頭打ち）。outcome が無い・done でも
  cutoff 前の報告が無いときは timeout で wall = trial_timeout_secs。
- snapshot.json の materials_sha256 は lock の固定値の写し（`materials_pin_verified: false`）。assemble は資料を再 hash しない。

## §6 の未確認点の結果（`tests/ops_r1_pipeline_assemble.rs` で確認）
- reconcile_set_fixed は routing の無い B を通す: fixed に models を宣言した manifest で check が ok（`ops_r1_pipeline_existing_stages_pass_on_the_assembled_out`）。
  set_fixed の run 照合は policy を宣言しなければ routing を見ず、executor が `ops-r1-assemble` なので `celeris_run` も false。
  `conditions.B.intake=cos_thread` を読んで外す変更は要らなかった。
- checker は `harness_exit`・`operator_stop`・`task_failed`・`task_cancelled` と timeout（wall=trial_timeout）を違反にしない
  （`ops_r1_pipeline_checker_accepts_ops_r1_reasons`、3 組とも check ok）。
- `scripts/campaign/check-out-consistency.py` は `run-meta.json` の executor を読まない（引数・読む file を確認。run-meta を開かない）。
  script と `test_check_out_consistency.py` は変えていない。全段での script の実行は pipe-e2e が行う。
- stage-grader-inputs は ops-r1 manifest で grade-case の隣の公開入力 tgz を写し戻す（同じ試験で消してから戻し、byte 一致）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 条件0・1・2: `cargo test -p celeris-campaign --test ops_r1_pipeline_assemble` → exit 0、5 passed。
  2 回の assemble で out の全 file と records.jsonl が byte 一致、stdout も一致。stage-grader-inputs・grade-out（ok true）・
  check（ok true）・final-accounting・export が exit 0。CoS model 逸脱で停止。旧 manifest と空でない out を拒否。
- 条件3: `bash scripts/dev/test-affected.sh` の結果は下に追記。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決事項
- B の CoS 消費を試行の値に含めるか（上の「設計 §4 からの差」）。含めるなら final-accounting に CoS 役割を足す変更が要る
  （旧系列の出力 bytes を変えないよう分岐が要る）。pipe-close か人の判断。
- B の `workspace_sha256_initial` と `--b-artifact` の取り方は runbook の担当（設計 §6 のまま）。

## 提案
- final-accounting の by_role に ops-r1 の CoS 役割（cos_project・cos_general）を足し、ops-r1 の試行の値を CoS 込みにするかを決める。
