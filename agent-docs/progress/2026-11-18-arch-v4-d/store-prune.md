---
title: アーキ v4 D store-prune — production の呼び出し元が無くなった SqliteStore の org の wrapper を消し、それを名指す文書の行を直す
tasks: [01M7MZTKB4RYM80S6CT3RST2N9]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# store-prune（WorkUnit）

完了日: 2026-11-18

usecase（`crates/task-api/src/org_write.rs` への集約）の後に、production の呼び出し元が 0 になった
`SqliteStore` の org の wrapper 3 つを消し、旧い名前を名指していた doc comment と文書の行を直した。
`org_upsert`（`org_directory.rs`、celeris-mcp と `OrgDirectory` が使う）・`org_upsert_tx`・`org_delete_tx` は残した。
基準: 範囲の check は `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## commit（件名）
- `arch-v4 D store-prune: SqliteStore::org_upsert_browser_settings・org_upsert_with_actor・org_delete を消し、org_delete の試験を write_tx と org_delete_tx に直し、旧い名前の文書を直す`
- `arch-v4 D store-prune: 進捗 store-prune.md を書く`

## 実装前 brief
1. 根本原因: usecase で HTTP と CoS の org の書き込みが `org_write::apply`（`write_tx` の中で `org_upsert_tx`・`org_delete_tx` を呼ぶ）に集まり、
   自分で transaction を開く wrapper `org_upsert_browser_settings`・`org_upsert_with_actor`・`org_delete` は production から呼ばれなくなった。
   残すと「org を書く入口が 2 つある」ように見え、文書（api.md・browser-department.md）も旧い入口を実装として名指していた。
2. 責務 owner: `crates/task-core/src/store/org.rs`（store の org の SQL）。文書の行は `docs/api/v1/api.md` の表と `docs/ops/browser-department.md`。
3. 既存の再利用先: `SqliteStore::write_tx`（`store/mod.rs`）と `SqliteStore::org_delete_tx`。HTTP と CoS と同じ形で試験を書く。
4. 選択肢: (a) wrapper を残し `#[cfg(test)]` にする、(b) 消して試験を `write_tx` + `org_delete_tx` に直す。
   (a) は試験のためだけの入口が残り、production と違う経路を試験することになるので (b) を採った。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `SqliteStore::org_upsert_browser_settings`（pub）・`org_upsert_with_actor`（private）・`org_delete`（pub）、
   それらを名指す doc comment 2 行（`org_upsert_tx` の「as `org_upsert_browser_settings` does」、`org_delete_tx` の「Same refusals as `org_delete`」）、
   `docs/api/v1/api.md` の表の 41〜43 行の「store `org_upsert`」「store `org_delete`」、`docs/ops/browser-department.md` 112 行の `org_upsert_browser_settings`。
7. 不変条件: 組織ノードの削除の拒否（未終了 task の assignee なら `InUse`、子ノードがあれば `InUse`、無い id は `Ok(false)`）と、
   拒否のとき何も消えないこと（`task-state-in-sqlite`・`no-data-loss`）。`store::tests::org_delete_tx_refuses_while_a_task_is_open_or_children_remain` で確かめる。
   browser 設定の監査行が同じ tx で書かれること（`cos-audit-same-tx`）は usecase の契約試験（`cos_delivery_suite/org_write_contract.rs`）が確かめ、この unit は変えていない。

## 呼び出し元の確認（消す前）
`git grep -nwF -e org_upsert_browser_settings -e org_upsert_with_actor -e org_delete -- crates docs`（base で実行）の当たり:
- `crates/task-core/src/store/org.rs`: 3 関数の定義、`org_upsert_browser_settings` → `org_upsert_with_actor` の呼び出し 1 か所、doc comment 2 行。
- `crates/task-core/src/store/tests.rs` 2813〜2825 行: `org_delete` の試験 4 か所（試験だけ）。
- `crates/task-api/src/handlers/org.rs` 144 行: `tracing::info!(… op = "org_delete" …)` の文字列。関数の呼び出しではなく log の操作名なので残した（task-api はこの unit の範囲外）。
- `docs/api/v1/api.md` 408 行、`docs/ops/browser-department.md` 112 行。

`org_upsert_with_actor` の呼び出し元は `org_upsert_browser_settings` だけだったので、一緒に消した。
`org_upsert`（`org_directory.rs` の `write_tx(|tx| org_upsert_tx(tx, node, None))`）は celeris-mcp・`OrgDirectory`・試験が使うので残した。

