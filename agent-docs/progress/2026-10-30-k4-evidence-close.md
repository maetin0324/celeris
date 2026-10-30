---
title: K4 追試の証跡確定と報告第 3 版（CoS 別掲・実 model・欠測・比較範囲）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 追試の証跡確定と報告第 3 版

完了日: 2026-10-30

状態: 全 WorkUnit 完了（最後の WU report は報告第 3 版とこの進捗ファイル）。LLM・daemon・実試行・本番配備はしていない。
入力の R（親 K4 task 01M62S8XWCZDCY2ZBVQ35176MH の artifacts/ops-r1）と P（追試 task 01M6617H9V0TZGQKFA9J82141A の artifacts/ops-r1）は読んだだけ。
出力は T=/local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1（repo の外）。WU ごとの詳細は `2026-10-30-k4-evidence-close/<wu>.md`。

## 結論
- 試行は trial_id で 3 件（A-t001 の 2 pipeline 間の重複を除いた）。A は 1 試行で独立 grader pass・結末 passed。B-t001（B-celeris-ops）は grader pass だが結末は condition_drift の停止で非成功。B-t002（B-celeris-ops-r2）は grader pass・結末 passed（cos_self_completed）。B の 2 試行は条件が違い、合算した率は作らない。
- 採点は新 binary で raw から 2 回作り直し、2 回の出力は byte 一致、grades.jsonl と grade-out.json は第 2 版と byte 一致。
- 消費は 6 区分（task_execution・project_cos・general_cos・external_operator・shared・evaluator）で別掲し、試行の合計には task_execution だけを足す。B の案件 CoS（claude-opus-5-5）は B-t001 550,684 tokens・B-t002 360,051 tokens で、試行に足していない。`reduction_confirmable` は両 pipeline・全 metric で false。
- 宣言 worker model は claude-sonnet-5-5、B の実際の model は CoS の claude-opus-5-5、B-t002 の実行経路は cos_self_completed。
- 人の能動時間は未記録。報告は 0 と書かず、欠測・下限と coverage（t002 は lower_bound、trial_ratio 0.5、op_interval_ratio 0.0）で示した。
- 外部 operator の支援は session-map v3 で 20 区間を transcript と照合し、結果に効いたか決められない 3 区間（3,912,900 tokens）を unknown に残した。

## 変更（commit の件名）
- `snapshot: K4 証拠束の確定写し（human-ops 旧/新 hash・operator transcript の範囲・DB 本体）の進捗を書く`
- `addendum: B-t002 の事後追補（事前固定と事後の区別、未確認の判断の分離）の進捗を書く`
- `acct-labels: 消費の 6 区分・宣言/観測 model・実行経路・比較範囲を ops-r1 の集計に出す`（ADR `agent-docs/adr/2026-10-30-ops-r1-consumption-labels-model-scope.md`）
- `human-cov: 人の時間の欠測・下限と coverage、外部 operator の unknown 区分を足す`（ADR `agent-docs/adr/2026-10-30-ops-r1-human-time-coverage.md`）
- `support-audit: operator 支援の区分を transcript と照合し session-map v3 と recount を作る`
- `regrade: K4 の 2 pipeline を新 binary で raw から 2 回再生成し、採点不変と A-t001 の重複除去を確かめた進捗を書く`
- `report: K4 報告第 3 版と task の進捗を書く`（この WU。repo の変更はこの進捗ファイルだけ）

