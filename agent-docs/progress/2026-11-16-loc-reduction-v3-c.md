---
title: LoC 削減 v3 C — pilot 削減（TaskStore の trait と単一実装の store trait の撤去）
tasks: [01M7J3NKV7PY9DPT0K9D1VX4B7]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# LoC 削減 v3 C — pilot 削減（TaskStore の trait と単一実装の store trait の撤去）

完了日: 2026-11-16

親 01M7J3N68JRNTP5Q4G934QWF36（依頼キー loc-reduction-v3-20261116）の子 C。設計は [ADR 2026-11-16-taskstore-trait-removal](../adr/2026-11-16-taskstore-trait-removal.md)。監査は子 A の [進捗](2026-11-16-loc-reduction-v3-a.md) の候補 C1・C3。

## 結論
- production 全言語の純減は **2,701 行**（unit: `git merge-base HEAD main` → HEAD、固定: 開始時 main 415a19a7004cb6a73b872e861fa8d1cf634dddbb → HEAD とも同じ）。第 1 マイルストーン（1,500 行）を満たす。production Rust も 2,701 行、own_total_with_tests は unit で 2,838 行、固定で 2,426 行減った。
- 削った領域は A の候補の 2 つ: C1（TaskStore の trait と 144 個の 1 行転送）と C3（実装が SqliteStore だけの store trait 15 個）。あわせて、C1 で前提が変わった `OrgDirectory`・`TaskQuery` の本体を固有 method にし、具体型の呼び手の trait の import を消した。
- SQL・schema・migration・event・transaction の境界・lease の照合は変えていない。試験は 1 件も消していない（`#[test]` の削除 0、追加 0）。
- 第 2 マイルストーン（累積 5,000 行）と最終目標（10,000 行）は未達。今回の結果は第 1 マイルストーンだけを満たす。

## 消した対象と before/after
| 対象 | before | after |
|---|---|---|
| `TaskStore` の trait（`crates/task-core/src/store/task_store.rs`） | 1,181 行、fn 宣言 181、store の supertrait 18 | file を削除 |
| `impl TaskStore for SqliteStore`（`task_store_impl.rs`） | 1,299 行、うち 1 行転送 144 個 | 108 行（転送でない 9 method の固有 impl） |
| 単一実装の store trait 15 個（`ApprovalStore`・`BrowserWaitStore`・`ClusterJobWaitStore`・`CronJobStore`・`DeliveryStore`・`DiskWatchStore`・`NoticeStore`・`KnowledgeRunStore`・`McpClientStore`・`McpCallStore`・`NodeSessionStore`・`NotificationStore`・`ReportStore`・`ScriptInboxStore`・`ModelCatalogStore`） | 宣言と `impl X for SqliteStore` | 宣言を削除、`impl SqliteStore` の固有 method |
| `OrgDirectory`・`TaskQuery` | 本体は trait の impl、具体型の呼び手 100 余り file が trait を import | 本体は固有 method、trait の impl は 8 個の転送、import は偽物を使う celeris-mcp だけ |
| celeris-mcp の `McpState.node_sessions` | `store` を `NodeSessionStore` として見る 2 本目の handle | 削除（`store` を使う） |

残した trait と理由（ADR D3）: `OrgDirectory`・`TaskQuery`（celeris-mcp の偽物 `tools/fake_ports.rs`）、`ScriptDomainStore`（試験の偽物 `FlakyRollback`）、`CredentialWaitStore`（path class `credential`。設計 review の範囲を増やさない）、`KnowledgeUseStore`・`RequestLog`（A の候補の外）。

### 行数（`source-size-report.py --exclusive-classes`、分類規則は開始時 main の script と toml に固定）
| 時点 | production 全言語 | production Rust | own_total_with_tests |
|---|---:|---:|---:|
| 開始時 main（固定） | 386,338 | 340,223 | 1,043,479 |
| unit 基点（`git merge-base HEAD main`） | 386,338 | 340,223 | 1,043,891 |
| commit「TaskStore の trait と 144 個の 1 行転送、…を消し」の後 | 383,661 | 337,546 | 1,041,091 |
| HEAD | 383,637 | 337,522 | 1,041,053 |
| 純減（unit / 固定） | 2,701 / 2,701 | 2,701 / 2,701 | 2,838 / 2,426 |

- 固定と unit の差は own_total_with_tests の 412 行だけで、子 B が main に足した試験（reduction_flow など）の分。production は B で変わっていない。
- 差分（.rs、`git diff --numstat`、試験の file を含む）: C3 の trait の 15 file +304 −814、task-core の store の file（C1 の本体）+610 −2,747、呼び手 551 file +2,105 −2,344（大半は `TaskStore` の import と `dyn TaskStore` の型の書き換え）。

