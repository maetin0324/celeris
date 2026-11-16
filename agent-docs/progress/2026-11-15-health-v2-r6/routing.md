---
title: R6 routing の未使用定義と hash の集約
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
---
# R6 routing の未使用定義と hash の集約

完了日: 2026-11-15

## commit

- `phase 1: converge routing records and remove unused definitions`
- `routing: ADR の本文を進捗へ移して範囲外の file を消し、check の grep の誤りを未解決に書く`
- `routing: replan で直った check を再実行し、check の誤りを『replan で直した』に書き換える`

## 判定と削除前の検索

検索: `rg -n 'ScoreBreakdown|DecisionSubject|\bRouteDecision\b|\bto_key\b|\btask_routing_audit\b|cheap_local_first' crates web control-ts scripts docs config/celeris.example.toml`。削除前に実行し、結果全文は run artifacts の reference-search.txt に保存した。cfg・feature・試験・wire/schema・文書もこの範囲で確認した。

| 項目 | 判定 | 根拠 |
|---|---|---|
| config-execution-cheap-local-first | 消した | execution.rs の欄・default 関数・Default 初期化と設定例だけ。選択コードの読み取りは無し。web/e2e/support/fake-daemon.mjs の同名の理由文字列は設定欄の参照ではなく履歴 fixture なので維持 |
| route-record-route-decision | 消した | route_record::RouteDecision と DecisionSubject は定義と両者の内部参照だけ。schema・web・task_core::RouteDecision は direct_route の別型。route_select の同名型は現行の判断記録の正本なので維持 |
| task-features-content-hash | 寄せた | route_record の FeatureBody と eval_trials の EvalFeatureBody は同じ4欄・同じ欄順・同じ serde_json::to_vec。route_record に集約し両方から呼ぶ |
| unref-scorebreakdown | 消した | cost.rs の定義だけ。cost の実計算と試験は維持 |
| unref-to-key | 消した | provider_source.rs の定義だけ。Serialize/Deserialize の wire は維持 |
| unref-task-routing-audit | 消した | task-ops の定義だけ。task-api は task_routing_audit_with_requests を使用。RoutingAudit と履歴の投影は維持 |

owner 地図: docs/architecture-map.md の「LLM source / model の選択」「routing 監査と学習 record の export」。hash と記録は task-core::route_record、監査は task-ops::routing_audit、評価記録の呼び元は task-dispatch::dispatcher::eval_trials。設定欄は celeris::config::execution が owner。

## 実装前 brief

1. 根本原因: 廃止済みの定義と同じ hash 仕様の二重実装が残る。
2. owner: 上記の record・config・audit の定義元。
3. 再利用: FeatureBody・digest・既存の deny_unknown_fields。
4. 選択: 既存の直列化をそのまま共通関数にする。別々の hash 実装は撤去する。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去: 上表の未使用定義、無効な設定例と説明、EvalFeatureBody。
7. 不変条件: hash の byte 一致、wire・DB・events・履歴の維持。既存試験は固定の期待値を持たず同時の変化を検出できないため固定 hash 試験を追加。廃止欄の true / false を config が拒否する試験を追加する。

## 決定（ADR は置かない）

この WU の範囲の check は agent-docs/adr/ を許さないため、前回の run が書いた agent-docs/adr/2026-11-15-routing-record-cleanup.md を消し、決定の本文をこの節に移した。決定の中身は task の Objective が指定したもので、新しい設計判断は含まない。

