---
title: "健全性v2 R6 test-dedupe: R5 から回った duplicate_setup の残り 12 組を寄せる"
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R6 test-dedupe

完了日: 2026-11-15

WorkUnit `test-dedupe`（段 api）。R5 の [dedupe-api.md](../2026-11-10-health-v2-r5/dedupe-api.md)（D01・D16・D41・D46）と
[dedupe-celeris.md](../2026-11-10-health-v2-r5/dedupe-celeris.md)（D04・D09・D21・D25・D26・D28・D37・D43）が
write set の外として残した試験の setup の写しを片付けた。番号 D は R5 の `contracts.json` の `candidates.duplicate_setup` の
1 始まりの順。基準は `$CELERIS_WU_BASE`（slices-check は `git merge-base HEAD main`）。

## 結論
- 12 組のうち 11 組は写しを全部消し、1 つの helper に寄せた。D37 は 3 つの写しのうち型の同じ 2 つだけを寄せ、
  型の違う 1 つ（support の `is_timeout`）は残した。
- 試験関数は 1 本も消していない。`cargo nextest list -p task-api -p celeris` で、寄せる前と後の試験 binary は
  113 個、試験は 1703 件で同じ。binary ごとの件数と試験名の集合も一致した（前だけ 0 件、後だけ 0 件）。
- R5 の後に main で既に寄せた組・消えた組は無かった。12 組の member はすべて残っていた。段 bundle が動かした file が 3 つある
  （`general_cos_consult.rs` → `cos_delivery_suite_b/`、`unblocks.rs` → `unmapped_suite_b/`、
  `cos_chat_attach_handoff_kb.rs` → `cos_delivery_suite_a/`）。動いた先の path で扱った。
- `crates/campaign`・評価器・凍結 fixture は変えていない（下の check 3 が exit 0）。campaign の 3 つの e2e
  （`crates/celeris/tests/campaign_*_e2e.rs`）は celeris の試験 file で、event の雛形と `dechunk` の呼び先だけを変えた。

## 実装前 brief
1. 根本原因: 同じ setup（DB の 1 値を数える・添付付きの env・credentiald の binary の探索・event 列の通し番号・偽 webhook・
   config の読み込み・実行 file の書き込み・run の event の雛形）を試験 file ごとに写していた。症状は、`TestEnv`・`Event`・
   `Config::load` の形が変わると複数の写しを直す必要があること。
2. 責務 owner: task-api の結合試験は `crates/task-api/tests/common/`、celeris の結合試験は `crates/celeris/tests/support/`。
   src の unit 試験は、写しを持つ module の最も近い共通の親に `#[cfg(test)]` の module を置く
   （`cos::basis`・`config`・celeris の crate 根）。
3. 既存の再利用先: `common/mod.rs`（`admin_env`）、`support/webhook.rs`（R5 の D42 で作った偽 webhook）、
   `support/script_compare.rs`（`is_timeout`）。D09 は新しい helper を作らず `support/webhook.rs` を使った。
4. 選択肢: (a) contracts.json の案どおり D04・D21・D26 を `crates/campaign/tests/support_cases` に置く案は、campaign の
   crate を変えるので採らない。celeris の `tests/support/campaign_e2e.rs` を `#[path]` で読む。(b) D09 を repo 直下の
   `tests/support/http_async.rs` に移す案は、この unit の範囲（`crates/task-api`・`crates/celeris`）の外なので採らない。
   (c) 小さな helper は R5 と同じく `support/` の別 file にして `#[path]` で読む（`mod support;` で 769 行の
   `script_compare.rs` を読ませない）。
5. 新しい状態・分岐・fallback・port: 無し。`#[cfg(test)]` の module 3 つ（`task_api::cos::basis::test_support`、
   `celeris::config::test_support`、`celeris::test_support`）と試験の helper file 3 つ（`common/browser.rs`、
   `support/campaign_e2e.rs`、`support/executable.rs`）を足した。
6. 撤去対象: 写しの helper 31 関数と `Hook` の struct 2 つ（下の表）。
7. 不変条件: 試験関数の名前と数が変わらない（`nextest list` の比較）。寄せた helper の本体は写しと同じ
   （D09 だけは `support/webhook.rs` の `Hook` が `seen` の受け口を余分に持つ。読み手は使わない）。affected の試験が通る。

