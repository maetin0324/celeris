---
title: 健全性v2 G2 dispatch policy-no-reviewer — reviewer run の無い final review にも policy の関門を掛ける
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch policy-no-reviewer — reviewer run の無い final review にも policy の関門を掛ける

完了日: 2026-11-08

WorkUnit `policy-no-reviewer`（final review の差し戻し (c) の修正）。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health・test-affected）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の I8・D6.4 と「fallback: 新しく置かない。読めないときは fail closed」。不明の扱いは先行の WorkUnit [policy-unknown](policy-unknown.md) の実装を使う。

## 変更

件名「G2 policy-no-reviewer: reviewer run の無い final review にも policy の関門を掛ける」の commit。

- `crates/task-dispatch/src/health_review/input.rs`（材料作り）:
  - `Dispatcher::health_job` の引数を `(task, synced, run_id, reviewer)` にした（`remote` は task の `workspace` から自分で決める）。reviewer run があれば従来どおり。reviewer run が無ければ `policy_only_job` が、review 前の同期（ADR-0118）をした candidate のうち `ReviewerPassReused` の無いもの（reviewer の合格の再利用でないもの）を対象に、`policy_only` の仕事を作る（`review_run` は worker run の id）。`reviewer_reuse` の回は従来どおり作らない。event を読めないときは全 candidate を対象にする（fail closed）。
  - `HealthInputWork` に `policy_only` を足した（struct の欄。config 欄・Event・状態ではない）。`build_health_input` は `policy_only` なら candidate ごとに `record_policy_only` を呼び、常に `None` を返す（`ReviewRequest` には何も渡さない）。
  - `record_policy_only`: 同じ candidate `(repo, target_sha, reviewed_sha)` の既知の policy の状態が記録済みなら何もしない（repo health は candidate ごとに 1 回）。無ければ base の検査器（`with_base_checker`。I3）で report を 1 回走らせ、`HealthReviewInputSkipped`（`reason` は定数 `NO_REVIEWER`、`policy` 付き）に残す。report を作れない・base の検査器を出せないときは不明（`source = unavailable`。policy-unknown の扱い）。base に検査器が無い repo は関門が無いので何も残さない。方針（`--print-review-policy`）は reviewer の材料（owner 地図・不変条件・定義の hash）にしか使わないので読まない。
  - `HealthInputJob::attach`: 仕事があれば reviewer run の有無に依らず `spawn_blocking` で走らせる（review の非同期 task の中。tick は止めない）。reviewer run があるときだけ `run.health` に入れる。
  - `find_recorded_policy` は `HealthReviewInputSkipped.policy` に加えて `HealthReviewInputBuilt.input.policy` の既知の状態も読む（関門の `candidate_policy` と同じ範囲）。
  - `reused_from`（`ReviewerPassReused` の元の candidate）を置き、関門と `policy_only_job` が共有する。
- `crates/task-dispatch/src/health_review/policy_review.rs`（D6.4 の関門）:
  - `policy_review_gate` は `Check::Reviewer` の条件が無い task でも掛かる（以前は条件が無ければ何もしなかった）。binding（review 前の同期の candidate）が無い review では従来どおり何もしない。
  - `policy_changes`: reviewer run の無い review は、`ReviewerPassReused` があれば元の candidate（従来どおり）、無ければその candidate の記録を見る。
  - 承認の子の本文: reviewer の承認の理由が無い変更は「This candidate changes the repo-health policy files.」で始める（reviewer の承認があるときは従来の文のまま）。
- verdict を付ける criterion の選び方（`policy_criterion`）:
  - `Check::Reviewer` の条件があればその条件（従来どおり）。
  - 無ければ、この review の verdict の最大の `criterion_idx` の次（acceptance の外。verdict が無ければ `acceptance.len()`）。
  - 理由: `Check::Command` の条件に付けると、却下の不合格を `try_review_repair` が cmd で分類し、局所修復（fmt・clippy 等）に化けうる。acceptance の外の不合格は `try_review_repair` が `Check::Human` と同じに扱い（修復しない = substantive → ReviewFail → replan）、`review_discrepancy` の対象（reviewer の条件）にもならない。最大の次なので、delivery が足す reviewer の条件（`acceptance.len()`）・plan・必須検査・research の暗黙の条件と同じ index にならない。
- `crates/task-dispatch/src/dispatcher/review_spawn.rs`: `health_job` の呼び出しの 1 行の引数（`remote_workspace` → `&run_id`）だけを変えた。`dispatcher/` の本体の行数は変えていない（check-health の worsened=0）。
- `docs/api/v1/api.md`: `health_review_input_skipped` の説明に、reviewer run の無い review の記録（`review_run` は worker run、同じ candidate の既知の記録があれば残さない）を書いた。Event の欄・種類は変えていないので生成物は変わらない。
- 新しい Event の種類・状態・config 欄・切替 flag は足していない。`scripts/dev/repo-health/`・`scripts/dev/check-health.sh` は変えていない。dispatcher から LLM は呼ばない。
- reviewer の無い review の時間: reviewer run の無い final review は、review の非同期 task の中で検査（`review_task`）の前に base の検査器の report を待つ（`HEALTH_INPUT_TIMEOUT` 600 秒が上限）。base に検査器がある repo（この repo など）では、`Check::Reviewer` の無い task の final review が repo health の実行時間だけ遅れる。同じ candidate の 2 回目以降（承認の後のやり直し・再起動の後）は記録を使うので遅れない。base に検査器が無い repo は git の 2 回（`rev-parse`・`cat-file`）だけ。

## 試験

