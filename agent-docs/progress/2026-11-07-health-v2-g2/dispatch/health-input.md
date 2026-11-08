---
title: 健全性v2 G2 dispatch health-input — final review の health 文脈を candidate SHA ごとに 1 回作る
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch health-input — final review の health 文脈を candidate SHA ごとに 1 回作る

完了日: 2026-11-08

WorkUnit `health-input`。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の D4.1〜D4.4・I5。

## 変更
件名「G2 health-input: final review の health 文脈を candidate SHA ごとに 1 回作り ReviewRequest.health に渡す」と「G2 health-input: 試験の仮の owner 地図の path を docs/ の外にする（doc-links の検査が link と読む）」の commit、この進捗の commit。
- `crates/task-dispatch/src/health_review/input.rs`:
  - `Dispatcher::health_job`（tick の中。git・python は起こさない）: reviewer run がある review だけ、`ReviewSync` の repo を候補（repo_id・worktree・target_sha・reviewed_sha）にする。remote workspace の repo は理由「remote workspace」、同期の無い repo は「review 前の同期が無い」で skip に回す。
  - `HealthInputJob::attach`（reviewer run を起こす非同期 task の中）: `build_health_input` を `spawn_blocking` で走らせ `ReviewerRun.health` に入れる。子 process は全体で 600 秒（`HEALTH_INPUT_TIMEOUT`）の締め切りを共有する（tokio の timer は使わない）。
  - `build_health_input`: repo ごとに base 側の検査器（`policy::with_base_checker`）で `--print-review-policy` を読み、同じ `(repo_id, target_sha, reviewed_sha, definition_sha256)` の `HealthReviewInputBuilt` があればその `input` を使う。無ければ `git diff --numstat -z --no-renames <target>..<reviewed>`（paths 40 件まで）、`git show <target>:<owner_map>` の表の行のうち変更 path を含む行（`…` の中の path。`{a,b}` 展開・親 dir・`foo.rs` の module dir。40 件まで）、`repo_health.py --repo <worktree> --base <target> --head <reviewed> --policy-version 1 --out <task dir>/runs/health/<repo>-<target12>-<reviewed12>.json` の report を作り event に追記する。findings は `delta ∈ {new, worsened}` かつ exception が null のものを 50 件まで、超えた数を `truncated`。`reviewable = false` は findings を空にして `reason` に理由。policy の `files` は base と head で blob の違う file だけ。方針が absent・読めない repo は `HealthReviewInputSkipped`。brief は active な計画の `design_brief`。
- `health_review/policy.rs`: 検査器の取り出しを `with_base_checker`（`BaseChecker::run`・`print_review_policy`）に分け、`load_review_policy` と health の材料で共有した。時間切れの文は「base の検査器の実行が時間の上限を超えた」。
- `crates/task-dispatch/src/review.rs`: `ReviewerRun.health` を足し、`into_request` が `ReviewRequest.health` に入れる（`run_reviewer_inner` の 1 行を書き換え、行は増やしていない）。
- `crates/task-worker/src/protocol/health.rs`: `HealthFindingsSummary.reason`（任意。findings を空にした理由）。`docs/protocol/worker-protocol.schema.json` を再生成。
- `dispatcher/review_spawn.rs`: `spawn_review` に `health_job` の 1 行と `reviewer: health.attach(reviewer_run).await`。同じ関数で 2 度目の `events_for` の読み直し（6 行）を 1 度目の結果の写しに変えて相殺した（間に足される event は WorkerStarted・routing だけで、必須 check の再利用の判定に使う event は変わらない）。`pick_reviewer` は `health: None` の 1 行を `harness` の組み立ての 1 行の整理（struct の更新構文）で相殺。`ReviewSync`・`review_spawn`・`task_dir` の可視性を `pub(crate)` にした（行は増えない）。
- 試験（prefix `health_review_input_`、13 件）: `health_review/input/tests.rs`（7 件。findings の上限と truncated・unreviewable・paths の上限・owner 行の当て方・同じ candidate で 1 回・absent の skip・report の失敗は記録しない）と `dispatcher/tests/health_review_input.rs`（6 件。reviewer の request に diff 統計・owner 行・findings 要約・policy の状態・不変条件が入る、reviewer の再試行で再利用し repo health は 1 回、absent で Skipped、reviewer の無い review と tick では作らない、reviewer_reuse の再 review では作らない、WU の run・check では作らない）。`target_sync.rs` の補助関数 5 つを `pub(super)` にした。

## 証拠（提出時の HEAD で実行）
この WU では main の取り込み・rebase をしていない。下の検査は最後の code の commit の後に実行した（後の commit は進捗の file だけ）。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。nextest 5645 passed・22 skipped。check-health は verdict=pass・fail_grade=0・new=0・worsened=0（existing 718）。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `cargo nextest run -p task-dispatch -E 'test(health_review_)'` → 25 passed（policy 12・input 13）
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を計画の許可の正規表現で除く）→ 範囲外 0 件（変更は crates/task-dispatch・crates/task-worker・docs/protocol と、この進捗）

## 未解決
- repo health を走らせられなかった（時間切れ・report が無い・壊れた）材料は event に記録せず、reviewer には `verdict = error`・`reason` 付きで渡す。次の再試行で走らせ直すので、その場合は同じ candidate で 2 回以上走る（I5 の「高々 1 回」は記録した材料について）。
- 本物の `repo_health.py` はこの repo の全体で約 80 秒かかった（worktree 指定、手元の計測 1 回）。その分 reviewer run の起動が遅れる。上限 600 秒は計測 1 回からの見積もり。
- owner 行の当て方（`…` の中の path との一致）は architecture-map の書式に合わせた決め方で、ADR に細かい規則は無い。
- `cargo clippy -p task-dispatch --tests` は既存の `dispatcher/tests/cos_slot_wait.rs:172` の lint で落ちる（必須の clippy は `--tests` なしで通る）。

## 提案
- architecture-map の「健全性の review」の行に `health_review/input.rs`（`build_health_input`・`Dispatcher::health_job`）と試験の path を足す（close の unit）。
- health-verdict の unit は、`ReviewRequest.health` を渡した review の review.json に `health` を必須にする。この unit の流れの試験の reviewer は `"health":{"findings":[]}` を書いてあるので、そのまま通る見込み（未確認）。
