---
title: P0-E reviewer・pilot 受け入れの検証強化
tasks: [01M6N7XY9R6FREMRKFFVS4ENMG]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# P0-E reviewer・pilot 受け入れの検証強化

完了日: 2026-11-05。親は 01M6N7WD1WXXW0A5FV7RVZ67B7。基準は `git merge-base HEAD main`。P0-C（bench v2）を含む main へ rebase 済み。設計は ADR `agent-docs/adr/2026-11-05-phase3-p0e.md`。担うのは不変条件 I7〜I9、ケース K3・K21・K22。

## 結論
- 依頼文の Outcome block に任意の `checks`（`{cmd, expect_exit}`）を足した。Rust が `launch_root` の効果を作るとき、対象 Outcome の `checks` を root の acceptance に `Check::Command` として足す。TS は command を作らず、snapshot の JSON にも出さない（TS の bundle・schema は不変）。
- review の `Check::Command` の結果を新しい event `review_command_checked`（acceptance の index・command・期待 exit・exit・timeout・合否）に、`review_verdict` と同じ transaction で残す。
- pilot の driver は全 Outcome に公開契約の check を付け、facts の `roots[].review` と ops の `reviews` に、reviewer の判断の理由・検査の出典と exit・食い違いを写す。
- stub reviewer の「全基準を無条件に pass」を消した。確かめられる 2 種類の基準（Outcome の成果、delivery の取り込みの判定）だけを pass にできる。
- 本物の TS と本物の shell で、検査が落ちている間は reviewer が合格を返しても root は done にならず、controller も達成にしないことを固定した（K3）。実 daemon の stub pilot でも、成果を書かない worker では達成にならず、facts と ops に exit 1 と出典が残る。

## 受入条件との対応
| 受入 | ケース | 試験・証拠 |
|---|---|---|
| 0 check fail が reviewer pass に優先し、root done・達成にならない | K3 | `pilot_review_check_failed_check_blocks_root_and_achievement`（本物の TS の controller、本物の `review_task`、reviewer の判定は台本で合格。試行を使い切るまで done・`pc/achieved` なし。次の試行で成果を置くと done → achieved の対照つき）。既存の `failed_command_check_wins_over_reviewer_pass`。実 daemon の段 2c（`test-long-horizon-pilot.sh`、成果なしの stub で root 2 本とも failed、`pc/achieved` なし） |
| 1 理由・出典・exit を facts/ops/event に残す | K22 | event: `command_check_record_is_written_with_the_verdicts`・`done_then_command_review_passes_and_events_are_recorded`、`review/tests.rs` の `command_checks_are_recorded_with_exit_codes`・`command_check_timeout_is_recorded_without_exit`・`acceptance_command_shared_with_a_repo_check_keeps_the_record`。facts・ops: `test-long-horizon-pilot.sh` の段 2 と 2c の検査。raw は `p0e/stub/`・`p0e/stub-no-output/` |
| 2 stub reviewer が検証できない基準を無条件 pass しない。check 無しで保証できない範囲を文書に | K21 | `scripts/dev/long-horizon-pilot/test-stub-reviewer.sh`（Outcome を名指さない基準・check なし・check 不合格・成果なし・未 commit・取り込みの判定の逆向きと空、の各陰性と陽性）。文書は ADR の「check の無い経路で保証できないこと」 |
| 3 `test -f agent-docs/progress/2026-11-05-phase3/p0e.md` | — | 下の証拠 |

その他の試験: `outcome_block_checks_are_parsed_and_hidden_from_the_snapshot_json`・`outcome_block_rejects_bad_checks`（task-core）、`launch_root_spec_adds_the_checks_of_the_named_outcomes_only`（task-ops）、`pilot_review_check_outcome_without_checks_stays_reviewer_only`（check の無い Outcome は reviewer だけのまま）。

## 証拠
すべて提出時の HEAD（P0-C を含む main へ rebase した後、差し戻しの修正を commit した後）で実行した。

