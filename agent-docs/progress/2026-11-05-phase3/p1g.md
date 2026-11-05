---
title: P1-G 自律復帰とイベントの正本
tasks: [01M6N7XY9W412TA8G2WV0726Z0]
status: running
updated: 2026-11-05
---
# P1-G 自律復帰とイベントの正本

未完了。最初の区切りは brief の原子的 wake と案件 CoS の予算集計。

## main と既存実装の照合
- 基準は `git merge-base HEAD main`。着手時の HEAD は main と同じ。A1〜A9 と P0-A の K29〜K40、A9 findings・arrival、運用手順を確認した。
- authority の grant 作成は既に `authority_unblock_tx` と同じ transaction で人の epoch と wake を書く。復帰は `pc/authority-required` に限る。これを再実装しない。
- brief の独立保存口は transaction を始めず、wake も入力の無効化も書かなかった。日次予算は task run だけを読んでいた。
- 本番操作、旧 milestones API、研究評価は変更しない。

## 変更
- commit 件名: `phase 1: make project brief wakes atomic and account for CoS chat usage`。task 全体の完了 commit ではない。
- brief の内容変更だけで、同じ transaction に `external_released` wake と入力 epoch を書く。同じ内容の再送は時刻が変わっても wake を増やさない。CoS の要約を人の回答として扱わず、失敗回数の人の操作によるリセットには使わない。
- 古い brief を見た snapshot の CAS と、未実行の効果の門は既存の epoch 照合で拒否する。brief だけでは blocked・paused・cancelled を解除しない。
- 案件の chat run の input + output を日次 token に加える。chat 起動数は `BudgetUsage.chat_starts` に数え、controller 入力数と独立に同じ日次上限を掛ける。両者を足して二重計上しない。
- ADR: `2026-11-05-phase3-p1g.md`。schema migration はまだ不要（予約 0116 は未使用）。

## 証拠
この区切りの提出時の HEAD で実行。task 全体は未完了。基準は `git merge-base HEAD main`。raw は `/local/celeris/data/workspaces/01M6N7XY9W412TA8G2WV0726Z0/artifacts/`。
- `cargo nextest run -p task-core -p task-ops -E 'test(/^p1g_|::p1g_/)'` → exit 0、2 passed（`p1g-focused.log`）。
- `test -f agent-docs/progress/2026-11-05-phase3/p1g.md` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- 最初の Rust 全対象検査 → exit 100、7617 passed・1 failed・21 skipped（`test-affected-stale-fixture.log`）。fixture 修正後の再実行は exit 100、7617 passed・1 failed・21 skipped（`test-affected.log`）。修正した fixture は合格。残る失敗は `script_generation_e2e_switch_error_rollback_restart`（TS call 200 ms timeout、generation 不一致）。main での再現は未確認。
- `cargo clippy --workspace -- -D warnings` → exit 0（`clippy.log`）。
- `cargo fmt --all -- --check` → exit 0（`fmt.log`）。
- web: `tsc -b` exit 0、unit と server 試験は合格。runner 検査は正しい pnpm store の指定後に合格。Rust の失敗で script が止まるため、後続の doc test は未実行。
- `CELERIS_SCRIPT_BUNDLE_DIR=<artifacts>/p1g-bundle CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_generation_e2e --test project_cos_shadow -E 'test(script_generation_e2e_switch_error_rollback_restart) | test(project_cos_ts_review_failed_retries_after_brief)'` → exit 0、2 passed・4 skipped（`isolated.log`）。generation 試験の不安定さは残る。全対象の合格とは扱わない。

## 失敗ケース
- `test-affected` の runner cache は sandbox の共有 cache が read-only で exit 2。`CELERIS_SCRIPT_RUNNER_CACHE="$CARGO_TARGET_DIR/p1g-runner-cache"` で task の書き込み可能な cache を使う。
- control-ts の offline install は local store と既存 store の両方で必要な版が不足し exit 1。lockfile 固定の通常 install で解消。試験内の一時 repo に同じ store を渡すには pnpm v12 の `pnpm_config_store_dir="$PWD/.pnpm-store"` が必要（`npm_config_store_dir` と `PNPM_STORE_DIR` は config get で undefined）。検査のネットワークは使わない。
- 既存の `project_cos_ts_review_failed_retries_after_brief` fixture は root 効果に epoch 0 を固定で渡していた。brief 更新後は epoch が進むため古い効果が拒否され、task 不在で失敗。fixture を現行 epoch の読み取りと `RootCreate::Created` の照合に直した。
- 最初の focused 試験の chat fixture が旧 kind `human` を使って CHECK 制約で落ちた（exit 100）。現行の `project` に直した。

## 受け入れの現在地と未解決
- 条件 0 / K29〜K36: atomic rollback・同じ brief 再送・3 日後の再送・入力 epoch による古い効果の拒否を store 試験で追加。restart/crash・brief/authority 同時編集・clear/deny・同じ失敗の再発・7 日複合 replay は未実装。
- 条件 1 / K37: brief で blocked・paused・cancelled が解けない試験を追加。未解決 human_required・危険予算の時間経過の複合確認は残る。
- 条件 2 / K38〜K40: chat token と起動数の案件別・日別集計と上限の試験を追加。human wait の毎時再評価削減の前後実測、翌日の expiry 分類と自律復帰は未実装。
- 条件 3: このファイルを作成。command check の実行結果は検査後に記録する。
- 人の question 回答・根本原因の解消・外部応答による blocked の復帰は、根拠の分類と CAS を設計してから実装する。単に brief の wake で blocked を外す変更はしない。

## 提案
- 次の区切りで K34・K35・K37・K40 の復帰分類を実装し、その後 K29〜K40 の複合 virtual clock replay と K38 の前後計測を通す。

## 本番への移行
- この区切りで config の削除・変更はない。本番への promote は実施しない。
