---
title: "健全性v2 R4 own-hooks: 研究 ablation と試験用フックを 1 つの fixture に隔離する"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: running
updated: 2026-11-12
---
# 健全性v2 R4 own-hooks: 研究 ablation と試験用フックを 1 つの fixture に隔離する

完了日: 未完了（範囲の check が 2 file を許さない。下の「未解決事項」）

## 概要

- `Dispatcher` の `#[cfg(test)]` の field 10 個（`test_now`・`test_cos_reserved_sources`・`test_halt_after_wu_transition`・
  `test_worker_start_delay`・`test_policy_clock`・`test_skip_pre_review_sync`・`test_disable_write_set_gate`・
  `test_disable_stale_priority`・`test_sync_conflict_as_review_fail`・`before_integration_start`）を `test_hooks` 1 個にした。
  型は `crates/task-dispatch/src/dispatcher/tests/hooks.rs` の `TestHooks`（`now`・`policy_clock`・`worker_start_delay`・
  `halt_after_wu_transition`・`cos_reserved_sources`・`before_integration_start`・`ablation`）。
- phase_effect_ab の 4 flag は `dispatcher/tests/phase_effect_ab.rs` の `EffectAblation`（`skip_pre_review_sync`・
  `sync_conflict_as_review_fail`・`disable_write_set_gate`・`disable_stale_priority`）にした。本体は `test_hooks.ablation` だけを読む。
  読む場所（`write_set_gate_disabled`・`order_review_sync_queue`・`pre_review_sync_enabled`・review 前同期の衝突）は今と同じ
  `#[cfg(test)]` の行のままで、本番の build の分岐は増やしていない（本番の build に `TestHooks`・`EffectAblation` の型は無い）。
- 試験は field の書き方だけを変えた（`d.test_now = …` → `d.test_hooks.now = …`、`d.test_disable_write_set_gate = …` →
  `d.test_hooks.ablation.disable_write_set_gate = …` など）。assert・scenario・試験の名前は変えていない。
- `crates/task-dispatch/src/dispatcher/tests/structure_guard.rs` に試験 3 本を足した。
  - `structure_guard_dispatcher_has_one_test_field`: `Dispatcher` の struct の本体で、条件に `test` を含む `#[cfg(...)]` 属性が
    1 個より多いと落ちる。
  - `structure_guard_owner_structs_have_no_test_fields`: 一覧 `OWNER_STRUCTS`（`ClusterRuntime`・`HostMaintenance`・`RoutingRuntime`）の
    struct に同じ属性が 1 個でもあると落ちる。後の owner unit は自分の struct をこの一覧に足す。
  - `structure_guard_counts_cfg_test_attributes_by_attribute`: 数え方の試験（名前に関わらず数える、`#[cfg( test )]`・
    `any(test, …)`・同じ行の field も数える、doc comment・`feature = "testing"`・別の struct は数えない）。
- owner ごとの guard（`structure_guard_cluster_runtime`・`structure_guard_host_maintenance`・`structure_guard_routing_runtime`）にあった
  「`#[cfg(test)]` で始まる行が無い」の assert は、上の一覧の試験と同じ契約なので消した（規約 2）。上の試験は同じ不具合を検出し、
  `#[cfg(any(test, not(test)))]` の形も検出する（下の変異の結果。旧い assert はこの形を見逃した）。
- 本番の build のコードは変えていない（変えたのは `#[cfg(test)]` の行と注釈だけ）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- 計画の check 3（awk で `Dispatcher` の本体の `#[cfg(test)]` を数え、直後が `test_hooks:`）: exit 0。
- `cargo nextest run -p task-dispatch -E 'test(structure_guard_) | test(phase_effect_ab) | test(integration_ready_race)'`:
  24 件すべて合格（phase_effect_ab 9 件・integration_ready_race 9 件・structure_guard_ 6 件）。
- 変異の確認（試験の後に file を戻した）: `Dispatcher` に `#[cfg(test)] probe_extra: (),` を足し、`ClusterRuntime` の最初の field に
  `#[cfg(any(test, not(test)))]` を付けて `cargo nextest run -p task-dispatch --lib --no-fail-fast -E 'test(structure_guard_)'`:
  exit 100、6 件中 2 件失敗（`structure_guard_dispatcher_has_one_test_field`「Dispatcher has 2 #[cfg(test)] fields」、
  `structure_guard_owner_structs_have_no_test_fields`「ClusterRuntime has a #[cfg(test)] field」）。旧い `structure_guard_cluster_runtime` は合格した。
- `bash scripts/dev/test-affected.sh`（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）: exit 0、nextest 3750 件合格・11 件 skip、
  壁時計 487 秒（開始 load 1、終了 load 12）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0（verdict=pass、fail_grade=0、worsened=0。new=1 は `host_maintenance.rs` の
  `migration.remnant`（warning）で、この unit が変えていない行）。
- 範囲の check（計画の check 5）: 不合格。範囲外は `crates/task-dispatch/src/dispatcher/tests/cos_live_fix_d4.rs` と
  `crates/task-dispatch/src/dispatcher/tests/credential_e2e.rs` の 2 件。