- `route_record` を `{schema, template, brief, aux}` の直列化と `sha256:` + 小文字 hex の owner にし、通常の特徴量生成と dispatcher の評価記録が同じ関数を呼ぶ。欄順・null・空配列・日本語の UTF-8 を含めて既存の byte 列を変えない。schema と template の版は変えない。
- 参照の無い `route_record::{RouteDecision, DecisionSubject}`、`model_router::cost::ScoreBreakdown`、`LlmSourceRef::to_key`、`task_ops::routing_audit::task_routing_audit` を消す。現行の `route_select::trace::RouteDecision`、直行経路の同名型、API、履歴の投影は残す。
- 効かない `[execution].cheap_local_first` を消す。既存の `deny_unknown_fields` により、この欄を書いた config は読み込みエラーになる。互換分岐は置かない。本番 config の確認と削除は「本番への移行」節に人の手順として書く。
- 検証: 共通化の前の両実装に固定入力の試験を置いて同じ期待値を確かめてから、共通関数の呼び出しに置き換える。従来の同一性の試験は両経路の hash が同時に変わる退行を検出できないため、固定の期待値を足す。config には true と false の両方の拒否の試験を置く。既存の config の試験はこの欄の再導入を検出できない。

## 証拠（前回の run の HEAD で実行。基準は `$CELERIS_WU_BASE`。最新の結果は下の「replan 後の check の再実行」）

- 共通化前: `cargo nextest run --workspace -E 'test(feature_hash_fixed_bytes)'` → exit 0、2 passed。core の FeatureBody + digest と dispatch の EvalFeatureBody + sha256 をそれぞれ使う旧実装で確認した。入力は schema `celeris.task-features/1`、template `eval`、brief `固定入力\nquoted "text"`、aux の genre `eval_problem`・harness `codex`・残り Default。期待値は `sha256:d7c838872304e2486d7ab93a63c269c60d359941b944a33034bba9a4ed64fcfa`。ログ: hash-before-fixed.log。初期の値の採取は hash-before-core.log・hash-before-dispatch.log。
- `bash scripts/dev/test-affected.sh --list` → exit 0、14 crate と web typecheck+unit、test-ts-verify。
- `bash scripts/dev/test-affected.sh` 初回 → exit 2。health は pass、新規・悪化0。web typecheck と unit は完了したが、test-ts-verify の準備で既定の `/local/celeris/data/scratch/script-runner-cache` が read-only のため失敗。offline install は不足 package があり、既存 script が prefer-offline に切り替えて install した。
- 2回目: `CELERIS_SCRIPT_RUNNER_CACHE="$TMPDIR/script-runner-cache" bash scripts/dev/test-affected.sh` → exit 1。cache は解消したが検証用コピーの offline store に std-env / esbuild が無く、install 段で失敗した。
- `pnpm -C control-ts install --offline --frozen-lockfile` は chai が不足して失敗。`pnpm -C control-ts install --prefer-offline --frozen-lockfile` で依存を準備した。コピーからも同じ store を読めるよう、TMPDIR の pnpm 起動スクリプトで元の pnpm に `--store-dir <worktree>/.pnpm-store` を渡す。検査コード・判定条件は変更しない。cache 設定と合わせた3回目は web 714 / gateway 113 / test-ts-verify 20 passed（0 failed）まで進んだが、共有検査枠 `/run/user/1001/celeris/check-slots/slot-0.lock` が read-only で exit 2。検査枠を迂回せず、必須試験に必要な書き込みとして権限審査付きで再実行した。CARGO_TARGET_DIR などの指定値は変更していない。
- 最終: `PATH="$TMPDIR/routing-check-bin:$PATH" CELERIS_SCRIPT_RUNNER_CACHE="$TMPDIR/script-runner-cache" bash scripts/dev/test-affected.sh`（共有検査枠のため権限審査付き実行）→ exit 0。nextest は 8,530 passed / 23 skipped、11 slow、586.741秒。doc test は3 passed / 2 ignored、失敗0。web は103 files / 714 passed、gateway 113 passed、test-ts-verify は20 passed / 0 failed、script bundle SKIP は0件。共通化後の固定 hash 試験2本と `removed_cheap_local_first_is_a_config_error`（true / false の両方）もこの実行に含む。ログ: test-affected-authorized.log。
- `cargo clippy --workspace -- -D warnings` → exit 0（vendor deno_core の既存 deprecated 警告のみ、workspace の警告は無し）。
- `cargo fmt --all -- --check` → exit 0、差分0。
- `sh scripts/dev/check-doc-links.sh` → exit 0。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（448 files）。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS" | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R6 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0、write-set R6: ok。列挙した10 path はこの WU の対象だけ。
- `git diff --name-only "$CELERIS_WU_BASE" -- crates/campaign crates/routing-eval crates/jev-classifier` → exit 0、差分0。K2/K4/IEEE Access の fixture・評価器・比較条件に変更無し。
- `git diff --exit-code "$CELERIS_WU_BASE" -- docs/api docs/protocol web/api/generated control-ts/fixtures crates/campaign crates/routing-eval crates/jev-classifier crates/task-core/migrations crates/task-core/src/model/events.rs crates/celeris-credentiald` → exit 0、差分0。

