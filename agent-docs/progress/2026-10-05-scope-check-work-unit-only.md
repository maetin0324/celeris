---
title: WU の範囲 check を段の統合・main 取り込みの後に再評価しない（WorkUnitCheck.scope）
tasks: [01M45K71AN6A4SKRZMXKBEB3DR]
status: done
updated: 2026-10-05
---

# WU の範囲 check を段の統合・main 取り込みの後に再評価しない（WorkUnitCheck.scope）

完了日: 2026-10-05。設計は [ADR-0074 付記（2026-10-05、範囲 check は WU の作業時だけ流す）](../adr/0074-parallel-work-units-checkpoints-milestones-quota.md)。
書き方は [agent-docs/guides/work-unit-checks.md](../guides/work-unit-checks.md)。

## 何を直したか

2026-10-04〜05 の本番で、WU の範囲 check（`git diff --name-only <基点>` が許可範囲の外を出していないか）が段の統合後の検査や
子 task の final review で流れ直し、他の WU・main の変更を拾って落ちる誤検出が 4 回あった（01M440S16A・01M44FP87W・01M4577C94・01M44C029S）。

- `WorkUnitCheck` に `scope: bool` を足した（既定 false、JSON には true のときだけ出る）。
- 段の統合（`start_integration` D1.4 の 4 → `integration_checks_for_phase`）は `scope: true` の check を流さない。
- leaf を子 task に上げる `promote_to_task` は `scope: true` の check を acceptance に写さない（残りが無ければ従来どおり done_when → reviewer）。
- repair に渡す範囲（`repair_scope_from_units` / `review_repair_scope`）は `scope: true` を正とし、従来の `git diff` 推定も残す。
- WU の checks と run の環境に `CELERIS_WU_BASE`（WU の base_commit）と `CELERIS_WU_TARGET`（統合先の Task ブランチ）を渡す。
- 既定の範囲 check の形を planner 指示（`PLANNER_CHECK_GUIDANCE`、/1・/3 の形、replan の説明）と guide に揃えた:
  `out=$({ git diff --name-only "${CELERIS_WU_BASE:-HEAD}"; git ls-files --others --exclude-standard; } | sort -u | grep -vE '^(<許可>)'); [ -z "$out" ] || { echo "out of scope:"; echo "$out"; exit 1; }`
  （範囲外の path を出してから非 0。無言の `test -z` は書かない）。
- `scope: true` の check が stdout・stderr とも空で落ちたら、不合格の判定文に「範囲外の path を出していない」旨の一文を足す（判定は変えない）。
- WU 前置き（`preamble.rs`）に環境変数の案内を 1 文足した。schema と gui/web の生成型を再生成した。

## 受け入れ条件と証拠

| 条件 | 証拠 |
| --- | --- |
| 0. 範囲 check は WU の作業時だけ流れ、統合後・final review では落ちない。作業時の範囲外変更は落ちる | `crates/task-dispatch/src/dispatcher/tests/scope_checks.rs`: `scope_checks_run_only_at_work_unit_time_and_not_after_integration`（2 WU が a/・b/ を変え、統合の検査に範囲 check が無く task は Done）、`an_out_of_scope_write_fails_the_work_unit_check_and_prints_the_paths`（`b/oops.txt` が判定文に載る）、`a_silent_failing_scope_check_gets_a_hint_in_its_detail`、`integration_checks_for_phase_skips_scope_checks`、`repair_scope_checks_include_scope_flag_and_git_diff_heuristic`。`crates/task-core/src/tree/tests.rs`: `promote_to_task_drops_scope_checks_from_acceptance` |
| 1. planner 指示と docs に書き方があり、落ちるときは path を出す | `crates/task-worker/src/claude_code/prompt.rs`（`PLANNER_CHECK_GUIDANCE` ほか）、`crates/task-worker/src/claude_code/tests.rs`、`agent-docs/guides/work-unit-checks.md`、`agent-docs/README.md`、`docs/architecture-map.md` |
| 2. ADR の付記と検査結果 | ADR-0074 付記 2026-10-05。下の検査結果 |

## 検査結果

| コマンド | 結果 |
| --- | --- |
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest `3942 tests run: 3942 passed, 12 skipped`（83.5 s）、doctest exit 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo test -p task-dispatch scope_checks` | 6 passed |
| `cargo test -p task-worker claude_code` | 105 passed |
| `UPDATE_SCHEMA=1 cargo test -p task-core` / `-p task-api schema` | 再生成後、UPDATE_SCHEMA なしの照合テストも通る |
| `sh scripts/dev/check-doc-links.sh` / `check-adr-numbers.sh` / `progress-index.sh --check` / `python3 scripts/dev/check-architecture-map.py` | すべて ok |

gui/web の生成型は各ディレクトリの中で `corepack pnpm install --offline` → `corepack pnpm gen:types`（`-C` だと root の pnpm 版が使われ ERR_PNPM_BAD_PM_VERSION）。

## 未解決事項

- task の acceptance（`Check::Command`）には `scope` を持たせていない（237 箇所の pattern に波及するため）。人や CoS が task の acceptance に
  範囲 check を書くと従来どおり final review で流れる。planner 指示では「task の acceptance に範囲 check を書かず leaf の `scope: true` に置く」とした。
- `scope` の無い既存の計画（走行中の task）は従来どおり統合でも全 check が流れる。replan で planner が新しい指示を読めば直る。
- `docs/SPEC.md` には WU の checks・統合の検査を規定する節が無く、SPEC は変えていない（guide と ADR に書いた）。
- run の環境への受け渡しは `with_env` を持つ adapter だけ。偽 adapter の試験は `prepare_run_adapter` までで、dispatch を通した run 環境の試験は無い。

## 提案

- `Check::Command` にも種類（scope）を持たせ、task の acceptance の範囲 check を final review で機械的に飛ばす（または review 前の target 同期の後に
  `merge-base` 基点へ書き換える `review_view` の経路で扱う）かは、本番で acceptance 側の誤検出が続くなら別 task で。
