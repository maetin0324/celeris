---
title: 配備・rollback の監査記録
updated: 2026-10-27
status: done
tasks: [01M5XXF7QP63BMV6AS0WNDW01N]
---
# 配備・rollback の監査記録

完了日: 2026-10-27

## 変更

- `phase 1: 配備とrollbackの実行根拠・検証結果を監査履歴に残す` — 必須実行者・理由・方針、task/decision 参照、verify snapshot、操作ごとの終了結果を保存。履歴 API と status に公開。
- 人 / CoS の API 経路を区別し、CoS の認証済み操作への参照を引き継ぐ。
- `記録した昇格・rollback の後の credentiald 切替失敗で監査記録を書き換えない` — promote.sh は promoted.json 記録後（`PROMOTED_RECORDED=true`）の失敗で audit を上書きしない。rollback.sh --restore-db は link 付け替え後に audit を exit_code=0 で閉じて rolled_back.json を書き、その後に credentiald を切り替える（失敗は exit 3 のまま）。失敗は credentiald_switch_failed.json だけに残す（ADR 付記）。
- `配備監査の actor を human-ops D2 の actor_kind 語彙に揃える` — `SD_ACTOR` は `<kind>` または `<kind>:<id>` で、kind は ADR 2026-10-27-human-ops-actor-roles-request-ref D2 の 7 種だけ。audit.sh は旧い `cos` / `operator:<name>` / `system:<name>`、未知の kind、空の id、メールの id を副作用の前に拒否し、記録に `actor_kind` と `actor_id`（無ければ null）を書く。CoS の `release.promote` は thread の種類から `cos_general:<thread id>` / `cos_project:<thread id>` を渡し、legacy・読めない thread は拒否する（`task_api::releases::cos_deployment_actor`）。`DeploymentContext::human(sha12)` の policy_ref は実際の sha12 を入れる。ADR の actor の節と docs/ops/selfdeploy.md を書き直した。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `bash scripts/dev/test-affected.sh` → exit 0。web 型検査、Vitest 682 件、Node 98 件、影響 crate の nextest / doc test 合格。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo test -p celeris --test deployment_audit` → exit 0、1 passed。実スクリプトを偽 host コマンドで実行し、必須情報欠落時の副作用前拒否、live / stop-start、通常 / DB 復元 rollback、失敗記録、再試行履歴、verify snapshot を検証。
- `bash scripts/selfdeploy/tests/promote_live_abort.sh` → exit 0、all ok。
- `bash scripts/selfdeploy/tests/credentiald_switch.sh` → exit 0、全件 ok。H・J・K で audit/*.json が 1 件・succeeded・exit_code=0 で promoted.json / rolled_back.json と一致することを検査。修正前の promote.sh・rollback.sh に戻すと H（audit status=failed exit_code=3）と J（rolled_back.json なし）が FAIL、exit 1。
- `bash scripts/selfdeploy/tests/promote_authorization_marker.sh` → exit 0、all ok（71 件）。actor 語彙の揃えで 3 件を足した: promote.sh・rollback.sh が `cos`・`operator:x`・`system:x`・`robot`・`robot:x`・`human:`・メールの id を拒否し副作用も audit も残さない、`SD_ACTOR=human` で actor_kind=human・actor_id=null、`external_operator:test` で actor_kind・actor_id が分かれて入る。
- `cargo test -p task-api --lib releases::tests` → exit 0、2 passed（CoS の general/project の区別と legacy・読めない thread の拒否、human の policy_ref の sha12 置換）。`cargo test -p task-api --test cos_ops_admin_config` → exit 0、4 passed（project thread の CoS 操作は `cos_project:<thread id>`）。
- `UPDATE_SCHEMA=1 cargo test -p task-api schema --lib` → exit 0、3 passed。`node web/scripts/gen-types.mjs` で生成型を更新。
- API 履歴と環境変数伝搬、CoS の actor・理由・方針・操作参照は Rust 回帰試験で検証。
- 本番配備、release gate、全 workspace の一括試験は実行していない。

## main の取り込み

- `Merge branch 'main' into celeris/01M5XXF7QP63BMV6AS0WNDW01N` — main の `selfdeploy: credentiald の切り替えで旧を disable、新を enable する` を取り込んだ。衝突は scripts/selfdeploy/tests/credentiald_switch.sh の K の検査 1 か所だけで、main の `cred_final`（is-active / is-enabled）と本 branch の `audit_matches` の両方を残した。ヘッダの注記（F、G・I・K の enable 状態、H/J/K の audit）と `SD_ACTOR`/`SD_REASON`/`SD_POLICY_REF` の export は自動で両方入った。lib.sh の `sd_switch_credentiald`（stop 旧→disable 旧→reset-failed 新→start 新→enable 新）は promote.sh・rollback.sh の記録後に呼ばれる位置のままで、disable/enable の失敗も exit 3 で audit は succeeded のまま。コード修正は不要だった。
- 証拠（統合後の HEAD で実行）:
  - `git merge-tree --write-tree main HEAD` → exit 0、`git merge-base --is-ancestor main HEAD` → 0。
  - `bash scripts/selfdeploy/tests/credentiald_switch.sh` → exit 0、30 件 ok（G・I・K の cred_final、H・J・K の audit_matches を含む）。
  - `bash scripts/selfdeploy/tests/promote_authorization_marker.sh` → exit 0、71 件 ok。
  - `bash scripts/selfdeploy/tests/promote_live_abort.sh` → exit 0、39 件 ok。
  - `bash scripts/dev/test-affected.sh`（基準 git merge-base HEAD main）→ exit 0。nextest 4638 passed / 20 skipped、Vitest 682、Node 98、health 検査すべて ok。
  - `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 本番への移行

本番変更は未実行。人が初回の昇格で本変更の promote.sh を使用し、`SD_ACTOR`（D2 の kind。人は `human`、外部 operator は `external_operator:<name>`）/ `SD_REASON` / `SD_POLICY_REF` を実際の実行者・理由・承認参照で設定する。以降の CLI rollback も同じ必須値を渡す。既存の配備記録は保持する。

## 未解決

なし。既存 release の実行者の欠落は推測で埋めない。

## 提案

なし。
