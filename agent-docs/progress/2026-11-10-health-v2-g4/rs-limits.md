---
title: 健全性v2 G4 rs-limits — 非 Celeris の fixture で修復の上限・同じ指摘の往復・変更の無い repo の棚卸しの省略
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-11
---
# 健全性v2 G4 rs-limits — 非 Celeris の fixture で修復の上限・同じ指摘の往復・変更の無い repo の棚卸しの省略

完了日: 2026-11-11

`crates/task-dispatch/src/dispatcher/tests/health_generic_limits.rs` に 3 試験を足し、3 試験とも合格する。設計は [ADR 2026-11-10 health-generic-fixtures](../../adr/2026-11-10-health-generic-fixtures.md) の (3) の (h) の行と D3.1・D3.2。基準は `$CELERIS_WU_BASE`。本体・検査器・policy・baseline・builder は変えていない。

## 実装前 brief
1. 根本原因: G3 の上限（`max_repairs_per_class`）・往復（D4.3）・棚卸しの省略（D6.4）・起票の重複防止（D6.6）の試験は、どれも stub の検査器（決まった fingerprint を返す）か、検査器を commit した一時 repo で確かめている。本物の検査器の fingerprint（層の逆向きの edge と file で決まる）と差分の判定（同じ違反は 2 回目に `existing`）で同じ結果になるかは確かめていない。
2. 責務 owner: 経路は `Dispatcher::try_review_repair`（`review_verdict.rs`）と `health_review::limits`、棚卸しは `health_review::inventory`（`InventoryFire::inventory_repo`・`file`）と `task_ops::health_debt::file_health_debts`。試験は `dispatcher/tests/health_generic_limits.rs`（ADR D3.1）。
3. 既存の再利用先: `health_generic.rs` の `pub(super)` の helper（`configure_real_bundled_checker`・`build_fixture`・`apply_candidate`・`changed_paths`・`review_health`・`blocking_rules`・`dep_inversion_rules`・`repair_classes`・`Fixture`）、`health_enforce.rs` の `non_celeris_dispatcher`・`RepairActionAdapter`、`target_sync.rs` の `reviewing_task`・`commit_file`。棚卸しの cron の組み立ては `dispatcher/tests/health_inventory.rs` の `setup` と同じ形（その file の `setup` は private で、この unit の write set の外なので写した）。
4. 選択肢: 上限は atomic の direct review で確かめた（段の統合でも同じ判定の関数を通る。review の経路は fixture の候補をそのまま worktree に入れられる）。往復の「root の受信箱に 1 件」は、1 つの root の下の 2 つの子 task が同じ fingerprint の集合で往復する形にした（root の task 自身の往復だと、子の往復が root に書かれることを通らない）。棚卸しは runner を直接呼ばず、注入した時計と cron の tick で発火させた。
5. 新しい状態・分岐・fallback・port: 無し（試験の file と `mod` の 1 行だけ）。
6. 撤去対象: 無し。
7. 不変条件: 下の「試験と結果」の表。

## 変更
- `phase 1: test repair cap, repeat escalation and unchanged inventory skip on generic fixtures`
  - `crates/task-dispatch/src/dispatcher/tests/health_generic_limits.rs`（新規）: 3 試験と helper（`reviewing_dep_inversion`・`dep_inversion_fails`・`assert_blocked_without_more_repairs`・`inbox_questions`・`fire`・`fire_counts`）。
  - `crates/task-dispatch/src/dispatcher/tests/mod.rs`: `mod health_generic_limits;` の 1 行。
- 修復の代わりの変更は fixture の内容を Rust に写さない（ADR の K2）: 上限の試験は候補が足した file を `git mv` で移すだけ、往復の試験は関係の無い `repair-<n>.txt` を commit するだけ、棚卸しの試験は `git update-ref` で `main` を fixture の base・head・tree の同じ空の commit（`git commit-tree`）に動かすだけ。

## 試験と結果（提出時の HEAD で実行）
| 試験 | 確かめること | 結果 |
|---|---|---|
| `health_generic_limits_repair_cap_blocks` | ts の dep-inversion の atomic の review。修復は逆向きの import の file を `git mv` で移すだけで、本物の検査器の fingerprint が毎回変わる（3 回の評価の `deps.violation` の fingerprint が 3 つとも違う）。既定の `max_repairs_per_class` = 2・`max_repairs` = 3 で、class `health:deps.violation` の repair 2 回の後に 3 回目の repair・`review_fail`・replan をせず Blocked（`review_blocked`、attempts 0）。受信箱の質問は 1 件で `health_repair_limit: review: … 修復の合計 2/3 回、health:deps.violation の修復 2/2 回`。往復（`HealthFindingsRepeated`）にはならない | 合格 |
| `health_generic_limits_repeat_escalates` | ts と rust のそれぞれで、Draft の root の下の 2 つの子 task（別々の fixture の repo）が dep-inversion の review の修復の後も同じ fingerprint の集合（ts は `deps.violation`、rust は `deps.cycle` と `deps.violation`）を出す。どちらの子も repair 1 回の後に Blocked（attempts 0）で質問 `health_findings_repeated:` が 1 件。`HealthFindingsRepeated` は root に 2 件（源は 2 つの子、位置 review）で、子には書かない。人の受信箱の `health_repeat` は 1 件で、task は root、blocking に 2 つの子 | 合格 |
| `health_generic_limits_inventory_skips_unchanged` | 毎時の cron `health_inventory` が 1 つの案件の ts と rust の fixture の repo（既定の branch `main`）を棚卸しする。1 時: base = head = fixture の base で Pass、起票 0。2 時: `main` が候補へ進み、前回の head を base に Fail、debt の draft 3 件（ts 1・rust 2）。3 時: 変化なしで 2 repo とも `unchanged`（検査器を走らせない）、起票 0。4 時: tree の同じ空の commit で `main` が進んでも `unchanged`。5 時: `main` を base に戻して評価し Pass。6 時: もう一度候補へ進めて同じ fingerprint の Fail、開いた debt があるので起票 0・`existing_skipped` 3。debt の task は 2 時の 3 件のまま、冪等キー `health-debt:` は 3 つ | 合格 |

ADR (3) の表との差: 無し。上限の試験は ts だけで確かめた（rust の fixture の逆向きの依存は `crates/core/Cargo.toml` の行で、内容を書かずに file を移せない。往復と棚卸しは ts と rust の両方）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(health_generic_limits_)'`: exit 0。3 tests run、3 passed。
- `bash scripts/dev/test-affected.sh`: exit 0（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 3737 tests passed・11 skipped、Python の unittest 94 件 OK・expected failures=2、check-health の台本 7 件 OK）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p task-dispatch --tests` でも新しい file に警告なし。
- `cargo fmt --all -- --check`: exit 0。
- 範囲と検査器・policy・baseline の不変は下の「範囲の確認」。

## 範囲の確認
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は `crates/task-dispatch/src/dispatcher/tests/health_generic_limits.rs`・`crates/task-dispatch/src/dispatcher/tests/mod.rs` と、この進捗 file。
- `git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh scripts/tests size-baseline.json`: exit 0（検査器・policy・baseline・builder に差分なし）。

## 未解決
- 無し（rs-limits の範囲）。本物の検査器で期待どおりに検出・判定できない能力は見つからなかった。
