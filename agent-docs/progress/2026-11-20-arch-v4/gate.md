---
title: アーキ v4 親 closeout gate — H の判定経路・反例の試験と本物の検査器の probe を HEAD で再実行し、H を含む release の gate を release の記録から確かめる
tasks: [01M7MZT8KX547464YYS76QNP4G]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# gate（WorkUnit、親の closeout の remeasure）

## 0. 結論

- H の [replan/gate.tsv](../2026-11-19-arch-v4-h/replan/gate.tsv) の 17 行を、[replan/gate.md](../2026-11-19-arch-v4-h/replan/gate.md) §7 の再現の手順どおり HEAD で再実行した。
  17 行とも結果は pass で、H の結果と一致した（match は 17 行とも yes、no 0・explained 0）。各行は [gate.tsv](gate.tsv)（列 `key h_result result match note`）。
- 統合後の release の gate は pass（key `release_gate`）。release `54e61984357f`（main の commit）は、H の範囲で `crates/`・`scripts/`・`web/`・`Cargo.toml`・`Cargo.lock` を最後に変えた
  main の commit（件名 `integrate wu/r6-release (phase fix2)`）を祖先に持つ。その `gate.json` は `ok: true`・`failed_step: ""`、`verify.json` は `ok: true`・`live_ok: true`、
  `promoted.json` は 2026-11-20T15:44:15Z の昇格（`status: succeeded`、actor は外部の operator）を記録している。
- 昇格したのはこの WorkUnit ではない。この WorkUnit は記録を読んだだけで、`release.sh`・`verify.sh`・`promote.sh` は走らせず、本番の state には書いていない。
- probe はそれぞれ検査器を走らせた直後に `git checkout --` で戻し、commit していない。戻した後の `git status --porcelain -- crates scripts web` は 2 回とも空だった。
- source と policy の file は変えていない。変えた file は `agent-docs/progress/2026-11-20-arch-v4/gate.tsv`・`gate.md` の 2 つだけ。
- userns の要る試験と全体の試験は走らせていない。

## 1. 対象と条件

- 対象の tree: この WorkUnit の HEAD（件名 `integrate wu/sync (phase sync)`。子 A〜H の入った main を親の branch に取り込んだ後）。
- `git merge-base HEAD main` は H の R9 の後の main（件名 `integrate wu/publish (phase publish)`、release `54e61984357f` と同じ commit）。
  `git diff "$(git merge-base HEAD main)" HEAD` は空で、HEAD の tree はこの main と同じ。
- 検査器 `scripts/dev/repo-health` は、H の health の base（件名 `integrate wu/close (phase close)` の G までの main、release `1680de5cd1b4` の commit）とも、
  いまの merge-base とも同じ内容（`git diff --quiet <base> HEAD -- scripts/dev/repo-health` が 0）。
- host は home-dev、時刻は 2026-11-20T15:51Z〜16:05Z、開始時の 1 分 load は 3.45（途中の最大 5.14）。並行の deps・loc・store の WorkUnit も同じ host で動いていた。
- nextest には `--no-tests=fail` を付けた（filter に合う試験が 0 件なら落ちる）。summary の `skipped` は filter から外れた試験の数で、実行時の skip ではない。

## 2. 試験の再実行

| key | コマンド | exit | 件数 | 時間 |
|---|---|---|---|---|
| case1〜case6・prompt_boundary・prompt_full_objective・planner_prompt_acceptance | `cargo nextest run -p task-dispatch -p task-worker -E 'test(/boundary_case_\|prompt_boundary_\|prompt_full_objective_\|planner_prompt_acceptance_/)' --status-level pass --final-status-level pass --no-tests=fail` | 0 | 16 tests run: 16 passed, 3224 skipped | build 込み 115 秒、試験 10.96 秒 |
| org_write_contract | `cargo nextest run -p task-api --test cos_delivery_suite org_write_contract --status-level pass --final-status-level pass --no-tests=fail` | 0 | 6 tests run: 6 passed, 44 skipped | build 込み 64 秒 |
| tx_boundary_contract | `cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract --status-level pass --final-status-level pass --no-tests=fail` | 0 | 8 tests run: 8 passed, 42 skipped | 2 秒 |
| dispatcher_cex | `cargo nextest run -p task-dispatch -E 'test(cex_) \| test(write_set_) \| test(store_access_guard) \| test(structure_guard)' --no-tests=fail` | 0 | 67 tests run: 67 passed, 1854 skipped | build 込み 104 秒 |
| repo_health_tests | `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'` | 0 | Ran 126 tests、OK | 55 秒 |
| clippy | `cargo clippy --workspace -- -D warnings` | 0 | — | 53 秒 |
| fmt | `cargo fmt --all -- --check` | 0 | 出力 0 行 | 8 秒 |