## 変更（commit の件名）
- `loc-reduction-v3 C: TaskStore の trait と単一実装の store trait の撤去の ADR を足し、ADR-0033 と consumer-ports に付記する` — 新しい ADR。ADR-0033 と ADR 2026-11-10-taskstore-consumer-ports に付記（CoS のコメントの依頼）。
- `loc-reduction-v3 C: TaskStore の trait と 144 個の 1 行転送、単一実装の store trait 15 個を消し、SqliteStore の固有 method にする` — C1・C3 の本体。`<method>_impl` を `pub fn <method>` に改名して転送を消し、呼び手の `&dyn TaskStore`・`S: TaskStore` を `&SqliteStore` にした。doc comment の `TaskStore::` を `SqliteStore::` に直し、生成 schema（`docs/api/v1/*.schema.json`・`docs/protocol/worker-protocol.schema.json`・`web/api/generated/schema.json`）を `UPDATE_SCHEMA=1` で作り直した（説明文の 1 か所だけが変わる）。
- `loc-reduction-v3 C: OrgDirectory・TaskQuery の本体を SqliteStore の固有 method にし、具体型の呼び手の trait の import を消す`

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 受け入れ条件 3 の size check（閾値 `1500 1 - - -`）→ exit 0、`OK []`。`net_reduction`: prod unit 2,701・fixed 2,701、rust 2,701・2,701、own unit 2,838・fixed 2,426。
- 同じ size check で HEAD の代わりに unit 基点を測った対照（純減 0）→ exit 1、`FAIL ['prod.unit=0<1500', 'own.unit=0<1']`。数値が届かない変更は決定的に落ちる。
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 8,546 passed・23 skipped、web の tsc と vitest 103 file・716 passed、ts-verify 20 passed。source-size gate 0 failure）。
- `cargo nextest run -p task-dispatch --lib -E 'test(reduction_flow)'` → 5 passed（B の配送と判定の試験: 純増・移動だけ・測定の欠損を拒否、本物の純減は合格）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `cargo clippy --workspace --all-targets -- -D warnings` は exit 101 だが、落ちる 4 file（`crates/celeris/tests/cap_probe_real_daemon.rs`・`crates/campaign/src/ops_r1_intake_tests.rs`・`crates/task-worker/src/adapter.rs`・`crates/campaign/tests/support_cases/mod.rs`）はこの task で変えていない（既存）。

## 受け入れ条件との対応
- 条件 0（2〜3 の独立した領域を実装で削除）: C1（`crates/task-core/src/store/` の TaskStore の trait と転送）と C3（`crates/task-core/src/` の 12 module と `store/model_catalog.rs` の store trait 15 個）。移動・改名・分割ではなく、宣言と転送を消した。before/after は上の表。
- 条件 1（配送と reviewer の数値判定）: この run の prompt（`runs/<run id>/prompt.txt`）には目的の全文、削減目標 1,500、基点 `base e19056a1dfb9`、size check の command と閾値の読み方が入っていた。ただし、汎用規約は 1〜10 までで、B が足した規約 11 は入っていなかった。この run を起動した daemon は B より前の release（ef0aa0a25a4f、13:03Z 起動）で、B を含む release（e19056a1dfb9）は 14:03Z に起動した。reviewer の prompt と `review.json` はこの run の後に作られるので、この run からは確かめられない（未確認）。数値が届かない変更の拒否は、上の対照（exit 1）と B の reduction_flow の 5 試験で示した。
- 条件 2（安全を壊していない）: SQL・migration・event・transaction・lease は変えていない。試験は消していない。trait を残すか消すかは「2 つ目の実装（試験の偽物）があるか」で決め、偽物のある trait は全部残した。`CredentialWaitStore`（credential）は触っていない。
- 高 risk の path class で変えた file: `store`（task-core の store 全体。`_impl` の改名と転送の撤去）、`credential`（`store/credential_wait.rs` の doc 1 行、`credential_wait_tests.rs` の import、`dispatcher/credential_gate.rs` の引数の型、`credential_resume.rs`）、`lease`（`dispatcher/leases.rs` の doc 1 行、`run_supervisor.rs` の import と引数の型、`task-core`・`celeris` の `instance.rs`）、`budget`（`token_budget.rs`・`quota_book.rs`・`routing_runtime/quota.rs`）、`selfdeploy`（`task-api/src/releases.rs` の trait の import 2 つ）。store 以外はどれも型・import・doc の書き換えだけで、分岐と SQL は変えていない（`git diff` で確認）。設計は ADR 2026-11-16-taskstore-trait-removal。
- 条件 3（size check）: 上の証拠のとおり exit 0。

## 未解決
- 条件 1 の reviewer 側（規約 11 を含む prompt と `review.json` の数値の判定）は、この run の後の review で確かめる必要がある。

## 提案
- `scripts/dev/repo-health/profiles/celeris.toml` の構造指標 `task_store_fn_decls`・`task_store_supertraits` は、数える file（`task_store.rs`）が無くなり `None` になる。profile は policy の file で、変えると `policy.changed` が別の review を求めるので、この task では変えなかった。別の review で 2 つの指標を消すこと。同じ file の `size.responsibility-growth` の説明（「TaskStore（…task_store.rs）」）も直す。
- D 以降の候補: `CredentialWaitStore`（設計 review が要る）、`KnowledgeUseStore`・`RequestLog` も実装は 1 つ。偽物が無ければ同じ形で消せる。

## 本番への移行
- 無し（config・DB・API の変更は無い）。
