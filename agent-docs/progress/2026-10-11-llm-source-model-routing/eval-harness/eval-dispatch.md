---
title: eval-dispatch（評価 run を Celeris の run 経路で起こす）まとめ
tasks: [01M4Q7C4X2QR3HYPTTQAG5GXQ9]
status: done
updated: 2026-10-12
---

# eval-dispatch まとめ

評価 run の受け口（予約枠 config・admission・`eval_trials` 表・HTTP API・dispatcher の起動段・採点・CLI・通し試験）を
ADR-2026-10-11-llm-source-model-routing-evaluation の付記（2026-10-12）どおりに実装した。
この締め leaf はコードを足していない。全体検査・clippy・文書検査と、受け入れ条件ごとの証拠を残す。

各 leaf の記録: `eval-harness/eval-dispatch/<leaf>.md`（eval-admit・trial-store・cfgwarn-fix・trial-api・trial-run・trial-score・cli-run・stub-e2e）。

## 受け入れ条件ごとの証拠

### 0. 全体検査と clippy が exit 0

- 条件: workspace の全試験（nextest + doc-test）が通る。clippy が警告ゼロ。
- 実行: `export TMPDIR=/local/x && bash scripts/dev/test-parallel.sh`（ログ: `artifacts/test-parallel.log`）
  - 結果: exit 0。`CELERIS_TEST_SUMMARY {"binaries":183,"nextest_binaries":172,"doc_binaries":11,"passed":5190,"failed":0,"ignored":14,"nextest_exit":0,"doctest_exit":0,"tmp_leftovers":0}`。`test-parallel: ok`。
- 実行: `cargo clippy --workspace -- -D warnings`（ログ: `artifacts/clippy.log`）
  - 結果: exit 0。警告・エラーなし。
- 補足（最初の試行の失敗と直し）: `TMPDIR=/local/x/close` で最初に流したとき、`task-api::browser_e2e` の 4 件が `timed out waiting for fixture readiness` で落ちた（exit 100）。原因は unix socket path が 108 byte を超えたこと。`test-parallel.sh` は TMPDIR の下に `celeris-test-parallel.XXXXXX/tmp/` を作るため、TMPDIR は `/local/x` まで短くする必要がある。`/local/x` にして再実行したら全件 pass。コードの変更はしていない。
- stub 通し試験（brief 契約 7）の単独実行: `cargo test -p celeris --test routing_eval_run` → `daemon_runs_two_stub_trials_with_hidden_checks_and_eval_records ... ok`（1 passed, exit 0）。

### 1. 進捗まとめがある

- 条件: このファイルに、受け入れ条件ごとの証拠・未解決事項・本番への移行が書かれている。
- 本ファイル（`agent-docs/progress/2026-10-11-llm-source-model-routing/eval-harness/eval-dispatch.md`）。

### 文書検査（brief の 3 本）

- `sh scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (183 files)`、exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → `check-doc-layout: ok`、exit 0。
- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0。

（参考）`python3 scripts/dev/check-architecture-map.py` は exit 1。検査対象は `docs/architecture-map.md` の 68 行目（試験の一時 dir の後片付け行）の `<workspace>/runs/<run_id>/tmp/`・`test-parallel.sh`・`check-test-tmp-leftovers.sh` の 3 件。同じ行は main（7b524e2f）にも既にあり、この task の変更による失敗ではない。上の 3 本の文書検査には含まれない。直すなら、表の該当行を検査が読める書き方に改める（別 WU の範囲なので、この締め leaf では触らなかった）。

### 0 の再検証（締め leaf の再実行、2026-10-12）

- 指示された check `mkdir -p /local/x/edc && TMPDIR=/local/x/edc bash scripts/dev/test-parallel.sh && cargo clippy ...` は exit 100 で不合格。
  `task-api::browser_e2e` の 4 件が `timed out waiting for fixture readiness`（60 s）。
  原因は TMPDIR の長さ。gate は `$TMPDIR/celeris-test-parallel.XXXXXX/tmp` を掘るため、`/local/x/edc` だと試験の credentiald の control.sock の path が 109 byte になり、unix socket の上限 108 byte を超える（bind が失敗し、serve が黙って戻る）。`/local/x` なら 104 byte で通る。
  同じ試験を単独（`cargo nextest run -p task-api -E 'binary(browser_e2e)'`）で `TMPDIR=/local/x/edc` で流すと 4/4 pass。失敗は負荷でなく path 長による。コードは変えていない。
- 直したあとの再実行（同じコミット、`export TMPDIR=/local/x`）:
  - `bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY {"binaries":183,"nextest_binaries":172,"doc_binaries":11,"passed":5190,"failed":0,"ignored":14,"nextest_exit":0,"doctest_exit":0,"tmp_leftovers":1}`。`test-parallel: ok`。`tmp_leftovers: 1` は gate が終了時に trap で消す警告（exit は 0）。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `sh scripts/dev/check-adr-numbers.sh` → ok (183 files)、exit 0。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok、exit 0。`sh scripts/dev/check-doc-links.sh` → ok、exit 0。