check 1 の 16 件は H の gate.md §2 の表と同じ試験名だった。各試験の時間は [gate.tsv](gate.tsv) の note にある。
dispatcher_cex の 67 件の内訳は `cargo nextest list` で数えた: `cex_` 40（review_authority 19・lease_restart 12・late_completion 6・same_tick 3）、`write_set_` 13、
`structure_guard` 14（`store_access_guard` 3 を含む）。H と同じ内訳である。

H（replan/gate.md・gate.tsv）との件数の比較:

| 項目 | H | 今回 |
|---|---|---|
| boundary_case_ | 6 passed | 6 passed |
| prompt_boundary_・prompt_full_objective_・planner_prompt_acceptance_ | 3 + 4 + 3 passed | 3 + 4 + 3 passed |
| repo-health の python 試験 | 126 OK | 126 OK |
| org_write_contract・tx_boundary_contract・task-dispatch の filter | 6・8・67 passed | 6・8・67 passed |
| clippy・fmt | exit 0・exit 0 | exit 0・exit 0 |

## 3. 本物の検査器に当てた probe

起動の引数は H の gate.md §3 と同じ（dispatcher の health の関門と同じ組で、head だけを `worktree` にする）。probe を入れた file も同じ `crates/task-api/src/lifecycle.rs`。
HEAD の `lifecycle.rs` には `use crate::knowledge` の行が無く（`knowledge` は 12 行目の doc comment にだけある）、組 `lifecycle -> knowledge` はまだ循環の内部に無いので、H の probe a をそのまま使えた。

| run | base | exit | verdict | findings | new | worsened | existing | deps.cycle | deps.violation | 時間 |
|---|---|---|---|---|---|---|---|---|---|---|
| probe a（probe_scc_edge） | `git merge-base HEAD main` | 1 | fail | 696 | 1 | 0 | 695 | fail（existing 17 + new 1） | pass（0 件） | 92 秒 |
| probe b（probe_plain） | `git merge-base HEAD main` | 0 | pass | 695 | 0 | 0 | 695 | pass（existing 17） | pass（0 件） | 86 秒 |
| probe なし（参考）、head は `HEAD` | H の base（G までの main） | 0 | pass | 695 | 0 | 0 | 695 | pass（existing 17） | pass（0 件） | 124 秒 |

- probe a の new の finding は H と同じ: rule_id `deps.cycle`、severity `error`、scope `module` / `crates/task-api/src/lifecycle`、
  evidence の symbol `crates/task-api/src/lifecycle -> crates/task-api/src/knowledge`（`crates/task-api/src/lifecycle.rs:26` `use crate::knowledge::KnowledgeItem;`）。
  fingerprint は `deps.cycle:` の後ろの 16 桁の hex を 4 桁ずつ区切ると `a106-75aa-c992-c815` で、H と同じ値（区切らない値は成果物 dir の `raw/health-probe-scc-edge.json` にある）。
  rationale は「head の 49 単位の循環の内部に新しい組 crates/task-api/src/lifecycle -> crates/task-api/src/knowledge（行 1 本）。base ではこの組は循環の内部に無かった。節点の数が変わらなくても循環の内部の依存が増えた。」で、fail の規則は `deps.cycle` だけ（21 規則のうち 20 が pass）。
- probe b は 21 規則がすべて pass。`deps.*` の new・worsened の finding は出なかった。
- H との違い: H の probe の base は G までの main だった。いまの `git merge-base HEAD main` は H を含む main なので、base が違う。
  base と HEAD の間に差分が無いため、H の「probe なしの run」と同じ意味の確認（H の R1〜R7 の差分が G までの main に対して new・worsened を出さない）は、
  base を H の base（件名 `integrate wu/close (phase close)` の commit）、head を `HEAD` にして走らせた（表の 3 行目）。結果は H と同じく exit 0・new 0。

## 4. release の gate（key `release_gate`）

### 4.1 対象の commit

```sh
R9=$(git log --format=%H --grep='^arch-v4 H R9 publish: 進捗の本体の証拠から旧い担当の語句の引用を消す$' main)
L=$(git log -1 --format=%H "$R9" -- crates scripts web Cargo.toml Cargo.lock)
git log -1 --format='%h %s' "$L"   # integrate wu/r6-release (phase fix2)
```

`L` は main の commit で、件名は `integrate wu/r6-release (phase fix2)`（2026-11-20T09:44Z）。H の R9（publish）は docs だけを変えたので、H の code の最後の変更はこの commit である。

### 4.2 release の記録（読むだけ）

```sh
for d in /local/celeris/state/releases/*/; do s=$(basename "$d")
  git merge-base --is-ancestor "$L" "$s" && echo "$s contains"; done
```

| release | `L` を祖先に持つ | gate.json `ok` / `failed_step` | verify.json `ok` / `live_ok` | promoted.json |
|---|---|---|---|---|
| `54e61984357f` | はい | `true` / `""`（2026-11-20T14:39:57Z） | `true` / `true`（2026-11-20T14:40:38Z） | 2026-11-20T15:44:15Z、`from` `1680de5cd1b4`、`status: succeeded`、`included.tasks` に H（01M7MZTKBPZPQE4XN451F8X84F） |
| `1680de5cd1b4`・`7014d24222c4`・`bb772ffca8ec` | いいえ | — | — | — |

