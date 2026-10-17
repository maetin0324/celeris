---
title: "MLSys pilot-2 で見つかった欠け 6 件の修正"
tasks: [01M5456VVZR2VC5FC6P5J0V12Q]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# pilot-2 の欠け 6 件

ADR: `agent-docs/adr/2026-10-17-mlsys-pilot2-fixes.md`。結果の元: `/local/celeris/data/scratch/mlsys-pilot-2/RESULTS.md`。
有料実行・本測定はしていない（mock / 一時 HTTP server / stub 実行ファイル / 一時 dir だけ）。worker は LLM CLI を起動していない。

| # | 欠け | 変更 | 試験 |
|---|---|---|---|
| 1 | 外部入力が届かない | `--inputs-src`、`inputs.rs`（取り込み・sha256 照合・全腕の `workspace/inputs/<name>` へ配送）、試行記録 `inputs`、preflight `inputs` 検査、case.json の fetch 文言を実手順に | `tests/inputs_delivery.rs` 3 件、`tests/preflight.rs` の `missing_or_wrong_external_inputs_are_detected` `initial_repo_needs_repo_src` |
| 2 | A に FS 隔離なし | `harness_exec::Sandbox`（bwrap。見える物は workspace・A のアカウント dir・実行ファイル・読み取り専用 `/usr` `/etc`、空 `/tmp`、`--clearenv`）。常に有効。preflight が bwrap を実際に起動して確認 | `tests/a_arm_hardening.rs`: `sandbox_exposes_only_…`（構成）、`real_bwrap_hides_everything_outside_the_workspace`（実 bwrap: 外の file 読めず・隣の dir 見えず・token 環境変数 unset・/tmp 書き込みが host に出ない） |
| 3 | per_case が A を縛らない | `--output-format stream-json`、message.id ごとの累積 tokens、監視 thread（1 秒）と `poll` が上限超過で process group を kill → cancelled。`Executor::usage` で runner も毎 poll で判定 | `a_arm_is_stopped_while_running_when_it_passes_the_token_cap` `a_arm_under_the_cap_runs_to_the_end` `live_usage_counts_each_message_once` |
| 4 | preflight が 422 template を通す | preflight が task-api に GET して `check_project_rules`（選ばれる repo にリモートがあれば拒否、`auto_integrate` が true の案件は拒否）。専用案件の用意手順は `run-final.md` §0.5 | `tests/preflight.rs` の `template_that_would_422_is_rejected` `auto_integrate_project_is_rejected` |
| 5 | 未実行でも complete=true | checker `Totals.unexecuted`（executor_error・予算で未投入）→ `incomplete_reasons`。export は `complete=false` `real_performance=false` で `table.incomplete.*` のみ | `tests/unexecuted_incomplete.rs`、`mock_e2e` を新規則に更新 |
| 6 | A の消費が見えない | `--a-account`（claude_oauth の管理アカウント。`GET /llm/accounts` の `dir` を `CLAUDE_CONFIG_DIR` に）、観測記録 `source=claude_oauth:<id>`、`celeris-campaign usage`（7 日使用率と腕ごとの tokens、上限 claude 0.45 / codex 0.21） | `tests/usage_report.rs`、preflight `a_arm_needs_a_managed_account_and_a_working_sandbox` |

## 本測定 case の確認（k2 / k4）
- k4（`main-v3-n2-43706.tgz`）は k3 と同じ問題（外部入力）。同じ機構で解ける。人が実物を `--inputs-src` に置く。
- k2 は外部入力ではなく初期 repo（`initial`）。`--repo-src` に展開済みの木を置く（preflight は `initial` を持つ case で `--repo-src` を必須にした。以前は任意で、無くても通っていた）。k2 の submodule 実体は case.json の記述どおり承認待ち／未実施。
- grader は展開後の `inputs/` を見る。展開（`cd inputs && tar -xzf …`）は試行の仕事。`no_artifact` 判定は `inputs/` を除いた木で行う。

## 旧い仕組みの削除
- `campaign asset store` の記述（case.json の fetch）、`--output-format json` 起動、無隔離・既定 `~/.claude` の A 起動。`HarnessExecutor::new` は stub 実行ファイル試験専用で、CLI は必ず `with_sandbox` + `with_budget_guard` を付ける。
- ADR 2026-10-16-mlsys-pilot-wiring-fixes D4 の「sandbox を足さない」は D2 で置き換え（その ADR に注記）。

## 本番への移行
- config 欄・DB の変更は無い。次を人が行う: (1) campaign 専用案件と空 repo を作る（`run-final.md` §0.5。`POST /projects`、`POST /projects/{id}/repos`）。(2) `task-template.json` の `project_id` と `repos` を専用案件に。(3) `--inputs-src`・`--a-account` を付けて preflight → run。(4) `bwrap` が運用 host で動くこと（preflight が確認）。
- 6c の `§5`（`2026-10-17-mlsys-audit-fix-6c.md`）はこの branch の base に無い（別 branch の progress）。手順は `run-final.md`（§0.5/2.5/3/4/4.5）に集約した。6c を再実行するときは同じ flag を足す。
- pilot-2 の `pilot2.json` はそのまま使える（manifest・hash は変えていない。case.json の fetch 文言の変更は manifest の固定対象外）。

## 区分
- 実装済み・mock／stub／一時 HTTP server で検証済み: 1〜6 すべて。
- 未実施（人・有料）: 実 claude が bwrap 内で `CLAUDE_CONFIG_DIR` のアカウントで動くか（`/etc` を読み取り専用で見せる前提。claude の実行ファイルが node 等の追加 path を要する形なら bind の追加が要る）、実 task-api への preflight GET、B/C の再実行。
- B/C のファイルシステム隔離: campaign 側は足していない。Celeris の worker sandbox（ADR-0095）に従う。この task では B/C worker が `out/` の外を読めるかを実測していない（未確認）。

## 未解決・提案
- 提案: B/C worker についても A と同じ「workspace の外が読めない」実測試験（Celeris の本物の worker を使うので有料でない一時 daemon + stub adapter）を足す。
- 提案: k2 の初期 tree と submodule snapshot の配置と hash 固定（`--repo-src` の中身を hash で検査する）。
- 注意: `real_bwrap_…` 試験は userns が作れない sandbox では skip する（`CELERIS_USERNS_TESTS=require` なら失敗）。

## 証拠
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0（passed 5612, failed 0, ignored 15, nextest_exit 0, doctest_exit 0）。全出力は run の `artifacts/test-parallel.log`。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