## 報告第 3 版（WU report）
- `T/report.md`: 結論・人が決めること、試行と採点（条件別、合算率なし）、消費 6 区分（CoS 別掲、task 0 を全体 0 と書かない）、宣言/観測 model と実行経路、人の時間（欠測・coverage）、operator 支援の監査（unknown を含む）、追補（pre_fixed と post_hoc、未確認の判断）、証拠束、比較範囲、論文での使い方・未検証の範囲・残件・適用範囲。
- `T/report-facts.json`（`T/build-facts.py` で生成）: 79 fact・164 item。各 fact は本文の引用と、値の出所（`T/regrade`・`T/operator`・`T/evidence`・`T/addendum`・`T/scope` の JSON と path）を持つ。
- `T/check-report.py`: quote が本文にあること、path の値が facts と等しいこと、値の書式が quote にあることを確かめる。逆向きに、本文の 3 桁区切りの数と 12 字以上の 16 進の識別子が facts を経ていなければ落とす。B の合算率の語句（「50%」「1/2」など）も落とす。
- `T/scope/comparison-scope.json`: 測定の条件（campaign.json の B の daemon release と cos-v6）と、報告を書いた時点の本番（`/local/celeris/state/current` の release と `~/.config/celeris/config.toml` の `[cos.triage] policy_version` = 10、どちらも読むだけ）を並べ、本番の値を試行に使っていないことを記録する。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 -I T/check-report.py T/report.md T/report-facts.json` → exit 0（facts=79 items=164 files=11 errors=0）
- 負の確認（`$TMPDIR` の写し）: 本文の 550,684 を 550,685 に変える → 2 件 NG。facts に無い数 1,234・識別子・「50%」「1/2」を足す → 4 件 NG。facts の値を 3→4 に変える → 1 件 NG。CoS の行の「足さない」を「足す」に変える → 1 件 NG。どれも exit 1
- `python3 -I config/skills/yomiyasu/scripts/yomiyasu_lint.py T/report.md` → 文末コロン 0 件。残る警告は箇条書きの比率と「版」（報告の版の意味）の 2 件
- `python3 -I T/regrade/check-combined.py T/regrade/combined.json` → ok true・trial_id 3・条件 3。`T/regrade/regen-diff.txt` は 0 byte
- `python3 -I T/addendum/check-addendum.py T/addendum/k4-b2-addendum.json` → exit 0（status ok）
- `python3 -I T/evidence/check-evidence.py T/evidence` → checked files=212 errors=0
- R 全 file と P/operator 全 file の sha256 一覧を取り直し、`T/evidence/inputs-guard/*-after.sha256` と `cmp` で一致。R で `sha256sum -c P/post-b2/parent-after.sha256` → exit 0
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check` → 下の「検査」節

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。この WU の repo の変更は進捗ファイルだけで、文書検査だけが走った）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok
- 集計 code の回帰試験（ops_r1_pipeline_assemble・human_ops_report・operator_usage）は WU acct-labels・human-cov の進捗にある。WU report では code を変えていない

## 未解決
- 人が答えていない判断: (a) B-t002 の state 置き場 5 行を drift としない扱い、(b) B-t002 を cos_self_completed で成功に数える扱い、(c) B-t002 区間の session-map の区切り、unknown 3 区間が結果に効いたとみなすか、親の承認追補の問い（承認が approval-request.md の作成より前）。
- 人の承認（b2-approve）は外部 operator の記録だけで、Celeris の decision も時刻も無い。
- t001 の human-ops には人の op が 1 件も無く、coverage の判定は bound=exact・union_secs=0 を出す。transcript には測定区間の人の発言が 4 回あるので、報告はこれを欠測として書いた。判定そのものは直していない。
- 費用は欠けがあり（complete=false）、比較していない。外部 operator の消費は final-accounting の外。
- finished の記録が無い subagent 67 件が終わっていたかは未確認。
- B では Celeris の task 経路（planner・worker・reviewer）が 1 度も動いておらず、稼働経路での挙動は未確認。

## 提案
- human-ops の coverage 判定で、評価者の記録に人の op が 0 件の集合を exact にせず、「人の op の記録なし」を別の bound（例 not_recorded）にする。operator の transcript に origin.kind=human の行がある区間と照合できるとよい。
- 人の発言による承認・判断は、外部 operator の human-ops 記録だけでなく Celeris の decision として時刻付きで残す手順を runbook に足す。
- 報告の数値は今回の report-facts.json と check-report.py の形（引用＋JSON path＋逆向きの検査）を ops-r1 の報告の標準にする。