## 変えたもの
- `crates/task-core/src/store/org.rs`: 3 関数を消した（−33 行・+8 行。+8 は 2 つの doc comment の書き直し）。
  `org_upsert_tx` の doc comment は「HTTP `/org` と CoS `org.*` が `write_tx` の中の `task_api::org_write::apply` から呼ぶ。`org_upsert` は自分の tx で包む。
  `actor` があれば監査行を同じ tx で書く」に、`org_delete_tx` の doc comment は旧 `org_delete` の拒否の説明を移し、呼び出し元を `org_write::apply` と書いた。
- `crates/task-core/src/store/mod.rs`: module 一覧の `org.rs` の行に tx 内の書き込み `org_upsert_tx`/`org_delete_tx` と呼び出し元（`org_write::apply`）を書いた（旧い名前は元から無かった）。
- `crates/task-core/src/store/tests.rs`: 試験 `org_delete_refuses_while_a_task_is_open_or_children_remain` を
  `org_delete_tx_refuses_while_a_task_is_open_or_children_remain` に直した（下の節）。
- `docs/api/v1/api.md` 406〜408 行（表の 41・42・43）: 実装の列を「`org_write` の plan → `SqliteStore::write_tx` の中の `org_write::apply`（store `org_upsert_tx` / `org_delete_tx`）」にした。
- `docs/ops/browser-department.md` 112〜113 行: 監査行を書く関数を `crates/task-core/src/store/org.rs` の `org_upsert_tx`（HTTP と CoS の両方が `crates/task-api/src/org_write.rs` の `apply` から呼ぶ）にした。

## 試験の書き直しと、同じ不具合を検出できること
旧い試験は `store.org_delete(id)` で次の 4 つを確かめていた: 未終了 task の assignee なら `InUse`、子ノードを持つ部なら `InUse`、
task が終端になれば `Ok(true)` で消え `org_get` が `None`、無い id は `Ok(false)`。
新しい試験は `store.write_tx(|tx| SqliteStore::org_delete_tx(tx, id))`（HTTP と CoS の `org_write::apply` と同じ形）で同じ 4 つを確かめ、
さらに拒否の後に `org_get("poc")`・`org_get("coding")` が `Some` であること（拒否で何も消えない）を足した。

旧 `org_delete` と `write_tx` の違いは、`Ok(false)` のとき旧版は commit せず、`write_tx` は空の tx を commit すること。どちらも行を書かないので、観測できる結果は同じ。
`Err` のときはどちらも commit せずに tx を落とす（rollback）。

検出力の確認（`org_delete_tx` を 1 か所ずつ壊して `cargo nextest run -p task-core org_delete_tx` を走らせ、`git checkout -- crates/task-core/src/store/org.rs` で戻した）:

| 壊し方 | 結果 |
|---|---|
| 未終了 task の判定を `if open_tasks > 0 && false` にする | FAIL（1 件中 1 件） |
| 子ノードの判定を `if children > 0 && false` にする | FAIL（1 件中 1 件） |
| 無い id で `Ok(false)` の代わりに `Err(Invalid)` を返す | FAIL（1 件中 1 件） |
| 壊さない | PASS（1 件） |

## 検査（提出時の HEAD で実行）
- `git grep -nwF -e org_upsert_browser_settings -e org_upsert_with_actor -e org_delete -- crates docs`: 当たりは `crates/task-api/src/handlers/org.rs:144` の log の操作名 `op = "org_delete"` の 1 行だけ（定義・呼び出しは 0、docs は 0）。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 8466 件 pass（22 skipped）、doc test pass、web の tsc -b と unit 101 file・705 件 pass、test-ts-verify 20 件 pass。終了時に `/local/celeris/data/scratch/tr-1001/.trash-*/…/own-eval` の `rm: Permission denied` が 3 行出たが、exit は 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` と `git diff --stat $CELERIS_WU_BASE HEAD`: 変更は `crates/task-core/src/store/{mod,org,tests}.rs`・`docs/api/v1/api.md`・`docs/ops/browser-department.md`・この file だけ。
- 行数（`git diff --numstat $CELERIS_WU_BASE HEAD -- crates`）: `org.rs` +8 −33、`mod.rs` +3 −1、`tests.rs` +12 −5。Rust の合計は +23 −39（本体 +11 −34、試験 +12 −5）。

## 未解決
- `crates/task-api/src/handlers/org.rs:144` の log の操作名 `op = "org_delete"` は関数名ではないので残した。名前を揃えるかは task-api の範囲の判断で、この unit では変えていない。
- `store` は高 risk の path class（`^crates/task-core/src/store/`）。この unit は wrapper を消すだけで、tx の境界・拒否の条件・監査行の書き方は変えていない。final review の設計の判定はこの brief を材料にする。
- `metrics`（C2・C7b・C12）はこの unit では測っていない。

## 提案
無し。
