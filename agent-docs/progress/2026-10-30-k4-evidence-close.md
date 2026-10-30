---
title: K4 追試の証跡確定と報告第 4 版（CoS 別掲・実 model・欠測・比較範囲・人の時間の not_recorded）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 追試の証跡確定と報告第 4 版

完了日: 2026-10-30

状態: 全 WorkUnit 完了（最後の WU report-v4 は報告第 4 版とこの進捗ファイル）。LLM・daemon・実試行・本番配備はしていない。
入力の R（親 K4 task 01M62S8XWCZDCY2ZBVQ35176MH の artifacts/ops-r1）と P（追試 task 01M6617H9V0TZGQKFA9J82141A の artifacts/ops-r1）は読んだだけ。
出力は T=/local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1（repo の外）。WU ごとの詳細は `2026-10-30-k4-evidence-close/<wu>.md`。

## 差し戻しと修正（第 3 版 → 第 4 版）
- 差し戻しの理由: final review は criterion 3（人の能動時間を 0 としない）で第 3 版を差し戻した。再生成した t001 の human-ops-report が、人の op が 0 件の集合（全体・arm・試行）を `human_time_coverage.bound=exact`・`union_secs=0`・`trial_ratio=1.0` と出していた。t001 の測定区間には transcript に人の発言が 4 回あるので、未記録の人の時間を 0 秒と確定したことになる。報告の文が「0 と読まない」と書いても、機械の出力が 0 のままでは誤読を防げない。
- 修正（WU cov-not-recorded）: 評価者の op 記録はあるが人の op が 0 件の集合を `not_recorded`（union_secs=null）にした。human-op の schema に「人の op なし」を宣言する記録が無いため。exact は全試行に人の op の区間があるときだけ。回帰試験を足した（詳細は `2026-10-30-k4-evidence-close/cov-not-recorded.md`）。
- 再生成（WU regrade-v2）: 直した binary で raw から 2 回作り直した（`T/regrade-v2/`）。2 回の出力は byte 一致、各 pipeline 17 step exit 0、compare-grades の mismatches=[]。旧 binary の出力との違いは human-ops-report だけで、採点・消費の file は byte 一致。t001 は全体・arm A・arm B・試行 2 件とも not_recorded・union_secs=null、t002 は全体 lower_bound・arm A missing・arm B lower_bound で旧と同じ。旧の `T/regrade/` は残した。
- 報告第 4 版（WU report-v4）: `T/report.md` の数値の正本を `T/regrade-v2/` に替えた。§4（人の能動時間）の t001 を not_recorded・union_secs=null・trial_ratio 0.0（未記録の試行 2）に書き直し、§9 の残件から判定の不具合を外して、直した内容を書いた。採点・消費・追補・支援区分の記述と数値は変えていない。（依頼文は人の時間を §7 と呼んでいるが、報告では §4 が人の時間、§7 は証拠束。）
- `T/build-facts.py`・`T/report-facts.json`: 出所を `regrade-v2` に替え、t001 の not_recorded・null・trial_ratio 0.0、arm と試行の not_recorded、t002 の recorded_trials、check-regrade-v2 の結果の fact を足した（83 fact・175 item）。禁止語に「union_secs=0」「exact 0」「exact・union 0」を足した。
- `T/check-report.py`: 引数は従来どおり report.md と report-facts.json。新しい検査 6 を足した。facts の `coverage` に挙げた human-ops report の全集合（全体・arm・試行）で、人の op 0 件の集合が missing か not_recorded・union_secs=null であること、本文に各 pipeline の全体の bound を書いた行があること、not_recorded の集合があれば本文に not_recorded があること、人の op 0 件の pipeline の行に exact が無いことを確かめる。