- 結論: 指示の check は TMPDIR の path が長すぎて、コードをどう直しても通らない（test-parallel が一時 dir を 1 段入れるため）。締めの成果は `TMPDIR=/local/x` で検証済み。check の TMPDIR を `/local/x` にするか、gate の一時 dir の入れ子をやめるかを計画側で決める必要がある（下の「提案」）。
- 3 回目の再検証（同じコミット、2026-10-12）: 長さの計算。gate の一時 dir（`celeris-test-parallel.XXXXXX/tmp`）と試験の tempdir・credentiald の `control.sock` の path は TMPDIR の外で約 97 byte。unix socket の上限 108 byte を超えないため、TMPDIR は 11 byte 以下でなければならない。`/local/x`（8 byte）は通り、`/local/x/edc`（12 byte）は必ず落ちる。
  - `export TMPDIR=/local/x && bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY {"binaries":183,"nextest_binaries":172,"doc_binaries":11,"passed":5190,"failed":0,"ignored":14,"nextest_exit":0,"doctest_exit":0,"nextest_secs":142.1,"doctest_secs":10.7}`。`test-parallel: ok`。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `check-adr-numbers.sh` → ok (183 files)、`check-doc-layout.sh` → ok、`check-doc-links.sh` → ok（いずれも exit 0）。
  - 指示の check（`TMPDIR=/local/x/edc`）は計画の問題として申告する（result.json の yield）。コードの変更は無い。
- 4 回目の再検証（run 01M4QKJ75R3DGP0K1ZCSCSZEQN、同じコミット 6e6243e3、2026-10-12）: 全体の検査を `TMPDIR=/local/x` で流し直した。
  - `bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY {"binaries":183,"nextest_binaries":172,"doc_binaries":11,"passed":5190,"failed":0,"ignored":14,"nextest_exit":0,"doctest_exit":0,"nextest_secs":271.0,"doctest_secs":9.5,"tmp_leftovers":0}`。`test-parallel: ok`。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `check-adr-numbers.sh` → ok (183 files)、`check-doc-layout.sh` → ok、`check-doc-links.sh` → ok（いずれも exit 0）。
  - 前回 run の check（`/local/x/edc`）は上の理由で通らない。この run の検査は `/local/x` で流した。

## 受け口の要点（実装と ADR 付記の対応）

- 予約枠: `[routing_eval] reserved_runs`（既定 0 = 評価 run を起こさない。未知欄は設定エラー）。`eval_admission` 純関数の判断順は reserved_runs = 0 → 予約枠満杯 → account 同時実行満杯 → quota 上限。
- 表: `eval_trials`（migration `0072_eval_trials.sql`、`SCHEMA_VERSION = 72`）。`insert_eval_trials` は同一 `(manifest_sha, problem_id, trial_no, source_id, model_id, harness_id)` を冪等に返す。
- API: `POST/GET /api/v1/routing-eval/trials`、`POST /api/v1/routing-eval/trials/{trial_id}/cancel`。GET の応答に `check_cmd`・objective は含めない。
- 起動: dispatcher の `eval_trials` 段は通常 dispatch の後。harness 固定・新 worktree（`<data>/eval/<trial_id>/repo`）・新 session・RunRequest は objective のみ（acceptance 空）。`runs.role = "eval"`、`route_decisions.stage = "eval"`、harness version を記録。
- 採点: worker 終了後に scoring へ移し、hidden check を timeout 付きで worktree 上に流し、`route_outcomes` を書いて worktree を消す。daemon 再起動で run が消えた running は理由付き error。
- CLI: `celeris-routing-eval run --api --manifest --candidates [--token-file] [--repo] [--wait] [--poll-interval-ms] [--out]`。summary に check command は出さない。

## 未解決事項

- 本番の評価 run は起きない（`reserved_runs` の既定 0）。有効化は人が行う（下の「本番への移行」）。
- `check-architecture-map.py` の 68 行目（上記）は main から続く既存の不備。
- 評価の本当の実行（LLM を呼ぶ評価）は走らせていない。試験は stub harness のみ。本番 harness での試行の初回は、人が小さな batch で確かめること。

## 本番への移行（人が行う）

1. 本番 DB: 起動時に migration `0072_eval_trials.sql` が自動で当たる（events は追記のみ、既存行は触らない）。新しい daemon（この task の commit を含む release）を通常どおり昇格する。
2. 本番 config: 評価 run を有効にするなら、本番 daemon の config（`~/.config/celeris` 配下で運用している設定ファイル）に次の節を足す。既定は 0 のまま。
   ```toml
   [routing_eval]
   reserved_runs = 1
   ```
   値は評価専用の同時実行枠。本番の全体並列度には数えない。最初は 1 にして、account の quota に余裕がある時間帯に、少数の試行（`celeris-routing-eval run ... --wait`）で動作を確かめる。
3. 確認: `GET /api/v1/routing-eval/trials` で試行の状態を見る。`route_decisions` の `stage = 'eval'` の行と、`runs` の `role = 'eval'` の行が増えることを確認する。
4. 本番の config・systemd・DB への書き込みは、この run からは行っていない。

## 提案

- `check-architecture-map.py` の対象行（68 行目）を、検査が読める表記（path を backtick で単独に書く）に直す小さな WU を 1 つ足す。
- 試験の TMPDIR の注意（`test-parallel.sh` 経由では TMPDIR を `/local/x` 程度まで短くする）を、`agent-docs/guides/testing.md` に 1 行足す。
- 締めの check の TMPDIR を `/local/x/edc` から `/local/x` に直す（path 長の上限は 108 byte。`/local/x/edc` は 5 byte 超える）。または `test-parallel.sh` の一時 dir の入れ子を 1 段減らす。どちらにするかは計画側で決める。
- `browser_e2e` は control.sock の path が上限を超えると黙って serve が戻り、60 s の待ちで落ちる。path 長を試験の前段で検査してエラーにすると、原因が次から分かる（小さな WU）。