| コマンド | exit | 要点 |
|---|---|---|
| `bash scripts/dev/test-affected.sh`（`CELERIS_WU_BASE` を外し、基準は `git merge-base HEAD main`） | 0 | nextest 7638 run・7638 passed・21 skipped。web の tsc -b と unit（vitest 104 files・709 passed）。script bundle SKIP 0 件 |
| `bash scripts/dev/test-affected.sh`（WU の基準 `$CELERIS_WU_BASE`） | 0 | 試験対象の変更なし（この修正は script と文書だけ） |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告は vendor の deno_core の既存 1 件だけ |
| `cargo fmt --all -- --check` | 0 | 差分なし |
| `bash scripts/dev/test-long-horizon-pilot.sh --out "${TMPDIR:-/tmp}/lh-p0e"` | 0 | dispatch 7・api 3・review_check 2 passed、stub reviewer OK、virtual raw ok（7 scenarios）、stub daemon raw ok、C06 effect ok、no-output ok |
| `sh scripts/dev/long-horizon-pilot/test-stub-reviewer.sh` | 0 | `test-stub-reviewer: OK`（全 19 Outcome の混入検査と、検査の関数の陰性・陽性を含む） |
| `CELERIS_SCRIPT_BUNDLE_DIR=<bundle> CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p task-core -p task-ops -p task-dispatch --lib -E 'test(/pilot_review_check\|command_check\|outcome_block\|launch_root_spec\|acceptance_command_shared\|review_check_wins/)'` | 0 | 15 passed |
| `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |
| `test -f agent-docs/progress/2026-11-05-phase3/p0e.md` | 0 | — |
| `sh scripts/dev/long-horizon-pilot/test-git-seed.sh` | 1 | `FAIL: out/o1.txt is not on main`。main（P0-C の後）でも同じく落ちる（P0-E の変更が原因ではない）。未解決 8 |

TS（`control-ts/`）は変えていない。TS policy だけの変更の verify の条件（cargo 0、p50≤30 秒・p95≤60 秒）は、この task の変更が Rust を含むので当てはまらない。

## raw（`p0e/`）
- `stub/`: C08 の `new`・`old.passive`。`new` は root 4 本が done、各 root の command check は exit 0・出典 `public/C08/case.json#oN.acceptance`、reviewer の理由は `stub reviewer: 1 command check(s) passed and HEAD:out/oN.txt has … bytes` と `stub reviewer: 1 commit(s) in <base>..<head>`。採点は new が pass、old.passive が fail（従来どおり）。`achieved_at` は 49 秒。
- `stub-no-output/`: C08 の `new`、worker が成果を書かない。root 2 本（o1 の 1 回目と再試行）とも failed、command check は 6 回とも exit 1、reviewer は「not evaluated: a deterministic check failed」、`pc/achieved` なし、採点は fail。
- `review_check.log`: K3 の nextest の出力。

## 失敗した経過と直したもの
1. 初回の stub pilot で、stub reviewer の判定が 0 件（`no verdict for criterion 0`）になった。reviewer の request は `context.review` の下にあり、作業場所は task の dir（候補の worktree はその下の `repos/<name>`）だった。読み先を直した。
2. delivery が root に足す Reviewer の基準（取り込みの判定）を stub が確かめられず、厳しくした stub では pass にならなかった。base..head の祖先関係と commit の有無で確かめる分岐を足した。
3. test-affected で 2 件落ちた: event の schema（`event.schema.json`）の再生成漏れと、event の並びを固定する既存試験 `done_then_command_review_passes_and_events_are_recorded`。schema を再生成し、試験の期待値に `review_command_checked` を足した。web の `event-kinds.ts`・`invalidation-map.ts` にも新しい種類を足した。
4. driver の facts の `achieved_at` が常に null だった（A9 からの不具合。判断の rule を `proposal_json` の `explanation` から読んでいたが、実際は `proposal.explanation` の下にある。main の `a9/stub` の raw も null）。読み先を直し、段 2 に「new 腕は `achieved_at` を持つ」を足した。

## 差し戻しの原因と修正
final review で `test-stub-reviewer.sh` と、これを呼ぶ `test-long-horizon-pilot.sh` が exit 1 だった。main の P0-C（bench v2）が `oracle/*/hidden/expected.json` を消し、rebase の後も試験がこの file を読んで FileNotFoundError になったため。前の p0e.md は 2 本の結果を取り込み前の実行のまま exit 0 と書いていた。

