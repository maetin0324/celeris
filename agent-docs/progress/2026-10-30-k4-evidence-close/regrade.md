---
title: K4 追試の証跡確定 — WU regrade（新 binary で raw から 2 回再生成、採点不変、A-t001 の重複除去）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
completed: 2026-10-30
---
# K4 追試の証跡確定 — WU regrade

完了日: 2026-10-30

状態: 完了。採点結果は前の版と同じ。LLM・daemon・実試行・本番配備はしていない。R・P の file は書き換えていない。repo の code は変えていない（この WU の repo の変更はこの進捗ファイルだけ）。

- R = /local/celeris/data/workspaces/01M62S8XWCZDCY2ZBVQ35176MH/artifacts/ops-r1（親 K4 task、読むだけ）
- P = /local/celeris/data/workspaces/01M6617H9V0TZGQKFA9J82141A/artifacts/ops-r1（追試 task、読むだけ）
- T = /local/celeris/data/workspaces/01M669QAZE5WEQDJBQS8MRX1AK/artifacts/ops-r1（出力。説明は T/regrade/README.md）

## やったこと
- この branch の HEAD（件名「integrate wu/support-audit (phase audit)」。acct-labels・human-cov の集計変更を含む）から
  `cargo build -p celeris-campaign` で build し、T/bin/celeris-campaign-<sha256 先頭 12 字> に置いた（実際の名前と sha256 は T/regrade/README.md）。
- T/run-k4-close.sh を R/post/run-post.sh と P/post-b2/run-post-b2.sh を元に書いた。`pass-N` を引数に取り、2 つの pipeline を
  T/regrade/work に作ってから T/regrade/pass-N へ移す（consistency.json が絶対 path を含むので、出力先は毎回同じにした）。
  - t001 = A-t001 + B-t001（R/run の旧 manifest、`--b-stop R/raw/stop.json`）
  - t002 = A-t001 + B-t002（R/run/B-t002 の manifest、`--b-cos-self-completed`）
  - DB: T/evidence/db/<B|B-t002>/db-copy.sqlite を work へ写し、db-copy.sha256 で照合してから使う。evidence の写しを直接開かないのは、
    ops-r1-trace が開くと -wal が作られ、evidence の SHA256SUMS の照合が落ちるため。
  - human-ops: T/evidence/human-ops/human-ops.jsonl（追記後 54 行）から、out/trials.jsonl の campaign_sha256 と同じ
    manifest_sha256 の行を順序のまま選ぶ（t001 26 行、t002 28 行。合計 54 で漏れ・重複なし）。0 行なら step を失敗にする。
- T/regrade/compare-grades.py: 試行ごとに final_pass・artifact_sha256・grader/tests/lock の hash・grade 本体と、trials.jsonl の
  outcome・passed・reason・tokens・wall・cost・model を前の版（t001 ↔ R/post/pass-2、t002 ↔ P/post-b2/pass-2）と比べる。
- T/regrade/build-combined.py → T/regrade/combined.json: trial_id（out/campaign.json の contract.conditions.<config>.trial_ids）で
  2 つの pipeline を合わせる。A-t001 は 2 つの pipeline の値が同じことを確かめて 1 行にし、sources に t001・t002 を残す。
  B-t001（condition B-celeris-ops）と B-t002（B-celeris-ops-r2）は別の行・別の by_condition にし、`pooling.B = not_pooled`。B を合算した率は作らない。
- T/regrade/check-combined.py: trial_id の一意、A-t001 の sources が t001・t002 の 1 回ずつ、B の 2 条件が別、by_condition の数が trials と合う、
  合算率の欄が無いことを確かめる。

## 結果（3 行）
| trial_id | condition | outcome（reason） | final_pass | 実行経路 | 宣言 / 観測 model |
|---|---|---|---|---|---|
| A-t001 | A-direct-claude-code | passed | true | direct_harness | claude-sonnet-5-5 / claude-sonnet-5-5 |
| B-t001 | B-celeris-ops | cancelled（condition_drift） | true | stopped | claude-sonnet-5-5 / null |
| B-t002 | B-celeris-ops-r2 | passed（cos_self_completed） | true | cos_self_completed | claude-sonnet-5-5 / claude-opus-5-5 |

- final_pass は独立 grader が成果物を採点した結果、outcome は試行の結末。B-t001 は成果物が採点を通っても試行は非成功のまま。
- 両 pipeline とも `reduction_confirmable` は tokens・list_cost_usd・cash_cost_usd がすべて false、`comparison_scope.comparable=false`
  （t002 の理由: B の経路が条件の境界 task_pipeline の外、腕間で観測 model が違う ほか）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash T/run-k4-close.sh pass-1`・`bash T/run-k4-close.sh pass-2` → どちらも exit 0、t001・t002 とも 17 step すべて exit 0
- `diff -r T/regrade/pass-1 T/regrade/pass-2 > T/regrade/regen-diff.txt` → exit 0、0 byte
- t001・t002 の consistency.json → `consistent: true`（両 pass）
- `python3 -I T/regrade/compare-grades.py T/regrade/pass-1`・`… pass-2` → exit 0、`same_as_previous: true`、mismatches 0。
  grades.jsonl と grade-out.json は前の版と byte 一致。final_pass は A・B とも true（grade-out の final_pass 2/2、両 pipeline）。
  artifact_sha256 は 3 試行とも前の版と同じ（値は T/regrade/compare-grades.pass-2.json）
- `python3 -I T/regrade/check-combined.py T/regrade/combined.json` → exit 0、`ok: true`、trial_id 3 件、条件 3 件。
  pass-1 から作った combined と pass-2 から作った combined は built_from の行を除いて同じ
- R 全 file の sha256 一覧が T/evidence/inputs-guard/R-after.sha256 と一致、`sha256sum -c P/post-b2/parent-after.sha256`（R で）→ OK、
  P/operator の一覧が P-operator-after.sha256 と一致、T/evidence の SHA256SUMS と check-evidence.py → errors=0
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → 下の「検査」節

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。この WU の repo 変更は進捗ファイルだけで、文書検査だけが走った）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok
- 新 binary の集計変更の回帰試験（ops_r1_pipeline_assemble・human_ops_report・operator_usage）は WU acct-labels・human-cov の進捗にある。この WU では code を変えていない

## 既存の出力との違い
- trials.jsonl・records.jsonl・final-accounting.json・export は新しい欄（execution_path・observed_model・consumption_lanes・comparison_scope 等）が増え、
  R/post/pass-2・P/post-b2/pass-2 と byte 一致しない（acct-labels の ADR のとおり）。採点（grades.jsonl・grade-out.json）は byte 一致。
- t001 の human-ops は R/post では 26 行の file 全体を使った。今回は追記後 54 行から campaign_sha256 で同じ 26 行を選んだ。

## 未解決
- この WU は報告の本文を書いていない（WU report）。
- 外部 operator の消費は final-accounting の入力に無い（`external_operator.state = outside_records`）。

## 提案
- 報告第 3 版は combined.json の by_condition を条件別に引用し、「B 2 試行中 1 成功」のような合算を書かない。