## 項目ごとの判定
| 組 | 写し（消した） | 判定 | 寄せ先の helper | 読み方 |
|---|---|---|---|---|
| D01 `count` | `cos_live_fix_d1.rs`・`cos_live_fix_d2.rs`・`cos_delivery_suite_b/general_cos_consult.rs`・`unmapped_suite_b/unblocks.rs` | 寄せた | `crates/task-api/tests/common/mod.rs` の `count` | `use common::*` |
| D01 の候補外 | `cos_operations.rs`・`cos_delivery_suite_a/cos_chat_triage_scenarios.rs` の `count`（`cos_ops::db(env)` で開く同じ意味の 1 値読み） | 寄せた | 同上 | 同上 |
| D16 `env_with_attachments` | `cos_chat_attach_pin.rs`・`cos_delivery_suite_a/cos_chat_attach_handoff_kb.rs`・`cos_live_fix_d1.rs` | 寄せた | `common/mod.rs` の `env_with_attachments` | `use common::*` |
| D41 `bridge_binary` | `browser_e2e.rs`・`browser_h3_injection.rs` | 寄せた | 新しい `common/browser.rs` の `bridge_binary` | `use common::browser::bridge_binary` |
| D46 `seq` | `src/cos/basis/job_wait_extend.rs`・`review_rework.rs` の `tests` | 寄せた | 新しい `src/cos/basis/test_support.rs`（`#[cfg(test)] mod test_support;`） | `use crate::cos::basis::test_support::seq` |
| D04 `dechunk` | `campaign_acceptance_e2e.rs`・`campaign_lock_e2e.rs` | 寄せた | 新しい `crates/celeris/tests/support/campaign_e2e.rs` | `#[path]` |
| D21 `ev_started`・`started` | `campaign_acceptance_e2e.rs`・`campaign_lock_e2e.rs`・`campaign_wiring_e2e.rs`（`started` は名前だけ違い本体は同じ） | 寄せた | `support/campaign_e2e.rs` の `ev_started` | `#[path]` |
| D26 `ev_finished` | `campaign_acceptance_e2e.rs`・`campaign_lock_e2e.rs` | 寄せた | `support/campaign_e2e.rs` の `ev_finished` | `#[path]` |
| D09 `Hook`・`hook` | `src/daemon/classify_tick/e2e.rs`・`tests/credential_wait_notify.rs` | 寄せた | 既存の `crates/celeris/tests/support/webhook.rs`（`use crate::fake_http` を `use super::fake_http` に直し、src の unit 試験からも読めるようにした） | `#[path]` |
| D25 `load` | `src/config/jev.rs`・`routing_mode.rs`・`script.rs` の `tests` | 寄せた | 新しい `src/config/test_support.rs`（`#[cfg(test)] mod test_support;`） | `use crate::config::test_support::load` |
| D28 `write_executable` | `cap_probe_real_daemon.rs`・`ops_r1_measure.rs` | 寄せた | 新しい `crates/celeris/tests/support/executable.rs` | `#[path]` |
| D43 `write_executable` | `src/daemon/adapters.rs` の `harness_adapter_tests`・`src/model_discovery/tests.rs` | 寄せた | 新しい `crates/celeris/src/test_support.rs`（`#[cfg(test)] mod test_support;`） | `use crate::test_support::write_executable` |
| D37 `is_timeout` | `script_compare_cos_digest.rs`・`script_generation_e2e.rs`（どちらも `CosDigestCompareRecord` を受ける） | 同じ型の 2 つを寄せた | `support/script_compare.rs` の `is_cos_digest_timeout` | `mod support;`（2 file とも元から読む） |
| D37 の残り | `support/script_compare.rs` の `is_timeout`（`ScriptCompareRecord` を受ける） | 残した | — | — |

- D37 の残した理由: 受ける記録の型が `ScriptCompareRecord` と `CosDigestCompareRecord` で違い、同じ関数ではない。
  2 つの型を 1 つの関数で受けるには trait か generic が要り、2 行の判定のために型を足すのは見合わない。
  型の同じ 2 つの写しは本体が同じで、2 file とも元から `mod support;` で `script_compare.rs` を読んでいるので、build の負荷を
  増やさずに寄せられた。
- D28 と D43 は別の組のまま 2 つの helper にした。D43 は `#!/bin/sh` の行を足して書き、D28 は本文をそのまま書く。
  結合試験（tests/）と unit 試験（src/）は同じ module を読めないので、置き場所も別になる。
- D09 で `support/webhook.rs` の `Hook` は `seen`（応答のたびに 1 つ届く受け口）を持つ。`credential_wait_notify.rs` と
  `classify_tick::e2e` は読まないので、その `mod webhook;` に `#[allow(dead_code)]` を付けた。`seen` は unbounded channel で、
  読まなくても送り手は止まらない。`campaign_wiring_e2e.rs` も `campaign_e2e` の `ev_started` だけを使うので同じ allow を付けた。

