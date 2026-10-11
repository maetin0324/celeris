---
title: WorkUnit の受け入れ検査ログを作業ツリーの外に置く（scope check の自己汚染を防ぐ）
tasks: [01M4NV0KJQ2A1ZY49CTPBQN1C7]
status: running
updated: 2026-10-11
---

# WorkUnit の受け入れ検査ログを作業ツリーの外に置く

verdict: reproduced（WU の検査ログ `work-unit-checks`）／ integration-checks は not-reproduced

## 再現（WU repro-fix）

試験（`crates/task-dispatch/src/dispatcher/tests/scope_checks.rs`）。fixture の `.gitignore` は
`.taskd/ runs/ inputs/ artifacts/` だけで、`work-unit-checks/` と `integration-checks/` は入れない。

- `wu_check_log_outside_tree_shared_mode_scope_check_ignores_check_logs` — shared mode（task_dir = repo の作業ツリー）。
  WU `a`（段 build）と WU `c`（段 next）の範囲 check（`git diff` + `git ls-files --others --exclude-standard`）が
  自分の検査ログも前の WU の検査ログも範囲外と数えないこと、ログが `<workspace_root>/<task_id>/work-unit-checks/<key>` にあり
  作業ツリーに無いことを見る。
- `wu_check_log_outside_tree_second_stage_ignores_integration_check_logs` — worktree mode。1 段目の統合の検査が走って
  ログを残し、2 段目の WU `c` の範囲 check がそれを数えないこと、ログ dir がどの task 作業ツリーにも入っていないことを見る。

修正前（base `130f9f34` のコード + 新しい試験）:

```
$ cargo nextest run -p task-dispatch --no-fail-fast wu_check_log_outside_tree   # exit 100
FAIL wu_check_log_outside_tree_shared_mode_scope_check_ignores_check_logs
  WorkUnitChecksFailed key=a ... stdout_tail="out of scope:
  work-unit-checks/a/1791735294545-0.log
  work-unit-checks/a/1791735294545-1.log"   （retry ごとに増え、3 回とも落ちる）
PASS wu_check_log_outside_tree_second_stage_ignores_integration_check_logs
Summary 2 tests run: 1 passed, 1 failed
```

integration-checks が再現しない理由: 統合のログは `ws.task_dir`（`task_workspaces_for` が常に
`<workspace_root>/<task_id>` を返す）の下で、worktree mode では作業ツリーは `<task_dir>/tree` か `<task_dir>/repos/<name>`
にあるので最初から外。shared mode では `parallel_mode().worktrees` が false なので統合の検査は走らない
（`phase_integration.rs` の `let (true, Some(ws)) = ... else` で即 `IntegrationRun::default()`）。よって置き場所は変えていない。

## 決めた置き場所

葉の WU の受け入れ検査のログ = `<workspace_root>/<task_id>/work-unit-checks/<wu_key>/<started_ms>-<index>.log`
（`Dispatcher::check_log_root(task_id)`、`phase_integration.rs`）。worktree mode ではこれは今の task_dir と同じ場所。
旧い `<task_dir>/work-unit-checks`（shared mode では repo の作業ツリーの中）の経路は消した（互換分岐・切替 flag なし）。
統合の検査のログは `<workspace_root>/<task_id>/integration-checks/<wu_key>` のまま（上のとおり既に外）。

ログを読む側（task-api `GET /tasks/{id}/work-units/{key}/check-log`）は event の絶対 `log_path` を読むので、コードの変更は要らない。

## 証拠（修正後）

- `cargo nextest run -p task-dispatch wu_check_log_outside_tree` → exit 0、2 passed
- `cargo nextest run -p task-dispatch` → exit 0、951 passed / 0 failed
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0
- 既存試験の追随: `scope_checks.rs` の shared fixture `.gitignore` から `work-unit-checks/` 行を削除、
  `drain_hand_off.rs` は WU 検査ログが `<root>/<task_id>/work-unit-checks/a` の下にあること、
  `integration_check_progress.rs` は統合ログが `<root>/<task_id>/integration-checks/integrate-build` の下にあることを assert。

## 未解決事項（後続 WU readers-docs / verify）

- 文書の旧記述: `agent-docs/adr/0040-self-improvement-deploy.md:553`（`<task_dir>/work-unit-checks/<wu_key>/`）、
  `agent-docs/progress/2026-10-04-drain-hand-off.md:30`。ADR 付記に置き場所の決定を書く。
- `crates/task-api/tests/files.rs:616-617` は `env.workspace(&task)` の下に WU ログを置く fixture（event の path を読むだけなので合格はするが、新しい置き場所に合わせるとよい）。
- WU preamble（crates/task-worker）の範囲 check 説明との突き合わせ。
- workspace 全体の `bash scripts/dev/test-parallel.sh` と `cargo clippy --workspace -- -D warnings` は verify WU で。

## 提案

- shared mode では `runs/` `inputs/` `artifacts/` `.taskd/` も task_dir = repo の作業ツリーに書かれ、repo の `.gitignore`
  頼みで範囲 check・`git status` を汚さずにいる（試験 fixture も `.gitignore` で隠している）。検査ログと同じく
  `<workspace_root>/<task_id>` に出すかを別 task で決めるとよい（run の作業ディレクトリ・worker の契約に関わるので ADR が要る）。

## 本番への移行

なし（config・DB の変更は無い。旧い置き場所に残った過去のログはそのまま。消すかどうかは人の判断）。
