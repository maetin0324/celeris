---
task: 01M7XQ9QWK58ESMW42Q1TGT3WZ
work_unit: blind-eval-input
status: done
completed: 2026-11-23
---
# 子 6 WU blind-eval-input: 評価器の入力の作り直し（子の報告の本文・出力の形・answer key の除去）

人の判断（2026-11-23、案 A）による再計画の WU。基準は `$CELERIS_WU_BASE`（WU の base）。検査は提出時の HEAD で実行した。

経緯: 1 回目の run は実装・試験・進捗を済ませたが、計画の範囲 check が落ちた（範囲外 `crates/task-worker/src/change_rules/review.rs`）。
評価器の prompt を組むのは `change_rules::review`（`parent_checkpoint_section`・`result_section`、呼び出しは `claude_code/prompt.rs`）で、
`protocol/parent_checkpoint.rs` は wire の型だけを持つ。run は plan_issue を返し、人が承認した計画 v6 が範囲に `change_rules/review` を足した。
2 回目の run（範囲の訂正）は branch の commit をそのまま使い、コードを変えずに検査を提出時の HEAD でやり直した。

## 結論

- 本番の request の組み立て（`task_dispatch::review::parent_evaluation::request`）が、子ごとに checkpoint の時点（判断の行の `created_at`）以前の
  `WorkerFinished.outcome`（worker の結果と review の報告）を `ParentCheckpointChild.reports` に入れる。選ぶのは純関数
  `task_core::parent_checkpoint::child_reports` の 1 つ（1 件 2,000 byte・子ごとに新しい順に 4 件・合計 16,000 byte。planner の run と時刻の読めない event は入れない）。
  読み口は既存の `SqliteStore::event_rows_for` だけで、store・migration・events の差分は 0。
- prompt（`change_rules::review`）は各子の報告を引用（`> `）で出し、Result 節に `celeris.parent-evaluation/1` の全欄と verdict・next・axes の定義、
  参照の形と支持に数える条件、`check_evaluation` の直し方を書く。前の prompt は commit の参照を支持に数えると書いていたが、本番の `facts` は commit を解決しないので、
  「commit は支持に数えない」に直した。
- ADR の D8.1 の表から answer key の列を消し、E-mid と D8.3 と背景の期待の値を本文から消した。比べる値は子 0 の `answer_key.json` にだけあり、試験と `python3 -I` の側だけが読むと書いた。
  ADR に D3.1.1（子の報告の本文）を足した（本文をその場で直した。末尾の付記は足していない）。
- E-mid・D-done の request・prompt を本番の選択関数で作り、`crates/task-api/tests/fixtures/parent_checkpoint_live/` に置いた。選んだ報告は
  `fixtures/parent_checkpoint_shadow/E-mid/reports.json`・`D-done/reports.json` に置いた（`slice.json` は子 0 の manifest と byte で一致させるため変えない）。

## 受け入れ条件と証拠（提出時の HEAD で実行。2 回目の run で再実行）

- 条件 0（E-mid・D-done の request に子の報告の本文、`parent_checkpoint_blind_` が pass、変更前に落ちた試験）:
  - `cargo nextest run -p task-api -p task-core -p task-worker -p task-dispatch --run-ignored all -E 'test(/parent_checkpoint_|project_controller_checkpoint_|parent_evaluation_/)'`
    → 73 tests run: 73 passed。内訳は `parent_checkpoint_blind_` 5 件（task-api 4 件・task-core の `parent_checkpoint_blind_child_reports_cutoff_and_caps` 1 件）、
    `parent_checkpoint_live_` 2 件（`#[ignore]` の sample の検査を含む）、その他の `parent_checkpoint_` 43 件、`parent_evaluation_` 8 件、`project_controller_checkpoint_` 15 件。
  - 入力の大きさ: E-mid は報告 17 件・5,490 byte、request 12,548 byte、prompt 15,326 byte。D-done は報告 17 件・5,600 byte、request 12,655 byte、prompt 15,431 byte。
    D の報告に D#1446（worker の結果、19:55:39Z）が入り、D-done には D#1566（review の報告、20:39:25Z、cutoff 20:39:40Z の前）も入る。
- 条件 1（request・prompt・slice・ADR に answer key の印と期待の値が無い）: `parent_checkpoint_blind_no_answer_key_or_future` が、request・prompt・reports.json・slice.json に
  answer key の印 3 種・全時点の理由の文・verdict と next の中黒の組 16 通りが無いこと、ADR とこの進捗に answer key の列名と中黒の組が無いこと、request・prompt・reports に
  cutoff より後の event の時刻と本文（40 byte 超）が無いことを確かめる。
- 条件 2（prompt に出力の欄と axes の定義、既存の試験）: `parent_checkpoint_blind_prompt_has_output_fields_and_axes` が欄 18 個と axes の値・定義の文を確かめ、prompt の例の JSON を
  `check_evaluation` に通して `Checked` を確かめる。既存の `parent_checkpoint_`・`project_controller_checkpoint_` の試験は上の nextest と下の test-affected で pass した。