## 本番への移行

人が配備前に使用中の config の `[execution]` を確認し、`cheap_local_first = true` または `false` があればその行を削除する。例: `rg -n 'cheap_local_first' ~/.config/celeris` で設定の include 先も確認する。削除後は配備時の通常の config 検証で unknown field が無いことを確認する。指定が残れば新しい版は読み込みを拒否する。人からの回答では本番 config にこの欄は無いことを確認済み。本 run は本番の設定変更・daemon 操作を行わない。

## 未解決

- 機能上の未解決事項は無し。
- test-affected の終了後に scratch の `.trash-*` 内の `own-eval` 3 件で清掃の Permission denied が出る（検査自体は exit 0）。この WU は当該 scratch 領域を変更していない。

## 前回の check の誤り（replan で直した）

- 誤り: v1 の check の `! grep -n 'pub fn task_routing_audit' crates/task-ops/src/routing_audit.rs` は前方一致で、残すべき `pub fn task_routing_audit_with_requests`（`GET /tasks/{id}/routing` の handler の crates/task-api/src/routing.rs が呼ぶ）に当たって落ちた。実装の誤りではない。前回の run が plan_issue として申告した。
- replan で直した: replan（計画 v2）は否定の grep を語の境界の `grep -nw` にし、`grep -qw 'pub fn task_routing_audit_with_requests' crates/task-ops/src/routing_audit.rs` で残す関数が在ることも確かめる形にした。コードは変えていない。

## replan 後の check の再実行（提出時の HEAD で実行。run 01M7FS8JSTRJ06KRNPE0XW6E02、2026-11-15）

基準は `git merge-base HEAD main`（範囲 check は `$CELERIS_WU_BASE` と `$CELERIS_WU_SCOPE_PATHS`）。
check 1・2 と対象試験は、本 file の更新だけの commit の直前の HEAD で実行した（提出時の HEAD とのコードの差分は無い）。check 3〜5・fmt・`progress-index.sh --check` は提出時の HEAD で再実行した。

- check 1 `bash scripts/dev/test-affected.sh` → exit 0（14:21〜14:37Z）。health は verdict=pass、新規 0・悪化 0。nextest は 8,530 passed / 23 skipped（12 slow）、563.195 秒。doc test は失敗 0。web は 103 files / 714 passed、gateway は 113 passed / 0 fail、test-ts-verify は 20 passed / 0 failed、script bundle SKIP は 0 件。前回の run で要った cache・pnpm・検査枠の迂回は無しで通った。
- check 2 `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check` → clippy exit 0（警告は vendor の deno_core の既存の deprecated 1 件だけ）、fmt exit 0・差分 0。
- check 3（語の境界の否定の grep 6 本と `task_routing_audit_with_requests` が在る確認）→ exit 0。
- check 4（凍結 path の `git diff --quiet` と `slices-check.py --write-set R6`）→ exit 0、`write-set R6: ok`。
- check 5（範囲 check、`sh "$CELERIS_WU_SCOPE_PATHS"`）→ exit 0、範囲外の path 無し。変更 path は本 file を含む 9 件。
- 対象試験: `cargo nextest run -p task-core -p task-dispatch -p celeris -E 'test(feature_hash_fixed_bytes) | test(removed_cheap_local_first_is_a_config_error)'` → exit 0、3 passed（固定 hash の 2 本と config 拒否の 1 本）。

## 提案

無し。