- 新規 `crates/task-dispatch/src/health_review/policy_review/no_reviewer_tests.rs`（5 件。policy-unknown の fixture `unknown_tests.rs` の `case` を `case_for(checker, changed, reviewer)` に広げて使う。`reviewer = false` は command の条件だけの task で、材料作りは `policy_only`）:
  - `health_review_policy_no_reviewer_policy_change_and_unknown_wait_for_a_human`（受け入れ条件 0）: policy の file だけを変えた candidate（方針 present・absent・`--print-review-policy` の失敗の 3 通り）と、report を書かない・壊れた report の candidate で、`HealthReviewInputBuilt` は無く `HealthReviewInputSkipped`（`review_run = worker`、reason は `NO_REVIEWER`）が 1 件。command の合格だけの outcome に承認の子を 1 件作り、`pending_children` が 1、`resolve_human_approvals` が `None`（合格しない）。同じ candidate の再判定で子を作り直さない。
  - `health_review_policy_no_reviewer_approval_passes_and_rejection_replans`（受け入れ条件 0）: 承認で `criterion_idx = acceptance.len()` に `health:policy approved by a human: <file>` の合格の verdict が付き、全部合格。却下の不合格は acceptance の外に付き、`try_review_repair` が `None`（修復にしない → replan）。
  - `health_review_policy_no_reviewer_no_checker_and_normal_change_pass_without_approval`（受け入れ条件 1）: 検査器の無い repo は記録なし、policy 不変の通常変更は記録 1 件。どちらも承認の子なし・verdict は変わらない。
  - `health_review_policy_no_reviewer_criterion_choice`（受け入れ条件 2）: `Check::Reviewer` のある task はその条件の index（verdict が後ろにあっても）、無い task は verdict の最大の次。
  - `health_review_policy_no_reviewer_reuse_reads_the_original_candidate`: reviewer の無い review はその candidate の記録、`ReviewerPassReused` がある candidate は元の candidate の記録を見る。
- 流れ全体 `crates/task-dispatch/src/dispatcher/tests/health_review_input.rs`（`spawn_review` から `run_until_idle` まで。本物の worktree・同期・stub の `repo_health.py`。report の実行を log に数える）:
  - `health_review_policy_no_reviewer_normal_change_passes_without_approval`（受け入れ条件 1）: command の条件だけの task で通常変更は `done`、reviewer run・`HealthReviewInputBuilt` なし、repo health は `--base <target> --head <candidate>` で 1 回、`HealthReviewInputSkipped` が 1 件（`review_run = worker`、`changed = false`）、承認の子なし。同じ candidate で `policy_only` の材料作りをもう一度呼んでも repo health は走らず記録も増えない。
  - `health_review_policy_no_reviewer_policy_file_candidate_needs_a_human`（受け入れ条件 0）: policy の file（`scripts/dev/repo-health/profiles/celeris.toml`）だけを変えた candidate は、command の条件が合格しても `reviewing` のまま承認の子が 1 件（題に candidate の SHA、本文に `policy files: …`）、tick を回しても合格しない。人が承認すると review をやり直して `done`、`health:policy approved by a human` の verdict は acceptance の外の criterion に 1 件、やり直しの review でも repo health は 1 回のまま（記録を再利用）。
  - 旧 `health_review_input_not_built_by_ticks_or_reviews_without_a_reviewer`（reviewer の無い review では repo health を走らせない）は、この変更で前提が変わったので上の試験に置き換えた。
- `Check::Reviewer` のある task の既存の試験（`health_review_input_*`・`health_review_policy_unknown_*`・policy-review の試験など）は変えずに通る（受け入れ条件 2）。`health_review_input_not_built_when_the_reviewer_pass_is_reused` は、reviewer の合格を再利用した回に `policy_only` の仕事も作らない（repo health が 1 回のまま）ことも確かめる。

## 証拠（提出時の HEAD で実行）

コードの検査は件名「G2 policy-no-reviewer: reviewer run の無い final review にも policy の関門を掛ける」の commit（この進捗 file を足す前。コードは提出時の HEAD と同じ）で実行した。進捗 file の commit の後に文書検査と health_review_ の試験を再実行した。

- `cargo nextest run -p task-dispatch -E 'test(/health_review_/)'` → exit 0、68 passed（うち `health_review_policy_no_reviewer_` 7 件）
- `bash scripts/dev/test-affected.sh` → exit 0。check-health は verdict=pass・new=0・worsened=0。web の tsc -b、vitest 711 件・node --test 113 件が合格。nextest 3,567 件 passed（11 skipped、245.9 秒）。script bundle SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check && python3 scripts/dev/check-architecture-map.py` → exit 0
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `crates/task-dispatch/`・`docs/api/v1/api.md`・この進捗 file だけ。

## 未解決

- 材料作りの非同期 task が panic した（`spawn_blocking` の失敗）ときは event が残らず、関門は掛からない（reviewer の有無に依らない。policy-unknown の未解決と同じ）。
- 同期の無い repo（同期を省いた・失敗した）・remote workspace の repo は candidate の SHA が決まらないので、記録も関門も無い（従来どおり）。
- reviewer の無い review の repo health の待ち時間（上の「変更」の最後）は、`Check::Reviewer` の無い task の final review にそのまま足される。
- ADR の付記（I8 の範囲・reviewer の無い review の扱い・criterion の選び方）と architecture-map の行は、後の WorkUnit `reclose` で合わせる。

## 提案

- `reclose` で ADR の D6.4 に「reviewer run の無い final review も関門に掛ける。verdict は acceptance の外の criterion（verdict の最大の次）に付ける」と、reviewer の無い review に repo health の時間が足されることを書くとよい。
- 材料作りの panic を fail closed にする（`attach` が失敗したとき candidate ごとに不明の `HealthReviewInputSkipped` を残す）は、試験用の失敗の注入点と一緒に別の WorkUnit で入れるとよい。
