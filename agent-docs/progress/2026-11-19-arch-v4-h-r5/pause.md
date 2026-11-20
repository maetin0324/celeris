---
title: アーキ v4 H R5 pause — lifecycle.rs の task_pause_op を plan と効果の形へ移す
tasks: [01M7VJY8NZ5G84ACBNQTPVV3R4]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# pause（WorkUnit）

完了日: 2026-11-20

## 0. 結論

- `crates/task-api/src/lifecycle.rs` の `task_pause_op`（`Option<&OperationAudit>` を受けて `Applied` を返す関数）を消し、
  plan の `plan_task_pause(store, task_id, pause, by, now) -> Result<(Task, Event), ApiProblem>` を置いた（ADR 2026-11-20-applied-shape-migration D1 の名前と引数のまま）。
  `lifecycle.rs` から `use crate::cos::operations::{Applied, OperationAudit};` を消した。`lifecycle.rs` の `crate::cos` の参照は 0 になった。
- HTTP の `task_action` は `plan_task_pause`（`by` は `caller.actor()`）→ `SqliteStore::update_task`（`store_problem`）→
  `task_ops::lifecycle::pause_result`（`ops_problem`）の順に呼ぶ。書き込みは前と同じ `update_task` で、書き直していない。
- CoS の `task.pause`・`task.resume`（`crates/task-api/src/cos/ops/tasks.rs`）は `plan_task_pause`（`by` は `audit.by()`）の拒否を
  `audit.reject(store, "task", <id>, problem)` で、書き込みを `audit.apply(store, "task", <id>, action, |tx| edit_task_tx(..))` で包む。
  `result` は前と同じ `{"task_id", "paused"}`。`audited(crate::lifecycle::…)` の呼び出しは無くなった。
- `docs/architecture-map.md` の lifecycle の行を書き替えた。「task の一時停止は `task_pause_op`」を「plan の `plan_task_pause` の後に
  `SqliteStore::update_task`」にし、CoS の腕 `crates/task-api/src/cos/ops/tasks.rs`（`OperationAudit::reject`・`apply`）を足し、試験の欄に
  `applied_contract.rs` を足した。
- `crates/task-api/tests/`・`cos/operations.rs`・task-ops の `lifecycle.rs`・`project_action`・policy の file は変えていない。

## 1. 変えた file

- `crates/task-api/src/lifecycle.rs`: `task_pause_op` を消し、`plan_task_pause` と `task_pause_trigger`（拒否の trigger
  `task_pause`｜`task_resume`。HTTP の tracing と `pause_result` の写しも使う）を置いた。`task_action` は plan → `update_task` → `pause_result`。
  旧い doc comment「handler（`audit = None`、認証済み principal）と CoS の `/cos/operations` … が共有する」は、plan と書き込みの分担
  （HTTP は `update_task`、CoS は `cos/ops/tasks.rs` が監査の transaction の中で `edit_task_tx`）の説明に書き替えた。
- `crates/task-api/src/cos/ops/tasks.rs`: `task.pause`・`task.resume` の腕（上の 0 節）。新しい `use` は足していない
  （`crate::lifecycle::plan_task_pause`・`time::OffsetDateTime` は完全修飾で呼ぶ）。
- `docs/architecture-map.md`: lifecycle の行（上の 0 節）。
- この file。

## 2. ADR との違い・objective との違い

- **objective との違い（`apply_checked` ではなく `apply`）**: objective の文面は CoS の書き込みを `apply_checked` で包み、
  architecture-map に `apply_checked` と書くよう求める。この WorkUnit は ADR D3・D4 の決定どおり、前と同じ `apply` を残した。
  理由は objective の上位の条件「API・CLI・DB の契約（応答の JSON・status・error code・表）を変えない」と ADR D3 の「拒否理由の文言を変えない」。
  `apply_checked` にすると、`edit_task_tx` が transaction の中で落ちたときの扱いが 2 つ変わる。
  1. 応答: 前は `chat_problem(ChatError::Store(e))`（code `chat_store_error`）。`apply_checked` に `store_problem` を渡すと `internal`・`db_busy` などになる。
  2. rejected の行の理由: 前は `StoreError` の表示文。`apply_checked` では `invalid: <detail>`（`ChatError::Invalid` の表示文）になる。

  WorkUnit `adr` の要点（[adr.md](adr.md) の「objective との差」）にも同じ判断がある。task の行・`Edited` event・監査の行が同じ transaction に入る点は、
  `apply` でも `apply_checked` でも同じ。architecture-map にはコードのとおり `apply` と書いた。