修正: 公開 check の混入検査の対象を現行の材料に替えた。全課題の全 Outcome の cmd に、`oracle/<case>/oracle.json` と `oracle/holdout.json` の canary、および `oracle/`・`grader`・`check_outcome` が含まれないことを確かめる。検査の関数 `leaked` が canary と場所の名前を含む cmd を検出し、`true` では検出しないことも同じ script で確かめる。driver.py・stub_harness.py・Rust・fixture は変えていない。ADR の決定 4 と「保証できないこと」の表も現行の oracle に合わせた。

## 変更（commit の件名）
- `p0e: pilot の root に公開契約の command check を結線し、検査と reviewer の根拠を event・facts・ops に残す`
- `p0e: 進捗に main の取り込みと再実行の結果を書く`
- `p0e: stub reviewer の試験を bench v2 の oracle に合わせ、ADR と進捗を直す`
- `p0e: 進捗の証拠表を提出時の HEAD の実測に直す`

## 本番への移行
config 欄・migration は無い。event の種類が 1 つ増える（追記だけ）。依頼文の block の `checks` は任意で、既存の案件の挙動は変わらない。

## 未解決
1. Roadmap の Outcome は `checks` を持たない。approved の Roadmap がある案件の root は check の無い経路になる。
2. 公開の check は、worker が repo の `minichunk.py` を書き換えたことを検出しない（HEAD の `minichunk.py` で再現するため）。採点の `grader/check_outcome.py` が別に確かめる。
3. `old.passive` の腕は CoS が root を起票するので、check が付くかは CoS 次第で、保証しない（ADR の表）。
4. `case.json` の `checks` の欄は bench v2 にも無い。全 19 Outcome の check は acceptance の `minichunk.py verify` の手順から導いている（出典は `public/<case>/case.json#<id>.acceptance`）。`driver.py` の docstring の「hidden は読まない」は旧い語のまま残した（この修正では driver.py を変えない指定）。
5. 実 LLM の reviewer が、check の結果を見て同じ判断をするかは未確認（P0-F の実測）。
6. commit 済みの raw（`p0e/stub/`・`p0e/stub-no-output/`）は P0-C の取り込み前に取ったもので、facts の `bench_version` は 1。今の fixture の pilot は `test-long-horizon-pilot.sh` が `$TMPDIR` に作り直して検査する（上の証拠）。raw は作り直していない。
7. 他の stub reviewer（`crates/campaign/tests/launcher.rs` の全基準 pass、`crates/celeris/tests/trial_context_canary_e2e.rs`）は long-horizon pilot の外の試験用の台本で、変えていない。
8. `scripts/dev/long-horizon-pilot/test-git-seed.sh` は main でも exit 1。P0-C で stub は acceptance の `minichunk.py verify` の手順だけで成果を書くようになったが、この試験は `minichunk.py` の無い seed と手順の無い基準（`c-o1: x`）を渡すので、stub は何も書かない。`test-long-horizon-pilot.sh` からは呼ばれない。この修正の範囲（test-stub-reviewer.sh と文書）の外なので直していない。

## 提案
- `test-git-seed.sh` の seed に `minichunk.py` を入れ、基準に verify の手順を書いて bench v2 に合わせる（P0-C の後続か P2-J）。
- Roadmap の Outcome に `checks` を持たせ、承認の時点で版に固定する（P0-B か P2-J）。
- `case.json` の Outcome に `checks` を置けば、driver がそのまま root に付ける（出典は `public/<case>/case.json#<id>.checks`）。P0-C の公開の自己検査 `python3 minichunk.py check <case> <id>` を使うなら、ここに置く。
- `docs/ops/project-controller.md` に block の `checks` と「check の無い経路で保証できないこと」を 1 節足す（P2-J がまとめる）。

## 次の gate
P0-F（人の許可の後の実 LLM 再試験）。P0-B・P0-C・P0-D と合わせて完了した後に、人が上限・account・費用を許可する。
