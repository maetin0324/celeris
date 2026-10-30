---
title: K4 追試の証跡確定 — WU regrade-v2（cov-not-recorded の修正 binary で raw から 2 回再生成、採点不変、t001 の人の時間は not_recorded）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 追試の証跡確定 — WU regrade-v2

完了日: 2026-10-30

状態: 完了。採点結果は第 2 版と同じ。新旧 binary の出力の違いは human-ops-report だけ。LLM・daemon・実試行・本番配備はしていない。
R・P・旧 T/regrade/・旧 binary は書き換えていない。repo の code は変えていない（この WU の repo の変更はこの進捗ファイルだけ）。

- R = /local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1（親 K4 task、読むだけ）
- P = /local/celeris/data/workspaces/01M6617H9V0TZGQKFA9J82141A/artifacts/ops-r1（追試 task、読むだけ）
- T = /local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1（出力。説明は T/regrade-v2/README.md）

## やったこと
- WU の base（件名「integrate wu/cov-not-recorded (phase fixcov)」）で `cargo build -p celeris-campaign`（dev、渡された CARGO_TARGET_DIR）し、
  T/bin/celeris-campaign-<commit 先頭 12 字> に写して書き込み不可にした。旧 binary（WU regrade が置いたもの）は残した。新旧の名前と sha256 は T/regrade-v2/README.md。
- T/run-k4-close.sh を基に T/regrade-v2/run.sh を作った。変えたのは出力先（T/regrade-v2/<pass>）・REPO（K4_REPO 必須）・CAMPAIGN_BIN（新 binary）・
  grade-cases（T/regrade-v2/grade-cases）だけ。`K4_REPO=<この worktree> bash run.sh pass-1`、続けて pass-2 を走らせた。
- 旧 T/regrade の compare-grades.py・build-combined.py・check-combined.py を写して同じ比較をした。
- 受け入れ検査 T/regrade-v2/check-regrade-v2.py を書いた。作業前の R・P・旧 regrade/・旧 binary の sha256 は T/regrade-v2/guard/ に取った。

## 証拠（提出時の HEAD で実行）
| 条件 | コマンド | 結果 |
|---|---|---|
| 0 pass-1 = pass-2 | `bash run.sh pass-1`、`bash run.sh pass-2`、`diff -r pass-1 pass-2 > regen-diff.txt` | 両 pass exit 0、各 pipeline 17 step 全部 exit 0（log の exit=0 は各 34 行）。diff exit 0、regen-diff.txt 0 byte |
| 1 採点不変・combined | `python3 -I compare-grades.py T/regrade-v2/pass-2 --json-out compare-grades.pass-2.json`、`build-combined.py`、`check-combined.py combined.json` | exit 0、mismatches=[]（pass-1 も同じ）。check-combined ok、trial_id 3 件（A-t001・B-t001・B-t002）、conditions 3 |
| 2 human_ops=0 が exact でない | `python3 -I check-regrade-v2.py` | t001 は全体・arm A・arm B・試行 2 件とも bound=not_recorded・union_secs=null（旧は exact・0）。t002 arm A は missing（union null）。human_ops≥1 の t002 arm B・全体は lower_bound（union 0 は下限） |
| 3 旧物・親の不変 | 同上、`cd R && sha256sum -c --quiet P/post-b2/parent-after.sha256` | 旧 regrade/・R・P・旧 binary とも作業前と一致、R は evidence/inputs-guard/R-after.sha256 と一致。sha256sum -c exit 0（85,932 file） |

新旧の違い（`diff -rq T/regrade/pass-N T/regrade-v2/pass-N`）: t001・t002 の human-ops-report/report.json・report.md と consistency.json だけ。
consistency.json の違いは出力先の絶対 path だけ（path を置き換えると一致）。grades.jsonl・grade-out.json は 4 file とも cmp で一致。

repo の検査の結果は下の「検査」に書く（この WU は repo の code を変えていない）。

## 検査
- `bash scripts/dev/test-affected.sh`: exit 0。「試験対象の変更なし（docs のみ等）」、文書検査（progress_index_commit_hashes・check-doc-links 等）は合格
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0（差分なし）
- `sh scripts/dev/progress-index.sh --check`: exit 0

## 未解決事項
- t002 arm B の union_secs=0 は lower_bound（区間の無い人の op 1 件）。人の能動時間の値は未記録のままで、この WU では埋めていない（時刻を推測で補わない）。
- 報告第 4 版（WU report-v4）で regrade-v2 の数値を引く。

## 提案
- なし。