- `/local/celeris/state/current` は `releases/54e61984357f` を指す（2026-11-20 の確認時）。
- gate.json の `steps` は 17 step とも exit 0（cargo-fmt-check・cargo-test・script-bundle-tests・cargo-clippy・source-size-report・cargo-build・web の typecheck・test・e2e など）。
  `cargo_test` は `nextest_passed` 8564・`nextest_skipped` 22・`failed` 0・`doc_passed` 3・`userns: true`。gate log `gate-logs/cargo-test.log` の Summary 行は
  `8564 tests run: 8564 passed (12 slow), 22 skipped` で、`nextest_passed` と等しい。
- verify.json の checks（start-and-migrate・counts-match・main-gets・n-1-compat・smoke など）は ok、schema_version は 126 → 126。
- 未確認: R6 が足した要約の `release-records.jsonl` は `/local/celeris/state/logs`・`/local/celeris/state/backups` に見つからなかった。この WorkUnit の範囲の外なので、それ以上は調べていない。

result は pass。match は yes とした。H の `not_run` は、H の記録の時点で H を含む release が無かったためで、そのあと人の決めた運用どおり release が作られ gate を通った。

## 5. 戻したことと範囲の確認

```sh
git status --porcelain -- crates scripts web                     # probe a の後・probe b の後とも空
git diff --name-only "$CELERIS_WU_BASE" -- crates scripts web    # 空
sh "$CELERIS_WU_SCOPE_PATHS"                                     # gate.md・gate.tsv だけ
```

## 6. 回帰条件

- H の replan/gate.md §6 と同じ: `boundary_case_` 6 件、`prompt_boundary_` 3 件、`prompt_full_objective_` 4 件、`planner_prompt_acceptance_` 3 件、`test_repo_health_*.py` 126 件、
  org_write_contract 6 件・tx_boundary_contract 8 件・task-dispatch の filter 67 件が通り続けること。
- §3 の probe a で `deps.cycle` の new の組 finding が出て exit 1、probe b で `deps.*` の new・worsened が 0 件のまま exit 0 になること。
  main で `lifecycle -> knowledge` の組が SCC の内部にできた後は probe a は使えない。そのときは H の前回の gate.md §3 の手順で、まだ無い組を選び直す。
- H を含む以後の release の gate.json が `ok: true` であること。

## 再現コマンド

```sh
cargo nextest run -p task-dispatch -p task-worker -E 'test(/boundary_case_|prompt_boundary_|prompt_full_objective_|planner_prompt_acceptance_/)' --status-level pass --final-status-level pass --no-tests=fail
cargo nextest run -p task-api --test cos_delivery_suite org_write_contract --no-tests=fail
cargo nextest run -p task-api --test cos_delivery_suite tx_boundary_contract --no-tests=fail
cargo nextest run -p task-dispatch -E 'test(cex_) | test(write_set_) | test(store_access_guard) | test(structure_guard)' --no-tests=fail
cargo nextest list -p task-dispatch -E 'test(cex_) | test(write_set_) | test(store_access_guard) | test(structure_guard)'
python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_*.py'
cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check
B=$(git merge-base HEAD main)
F=crates/task-api/src/lifecycle.rs
# probe a
sed -i 's/^use crate::human_epoch::HumanEpoch;$/&\nuse crate::knowledge::KnowledgeItem;/' $F
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out $TMPDIR/a.json; echo $?   # 1
git checkout -- $F
# probe b
sed -i 's|^use crate::human_epoch::HumanEpoch;$|&\n// probe: comment only (gate-cases)|' $F
python3 scripts/dev/repo-health/repo_health.py --repo . --base "$B" --head worktree --policy-version 1 --out $TMPDIR/b.json; echo $?   # 0
git checkout -- $F
# probe なし（参考）: base は H の base（G までの main。release 1680de5cd1b4 と同じ commit、件名 integrate wu/close (phase close)）
python3 scripts/dev/repo-health/repo_health.py --repo . --base 1680de5cd1b4c57d5df3ceeab59d03a3783a0f6c --head HEAD --policy-version 1 --out $TMPDIR/clean.json; echo $?   # 0
# release の gate（読むだけ。§4 の 2 つのコマンドの後）
python3 -c 'import json;g=json.load(open("/local/celeris/state/releases/54e61984357f/gate.json"));print(g["ok"],repr(g["failed_step"]))'
```

生の記録（この run の成果物 dir の `raw/`）: `check1.log`、`org_write_contract.log`、`tx_boundary_contract.log`、`dispatcher_cex.log`・`dispatcher_cex.list`、
`repo_health_tests.log`、`clippy.log`、`fmt.log`、`health-probe-scc-edge.{json,log}`、`health-probe-plain.{json,log}`、`health-clean-vs-g.{json,log}`、
`probe_scc_edge.diff`、`probe_plain.diff`。