### 候補外で残したもの（根拠の検索）
寄せた後に `grep -rn` で各 helper の定義を crates/task-api と crates/celeris で探した。
- `fn count(env: &TestEnv`・`fn env_with_attachments`・`fn bridge_binary`・`fn seq(events: Vec<Event>)`・`fn dechunk`・
  `fn ev_finished`・`async fn hook() -> Hook` → 寄せ先の 1 か所だけ。
- `fn load(extra: &str)` → 寄せ先のほかに `config/routing_eval.rs`・`config/maintenance_tests.rs`・`config/cron_tests.rs` に残る。
  3 つとも本体が違う（`BASE` の定数を前に置く、maintenance は `[workspace]` の欄も持つ、cron は `TempDir` も返す）ので、
  候補にも無く、寄せていない。
- `fn started(run: &str, role` → `task-api/src/cos/basis/review_rework.rs` の `started` が残る。adapter `fake` の別の雛形で、
  `ev_started`（adapter `claude-code`）と値が違う。
- `fn write_executable` → 上の 2 つ（D28・D43）だけ。

## 試験数（`cargo nextest list -p task-api -p celeris --message-format json`、寄せる前と後）
全体: 113 binary、1703 件 → 113 binary、1703 件。試験名の集合の差は 0 件。触った file を含む binary:

| binary | 前 | 後 |
|---|---|---|
| `task-api`（lib の unit 試験。D46） | 104 | 104 |
| `task-api::browser_e2e`・`browser_h3_injection` | 5・3 | 5・3 |
| `task-api::cos_chat_attach_pin`・`cos_live_fix_d1`・`cos_live_fix_d2`・`cos_operations` | 1・3・3・8 | 1・3・3・8 |
| `task-api::cos_delivery_suite_a`・`cos_delivery_suite_b`・`unmapped_suite_b` | 51・36・28 | 51・36・28 |
| `celeris`（lib の unit 試験。D09・D25・D43） | 536 | 536 |
| `celeris::campaign_acceptance_e2e`・`campaign_lock_e2e`・`campaign_wiring_e2e` | 9・4・1 | 9・4・1 |
| `celeris::cap_probe_real_daemon`・`ops_r1_measure`・`credential_wait_notify` | 6・1・2 | 6・1・2 |
| `celeris::cos_delivery_suite`・`root_done_summary`（`webhook.rs` を読む） | 28・4 | 28・4 |
| `celeris::script_compare_cos_digest`・`script_compare_lane_rules`・`script_generation_e2e` | 9・5・1 | 9・5・1 |

`task-api` の試験 binary はすべて `common/` を読むので、上に無い binary も前と後で同じ件数（全 113 binary が一致）。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・
  task-dispatch・task-worker と web。nextest `6153 tests run: 6153 passed (13 slow), 22 skipped`。script bundle の SKIP は 0 件。
  web の vitest `Test Files 103 passed`・`Tests 714 passed`。repo-health は `verdict=pass`（findings=717、new=0、worsened=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `git diff --quiet $(git merge-base HEAD main) -- crates/campaign crates/routing-eval crates/jev-classifier crates/script-runtime control-ts scripts/campaign crates/task-core/migrations crates/task-core/src/model/events.rs crates/task-core/src/script_guard.rs crates/celeris-credentiald && git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R6 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`
  → exit 0（`write-set R6: ok`）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `crates/task-api/{src,tests}/`・`crates/celeris/{src,tests}/` と
  この file だけ（範囲の外は 0 件）。
- `cargo build -p task-api --tests`・`cargo build -p celeris --tests` → 警告 0（deno_core の既存の `set_linger` の警告を除く）。

## commit
- `phase r6: test-dedupe: R5 から回った試験の helper の写し 12 組を tests/common・support・cfg(test) の 1 つに寄せる`
- `phase r6: test-dedupe の進捗を書く`

## 未解決
- 無し。限定 mutation は回していない。この unit は試験関数を消しておらず、寄せた helper は写しと同じ本体なので、
  R5 の規則（試験を消すときに mutation で検出力を示す）には当たらない。

## 提案
- `config/routing_eval.rs`・`maintenance_tests.rs`・`cron_tests.rs` の `load` は、前に置く config の本文と返り値が違う。
  `config::test_support::load` に前置きの本文を引数で渡す形にすれば 1 つにできるが、R5 の候補に無いので、次の健全性の走査で
  `dup.test-fixture` の候補になるかを見てから扱う。
