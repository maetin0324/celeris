---
title: ops_r1_pipeline 結合試験（pipe-e2e）
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
status: done
updated: 2026-10-30
---
# ops_r1_pipeline 結合試験（pipe-e2e）

完了日: 2026-10-30

## 変更
- `crates/celeris/tests/ops_r1_pipeline.rs` を追加（2 試験）。A は stub の `ops-r1-a-raw/1`、B は stub の `ops-r1-intake/1` と、
  `ops_r1/trace_fixture.rs`（本物の store で書いた DB の写し）から実 binary の `ops-r1-trace`・`ops-r1-attribution` で出した記録。
  そこから `ops-r1-assemble → stage-grader-inputs → grade-out → check → export → human-op report → final-accounting →
  python3 scripts/campaign/check-out-consistency.py` を実 binary で走らせる。human-op の記録は `human-op record --condition` で 2 件書く。
  celeris-campaign の binary は試験の中で `cargo build -p celeris-campaign --bin celeris-campaign` して同じ target から使う。
- 確かめること: 全段 exit 0。arm ごとに trials.jsonl の outcome・reason・passed、grades.jsonl の final_pass、check の再計算、
  export の表、human-op report の arm 表と試行行、final-accounting の cell（ran・trial_tokens）、check-out-consistency.py の
  counts が一致し、分母（計画 cell）は arm ごとに 1。B の trace の報告の有無と outcome、attribution の task run の行と試行の
  input tokens・observed、CoS の chat run の行数と final-accounting の共有行の数も照合する。
- 場面: (1) A は打ち切り（timeout）、B は報告まで届き採点に通る。同じ raw から 2 回通し、pass dir の全 file（trace・attribution・
  out・records.jsonl・report・final-accounting・check）が byte 一致。check-out-consistency.py の出力だけは照合した file の
  絶対 path を含むので、pass dir の path を置き換えて比べる。(2) A は exit 0 で採点に通り、B は done より前に打ち切られる（timeout）。
- `scripts/campaign/check-out-consistency.py` を直した: human-op report の arm 表は記録の arm_label（`ops-r1-A`・`A0` など）で
  引かれるが、script は config id（`A`）で引いていたため「human-op report has no arm」で exit 1 になった。report の試行行の
  `arm`→`arm_labels[0]` で label を引くようにした（label の無い旧 fixture はそのまま config id で引く）。
  `test_check_out_consistency.py` に label で引く場合の試験を足した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p celeris --test ops_r1_pipeline` → exit 0、2 passed。`cargo nextest run -p celeris --test ops_r1_pipeline` → 2 passed。
- `python3 scripts/campaign/test_check_out_consistency.py` → 16 tests OK。
- `bash scripts/dev/test-affected.sh`（対象 celeris・celerisctl）→ exit 100、833 tests run: 832 passed、1 failed。
  落ちたのは `celeris::credential_wait_notify wait_notify_register_verify_and_resume_once_through_the_fake_webhook` で、
  単独の再実行でも落ちた（1 回目は 456 行、2 回目は 433 行と落ちる assert が変わる）。この試験は今回の変更（新しい試験 file と
  scripts/campaign の 2 file）を読まない。base で落ちるかは未確認。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p celeris --test ops_r1_pipeline -- -D warnings` → exit 0。
  `cargo fmt --all -- --check` → exit 0。

## 未解決事項
- `credential_wait_notify` の失敗（上記）。pipe-close か別 task で確認する。
- grader の失敗分類が出る理由（report.md だけの成果物は model_failure）は合成資料の採点の仕様どおり。

## 提案
- なし。
