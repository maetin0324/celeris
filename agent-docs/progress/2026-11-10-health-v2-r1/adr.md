---
title: 健全性v2 R1 adr — ADR adapter-result-contract
tasks: [01M6TYVJSZDSCB697193J01FYW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R1 adr — ADR adapter-result-contract

完了日: 2026-11-10

WorkUnit `adr`（R1 の ADR: harness adapter 5 本の結果契約と実行共通処理の棚卸し・境界・試験の配置・R6 への引き継ぎ）。基準は `$CELERIS_WU_BASE`。コードは変えていない。

## 変更
- 件名「R1 adr: ADR adapter-result-contract と task・unit の進捗」の commit — [ADR 2026-11-10 harness adapter の結果契約と実行共通処理](../../adr/2026-11-10-adapter-result-contract.md)（状態: 提案）、task の進捗 [2026-11-10-health-v2-r1.md](../2026-11-10-health-v2-r1.md)、この file を追加した。

## 決めたこと（要約）
- (1) 現行挙動の表: 5 adapter の result.json の解釈（優先順位 `question` > `wait` > `summary` > `yield`、無効値は `Error{retryable: true}`、`lenient_evidence`、file が無いときの分類、誤配置の回収）、usage と cost（推定は claude_code・codex が上書き、aider は欠けたときだけ、pi・acp は無し）、wall clock（`BudgetExhausted{WallClock}`）と idle（`Error{retryable: true}`）、cancel（`process_group(0)` と `ProcessGroup::register`、timeout は `kill_now`、acp は `session/cancel` を先に送る）、log（`runs/<run_id>/stdout.jsonl` または `stdout.log`・`stderr.log`・`result.json`）、artifact（`artifacts/result.json`、`misplaced_report` は claude_code・codex だけ。dispatcher の `worker_task.rs` が全 adapter を覆う）を表にした。観測できる食い違いは 8 件。
- (2) 共通の契約: `ResultFile`・`lenient_evidence`・`interpret_result_text`・`read_result_file` を `result_report.rs` に置く（D1）。`adapter::result_file_wait` は消し、`adapter::wait_terminal` は dispatcher が使うので公開の署名を保つ。実行共通処理（`prepare_run`・`deadline_terminal`・`spawn_capture_stderr`・`with_estimated_cost`・`finish_run`）は `adapter.rs` に置く（D2）。食い違い 8 件を 1 件ずつ決めた: 統一 4 件（誤配置の回収・cost の推定〈欠けたときだけ〉・browser の log の `/dev/null`・二重の parse）、provider 固有のまま 3 件（usage を載せる範囲・acp の `session/cancel`・acp の `check_arg_lengths` 無し）、統一しない 1 件（stdout の log 名）。観測できる変化は pi・acp・aider の誤配置が adapter の時点で契約違反になること（task の終端は既に dispatcher が同じ値にしている）だけ。`preamble::worktree_note` は削除（dup-candidates `unref-worktree-note`）。
- (3) provider 固有: wire・認証・session の再開・stream の解析・result.json が無いときの分類・file より前の判定・timeout の止め方・usage の観測・browser の文言の伏せ字を表にした。
- (4) 試験: 共通契約 5 件（`result_contract_priority_order`・`result_contract_lenient_evidence`・`result_contract_invalid_json_is_retryable_error`・`result_contract_no_known_field_is_retryable_error`・`result_contract_usage_passthrough`）を `result_report.rs` に、実行共通処理 5 件（`run_common_` で始まる）を `adapter.rs` に置く。adapter ごとに結線の試験を 1 件以上残し、契約の写しの試験 13 件を消す（理由を書いた）。
- (5) R6 への引き継ぎ: 研究 harness 3 本の監視 loop は `deadline_terminal` を呼べる（langmem は idle を値で渡す）。wall clock の終端の差（`Error` と `BudgetExhausted`）、dispatcher 側の `misplaced_report` の扱い、`subprocess.rs` の helper の署名の保持を書いた。

## 目的文との違い
- 目的文は優先順位を「wait > question > summary > yield」と書くが、現行の code と ADR-0090 D1 は `question` > `wait`。ADR は現行の順を契約にし、task の進捗の「未解決」に書いた。
- 目的文の「cost の推定は claude_code・codex が上書き」は現行どおりだが、両者の入力は常に `cost_usd: None` なので、aider の「欠けたときだけ」に統一しても結果は変わらないことを code で確かめた（claude_code.rs の `result.usage` の parse と codex.rs の `turn.completed` の parse が `cost_usd: None` を置く）。

## 検査（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `git diff --stat $CELERIS_WU_BASE -- crates/` → 差分なし（受け入れ条件 2）。
- `sh scripts/dev/check-doc-links.sh` → exit 0。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R1 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。
- `sh scripts/dev/check-adr-numbers.sh --refs --base $CELERIS_WU_BASE` → exit 0（他 branch の ADR と名前が重ならない）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は ADR・進捗 2 file の 3 つだけ。
- `python3 scripts/dev/check-human-text.py` → exit 0（11 files, 0 findings。対象は code の中の人向け文面で、Markdown は見ない）。