- 条件 3（範囲 check、`change_rules/review` を足した版）: 計画の範囲 check の正規表現に `crates/task-worker/src/change_rules/review(\.rs$|/)` を足した command を
  `sh "$CELERIS_WU_SCOPE_PATHS"` の path に当てた → exit 0（範囲外の path 0 件）。`review/parent_evaluation.rs` の差分は request の組み立て
  （`Subject`・`Running`・`launch_all`・`running_by_root`・`request`）の hunk だけで、`write_record` に触れていない。

### 変更前に落ちた試験

base の E-mid の fixture（`E-mid.request.json`・`E-mid.prompt.md`）と base の ADR に一時的に戻して `cargo test -p task-api --test parent_checkpoint_live parent_checkpoint_blind` を走らせた:

- `parent_checkpoint_blind_request_has_d1446_body` FAILED: 「E-mid の request の D に D#1446 の本文が無い」
- `parent_checkpoint_blind_prompt_has_output_fields_and_axes` FAILED: 「E-mid.prompt.md に celeris.parent-evaluation/1 が無い」
- `parent_checkpoint_blind_no_answer_key_or_future` FAILED: 「ADR に answer key の列」
- `parent_checkpoint_blind_reports_capped` は新しい組み立てを使うので base の fixture でも pass（上限の試験で、変更前の失敗の対象ではない）。
  `parent_checkpoint_blind_child_reports_cutoff_and_caps` は `child_reports` が base に無いので変更前は compile できない。

### 前の sample（`E-mid.evaluation.json`）

`parent_checkpoint_live_sample_checks`（`#[ignore]`）は新しい request でも pass した（status Checked、違反 0、slice の外の参照 0）。目的の規則（通らなければ消す）に従い、
sample と試験は残した。sample は報告の本文の無い入力で書かれたもので、盲検の評価（WU blind-eval-runs）が置き換える。

### repo の検査（提出時の HEAD で実行。2 回目の run で再実行）

- `bash scripts/dev/test-affected.sh` → exit 0。nextest 8,764 passed・32 skipped、web の unit 745 passed（105 file）、test-ts-verify 20 passed、repo_health_gate all checks passed。
- `cargo clippy --workspace -- -D warnings` → exit 0（vendor の deno_core の既存の warning 1 件だけ）。
- `cargo fmt --all -- --check` → exit 0。

## 試験の追加の理由（守る不変条件と、既存の試験で検出できない理由）

- 不変条件: 評価器の入力に子の報告の本文が checkpoint の時点まで入り、answer key の印と時点より後の記録が入らない。上限を超えない。
- 既存の `parent_checkpoint_live_request_renders` は報告の欄を持たない request を fixture と比べるだけで、本文が無いこと（D#1446 を読めないこと）と prompt に出力の形が無いことを
  検出しなかった。task-worker の `parent_evaluation_wire_` は合成の request で wire だけを見る。

## LoC（`git diff --numstat $CELERIS_WU_BASE`）

- production の Rust: +213 / −42（task-core `parent_checkpoint.rs` +109、task-dispatch `review/parent_evaluation.rs` +53/−29、task-worker `change_rules/review.rs` +43/−10、
  `protocol/parent_checkpoint.rs` +8/−3）。純増 171 行の理由: 報告を選ぶ純関数と型（上限 3 つ）と、prompt の欄・axes の定義の文。新しい表・event・migration・Manager・trait・状態機械は無い。
- test の Rust: +423 / −77（task-api `parent_checkpoint_live.rs` +351/−77、task-core の試験 +65、task-worker の試験 +7）。
- fixture: E-mid・D-done の request・prompt と reports.json（生成物）。文書: ADR +53/−35、`docs/protocol/worker-protocol.md` +5/−1、schema +47。

## 回帰条件

- `parent_checkpoint` の無い review は request・prompt とも変わらない（`reports` は空なら省略。既存の `parent_evaluation_wire_` の base との一致の試験が pass）。
- strategy の無い task・controller off・shadow で reviewer run が増えない（起動の条件は変えていない。`launch_all` の分岐は不変）。

## 未解決・未確認

- 実 LLM の盲検の評価はまだ無い（WU blind-eval-runs が本番の reviewer run の起動口で流す）。
- 本番の判断の行の `created_at` と event の `ts` はどちらも RFC 3339 として比べる。形式の違う行は報告に入らない（fail closed）。本番の DB の値では確かめていない。
- replay の `outcome` は子 0 が 200 字で切った本文で、本番の `WorkerFinished.outcome` は 2,000 byte で切る。本文の長さの違いが評価に効くかは未確認。

## 提案

- 評価器の reviewer run は読み取り専用の DB と workspace を読めるので、`agent-docs/progress/` の他の進捗 file や子 0 の `replay/` を読めば answer key に届く。
  盲検の評価の run は、run の道具の記録で `replay/answer_key.json` と子 6 の進捗の path を読んでいないことを確かめる（blind-eval-runs の check に置く）。