## 2 回目の run（run 01M78XZH60）: celeris の check の不合格の直し

前の run の後に celeris が走らせた check のうち 2 件が不合格だった。

- `celeris:repo-health/1`: `size.responsibility-growth`（`dispatcher.rs` の本体の行 2139 が閾値 2000 超え、WU base では 2137）と
  `source-size`（`check-source-size.py --base $CELERIS_WU_BASE` が同じ file で exit 1）。原因は、`test_hooks` の field の前に足した
  2 行の doc comment が本番の行として数えられたこと（`#[cfg(test)]` より前の注釈は試験の範囲に入らない）。前の run の
  `check-health.sh` は base が `git merge-base HEAD main` で、そこでは値が大きいので落ちなかった。同じ説明は
  `tests/hooks.rs` の module 注釈にあるので、`dispatcher.rs` の 2 行を消した。
- 範囲の check: 下の「未解決事項」のとおり、正規表現が数字を含む file 名を許さない。コードでは直せないので plan_issue で申告する。

再検査（この run の作業ツリーで実行）:

- `python3 scripts/dev/check-source-size.py --root . --base $CELERIS_WU_BASE --baseline scripts/dev/source-size-baseline.json`: exit 0
  （0 failure、3 debt warning、1 reasoned exception）。
- `python3 scripts/dev/repo-health/repo_health.py --repo scripts/dev --base $CELERIS_WU_BASE --head worktree --policy-version 1`: exit 0
  （verdict=pass、fail_grade=0、new=0、worsened=0）。
- `cargo nextest run -p task-dispatch -E 'test(structure_guard_) | test(phase_effect_ab) | test(integration_ready_race)'`: 24 件すべて合格。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- commit「phase 1: dispatcher.rs の test_hooks の前の注釈を消し、WU base 比の本体の行を増やさない」の後の HEAD で:
  `bash scripts/dev/check-health.sh` exit 0（verdict=pass、fail_grade=0、worsened=0）、`sh scripts/dev/progress-index.sh --check` exit 0、
  `bash scripts/dev/test-affected.sh` exit 0（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、nextest 3750 件合格・
  11 件 skip、壁時計 445 秒、開始 load 0.7・終了 load 12）。
- 範囲の check は同じ 2 file で exit 1。許可の正規表現を `[a-z0-9_/]+\.rs$` にした写しでは範囲外の path が 0 件。

## 3 回目の run（run 01M78ZG0JC）: 範囲の check の再不合格

celeris の範囲の check は 2 回目の run の後も同じ正規表現 `[a-z_/]+\.rs$` のままで、同じ 2 file
（`cos_live_fix_d4.rs`・`credential_e2e.rs`）で exit 1 だった。コードは変えていない。

- 同じ check を作業ツリーで実行: exit 1、範囲外は上の 2 file だけ。許可を `[a-z0-9_/]+\.rs$` にした写しでは範囲外 0 件。
- `cargo nextest run -p task-dispatch --lib --no-fail-fast -E 'test(structure_guard_) | test(phase_effect_ab) | test(integration_ready_race) | test(cos_live_fix_d4) | test(credential_e2e)'`:
  38 件すべて合格。
- 2 file の変更は `d.test_now` → `d.test_hooks.now` の 4 行だけで、field を消す受け入れ条件 0 に要る。file 名を変えて check を
  避けることはしない（規約 9）。この run は done を返さず plan_issue（許可の正規表現を `[a-z0-9_/]+\.rs$` に広げる）を申告する。

## 未解決事項

- 範囲の check の正規表現 `crates/task-dispatch/(src|tests)/[a-z_/]+\.rs$` は file 名の数字を許さない。`cos_live_fix_d4.rs`（3 行）と
  `credential_e2e.rs`（1 行）は `d.test_now` を直接書くので、field を消すと同じ変更で `d.test_hooks.now` に直す必要がある。
  範囲の外に回避策は置けない（規約 9）ので、plan_issue で許可の正規表現を `[a-z0-9_/]+\.rs$` に広げることを申告する
  （2 回目の run でも check は同じ正規表現のままだった）。
- `crates/celeris/tests/root_done_summary.rs` の注釈が `test_now` を名指しする（範囲外。意味は「dispatcher の注入時計は外から差せない」で
  今も正しい）。直すなら record の unit で `test_hooks.now` に直す。

## 提案

- D6 の「入口が `TestHooks` から読んで引数で owner の関数に渡す」は、write-set gate と stale 優先の判断がまだ `impl Dispatcher` の
  method なので、この unit では読む場所を `test_hooks.ablation` の 1 か所に寄せるだけにした。admit・review-decide が判断を純粋関数に
  分けるとき、その関数は ablation を引数（本番は「ablation なし」）で受け、owner の struct に `#[cfg(test)]` の field を足さない。
  その owner の struct は `structure_guard.rs` の `OWNER_STRUCTS` に足す。