## 結論
- 試行は trial_id で 3 件（A-t001 の 2 pipeline 間の重複を除いた）。A は 1 試行で独立 grader pass・結末 passed。B-t001（B-celeris-ops）は grader pass だが結末は condition_drift の停止で非成功。B-t002（B-celeris-ops-r2）は grader pass・結末 passed（cos_self_completed）。B の 2 試行は条件が違い、合算した率は作らない。
- 採点は人の時間の判定を直した binary で raw から 2 回作り直し（`T/regrade-v2/`）、2 回の出力は byte 一致、grades.jsonl と grade-out.json は第 2 版と byte 一致。
- 消費は 6 区分（task_execution・project_cos・general_cos・external_operator・shared・evaluator）で別掲し、試行の合計には task_execution だけを足す。B の案件 CoS（claude-opus-5-5）は B-t001 550,684 tokens・B-t002 360,051 tokens で、試行に足していない。`reduction_confirmable` は両 pipeline・全 metric で false。
- 宣言 worker model は claude-sonnet-5-5、B の実際の model は CoS の claude-opus-5-5、B-t002 の実行経路は cos_self_completed。
- 人の能動時間は未記録。報告は 0 と書かず、欠測・下限と coverage で示した。t001 は not_recorded（union_secs=null、trial_ratio 0.0）、t002 は lower_bound（trial_ratio 0.5、op_interval_ratio 0.0）。
- 外部 operator の支援は session-map v3 で 20 区間を transcript と照合し、結果に効いたか決められない 3 区間（3,912,900 tokens）を unknown に残した。

## 変更（commit の件名）
- `snapshot: K4 証拠束の確定写し（human-ops 旧/新 hash・operator transcript の範囲・DB 本体）の進捗を書く`
- `addendum: B-t002 の事後追補（事前固定と事後の区別、未確認の判断の分離）の進捗を書く`
- `acct-labels: 消費の 6 区分・宣言/観測 model・実行経路・比較範囲を ops-r1 の集計に出す`（ADR `agent-docs/adr/2026-10-30-ops-r1-consumption-labels-model-scope.md`）
- `human-cov: 人の時間の欠測・下限と coverage、外部 operator の unknown 区分を足す`（ADR `agent-docs/adr/2026-10-30-ops-r1-human-time-coverage.md`）
- `support-audit: operator 支援の区分を transcript と照合し session-map v3 と recount を作る`
- `regrade: K4 の 2 pipeline を新 binary で raw から 2 回再生成し、採点不変と A-t001 の重複除去を確かめた進捗を書く`
- `report: K4 報告第 3 版と task の進捗を書く`
- `cov-not-recorded: 人の op 0 件の集合を exact にせず not_recorded にし回帰試験を足す`
- `cov-not-recorded: 範囲拡大後に検査を再実行し進捗を更新`
- `regrade-v2: cov-not-recorded の修正 binary で raw から 2 回再生成し採点不変と人の時間の not_recorded を確かめる`
- `report-v4: K4 報告第 4 版（人の時間の not_recorded）と task の進捗を書く`（この WU。repo の変更はこの進捗ファイルだけ）