- **ADR との違い**: 無し。ADR の D1（`plan_task_pause` の名前と引数。task-api に効果の関数を置かず、task-core の `edit_task_tx` を使う）、
  D2（HTTP は `update_task` → `pause_result`）、D3（CoS は `reject` → `apply`）のとおり。D1 に無い私的な補助 `task_pause_trigger` を 1 つ足した
  （trigger の文字列を plan と `task_action` の 2 か所で書かないため。`pub(crate)` ではない）。

## 3. 試験・検査の結果（提出時の HEAD で実行）

基準は `$CELERIS_WU_BASE`（WU の開始時の snapshot。内容は `integrate wu/measure (phase design)` と同じ）。

| 検査 | コマンド | 結果 |
|---|---|---|
| 範囲 | WU の範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を許可の 4 file と照合） | exit 0。変えた path は `lifecycle.rs`・`cos/ops/tasks.rs`・`docs/architecture-map.md`・この file の 4 つ |
| 語と呼び出し | `! git grep -nwE 'OperationAudit\|Applied' -- crates/task-api/src/lifecycle.rs` ほか WU の否定の grep、architecture-map の行、`sh scripts/dev/check-doc-links.sh` | exit 0（`check-doc-links: ok`） |
| 依存 | `lifecycle.rs` の `crate::(cos\|handlers)…` の path が base の集合に無いものを出す check | exit 0（新しい path は無い。`crate::cos::operations` は消え、`crate::handlers::parse_project_id` だけが残る） |
| 契約試験 | `cargo nextest run --no-tests=fail -p task-api -E 'test(applied_contract_task_pause_)'` | exit 0、2 tests passed（試験は無変更） |
| 既存の pause の試験 | `cargo nextest run --no-tests=fail -p task-api -E 'test(applied_contract_task_pause_) \| test(task_pause_and_resume_cover_the_subtree) \| test(cos_ops_mutations_task_edit_pause_resume_reopen_retry)'` | exit 0、4 tests passed |
| fmt | `cargo fmt --all -- --check` | exit 0 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| test-affected | `bash scripts/dev/test-affected.sh`（対象: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web の typecheck+unit） | exit 0。nextest 6158 passed・21 skipped、web unit 705 passed（101 files）、`repo_health_gate: all checks passed` |

反例の確認: CoS の腕を一時的に壊し、`audit.apply` の前（transaction の外）で `store.update_task` を呼ぶ形にして
`applied_contract_task_pause_cos_same_transaction_and_rejections` を走らせた。試験は `監査の行が書けなければ task も止めない`
（`applied_contract.rs:750`）で FAIL した。その後、壊す前の file の写しで元に戻し、上の 4 本が pass することを確かめた。

## 4. Applied の形の数（この WorkUnit の分だけ。task 全体の after は WorkUnit `after` が測る）

| metric | base（`$CELERIS_WU_BASE`） | この branch | 減った file |
|---|---|---|---|
| `applied_wide`（`git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src` の file 数） | 18 | 17 | `crates/task-api/src/lifecycle.rs` |
| `applied_narrow`（`git grep -l 'Option<&OperationAudit>' crates/task-api/src` の file 数） | 14 | 13 | 同じ |

## 5. 未解決事項

- ADR の付記の表（org-write の ADR）と、`apply` を残した判断を objective の文面とそろえる記述は、WorkUnit `after` が直す（objective の指示どおり、この WorkUnit は ADR を変えていない）。
- `crates/task-api/tests/cos_delivery_suite/applied_contract.rs` の冒頭の doc comment は `task_pause_op` の名前を挙げている。tests/ は変えない約束なので残した
  （試験の中身は関数名に依存しない）。

## 6. 提案

- 無し。