## 報告第 3 版（WU report、第 4 版で上の差し戻しの節のとおり改めた）
- `T/report.md`: 結論・人が決めること、試行と採点（条件別、合算率なし）、消費 6 区分（CoS 別掲、task 0 を全体 0 と書かない）、宣言/観測 model と実行経路、人の時間（欠測・coverage）、operator 支援の監査（unknown を含む）、追補（pre_fixed と post_hoc、未確認の判断）、証拠束、比較範囲、論文での使い方・未検証の範囲・残件・適用範囲。
- `T/report-facts.json`（`T/build-facts.py` で生成）: 79 fact・164 item。各 fact は本文の引用と、値の出所（`T/regrade`・`T/operator`・`T/evidence`・`T/addendum`・`T/scope` の JSON と path）を持つ。
- `T/check-report.py`: quote が本文にあること、path の値が facts と等しいこと、値の書式が quote にあることを確かめる。逆向きに、本文の 3 桁区切りの数と 12 字以上の 16 進の識別子が facts を経ていなければ落とす。B の合算率の語句（「50%」「1/2」など）も落とす。
- `T/scope/comparison-scope.json`: 測定の条件（campaign.json の B の daemon release と cos-v6）と、報告を書いた時点の本番（`/local/celeris/state/current` の release と `~/.config/celeris/config.toml` の `[cos.triage] policy_version` = 10、どちらも読むだけ）を並べ、本番の値を試行に使っていないことを記録する。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 第 4 版: `python3 -I T/check-report.py T/report.md T/report-facts.json` → exit 0（facts=83 items=175 files=12 errors=0）
- 第 4 版の負の確認（`$TMPDIR` の写し）: 第 3 版の report.md を新しい facts で検査 → 10 件 NG・exit 1（t001 の not_recorded の quote が無い、全体の bound の行が無い、not_recorded の語が無い、t001 の行に exact が 2 件、禁止語 union_secs=0 など）。coverage の対象を旧 binary の `T/regrade/pass-2/t001/human-ops-report/report.json` に替える → 全体・arm 2・試行 2 の 5 集合で「人の op 0 件なのに bound=exact union_secs=0」・exit 1
- `python3 -I T/regrade-v2/check-regrade-v2.py`（WU regrade-v2 の結果 `T/regrade-v2/check-regrade-v2.json`）→ ok true・errors []
- 以下は第 3 版の時点の確認（入力は変えていないので第 4 版でも同じ）
- 第 3 版: check-report.py → facts=79 items=164 files=11 errors=0
- 負の確認（`$TMPDIR` の写し）: 本文の 550,684 を 550,685 に変える → 2 件 NG。facts に無い数 1,234・識別子・「50%」「1/2」を足す → 4 件 NG。facts の値を 3→4 に変える → 1 件 NG。CoS の行の「足さない」を「足す」に変える → 1 件 NG。どれも exit 1
- `python3 -I config/skills/yomiyasu/scripts/yomiyasu_lint.py T/report.md` → 文末コロン 0 件。残る警告は箇条書きの比率と「版」（報告の版の意味）の 2 件
- `python3 -I T/regrade/check-combined.py T/regrade/combined.json` → ok true・trial_id 3・条件 3。`T/regrade/regen-diff.txt` は 0 byte（第 4 版の正本 `T/regrade-v2/regen-diff.txt` も 0 byte）
- `python3 -I T/addendum/check-addendum.py T/addendum/k4-b2-addendum.json` → exit 0（status ok）
- `python3 -I T/evidence/check-evidence.py T/evidence` → checked files=212 errors=0
- R 全 file と P/operator 全 file の sha256 一覧を取り直し、`T/evidence/inputs-guard/*-after.sha256` と `cmp` で一致。R で `sha256sum -c P/post-b2/parent-after.sha256` → exit 0
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`sh scripts/dev/progress-index.sh --check` → 下の「検査」節

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。WU report-v4 の repo の変更は進捗ファイルだけで、文書検査だけが走った）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok
- 集計 code の回帰試験（ops_r1_pipeline_assemble・human_ops_report・operator_usage、not_recorded の試験と actor_trace_flow1..5）は WU acct-labels・human-cov・cov-not-recorded の進捗にある。WU report-v4 では code を変えていない

## 未解決
- 人が答えていない判断: (a) B-t002 の state 置き場 5 行を drift としない扱い、(b) B-t002 を cos_self_completed で成功に数える扱い、(c) B-t002 区間の session-map の区切り、unknown 3 区間が結果に効いたとみなすか、親の承認追補の問い（承認が approval-request.md の作成より前）。
- 人の承認（b2-approve）は外部 operator の記録だけで、Celeris の decision も時刻も無い。
- t001 の human-ops には人の op が 1 件も無い。判定は直し（not_recorded・union_secs=null）、人の時間は欠測のまま。人の発言 4 回の時間は記録に無く、推測で補っていない。
- 費用は欠けがあり（complete=false）、比較していない。外部 operator の消費は final-accounting の外。
- finished の記録が無い subagent 67 件が終わっていたかは未確認。
- B では Celeris の task 経路（planner・worker・reviewer）が 1 度も動いておらず、稼働経路での挙動は未確認。

## 提案
- human-op の schema に「人の op なし」を宣言する記録を足せば、人が介入しなかった試行を 0 秒と確定できる（現状は not_recorded）。operator の transcript に origin.kind=human の行がある区間と照合できるとよい。
- 人の発言による承認・判断は、外部 operator の human-ops 記録だけでなく Celeris の decision として時刻付きで残す手順を runbook に足す。
- 報告の数値は今回の report-facts.json と check-report.py の形（引用＋JSON path＋逆向きの検査）を ops-r1 の報告の標準にする。
