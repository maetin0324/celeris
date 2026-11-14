---
title: task-api: repo の auto_integrate 欄・422・案件の欄の削除・docs/api の生成物
tasks: [01M7BT5CNT1PRK0JM3GK60BHSF]
status: done
updated: 2026-11-14
---
# task-api: repo の auto_integrate 欄・422・案件の欄の削除

完了日: 2026-11-14（ADR 2026-11-14-repo-auto-integrate D1/D2/D6/D8 の task-api 分）

## 変更
- `RepoCreateBody.auto_integrate`（既定 false）・`RepoPatchBody.auto_integrate`（省略は変えない）。`plan_create`・`plan_patch` が
  行へ写し、条件（local の git だけ）は task-core の検証に任せる。`store_problem` が `StoreError::Repo` を 422 に写すので、
  API の追加の分岐は無い。location を remote に変える・kind を dir に変える要求で有効な行は、落とさずそのまま渡して 422 になる。
- 管理ログ: `PATCH /repos/{id}` は `op = "repo_update"` と書いた欄の名前 `fields`（`auto_integrate` を含む）を残す。作成のログにも値を足した。
- CoS: `repo.create`・`repo.update` は本文に `auto_integrate` があれば値によらず 422 で、拒否は操作の記録（rejected）に残る
  （`cos/ops/admin.rs` の `reject_auto_integrate`）。
- `PATCH /projects/{id}` の `auto_integrate`: `ProjectPatchBody` から削除。unknown field は全 API で 400 だが、受け入れ条件と ADR が 422 なので、
  handler で本文に `auto_integrate` があれば移す先を示す 422 を返す。
- `Project`・`project_set_auto_integrate_tx` の参照を削除（task-api に `projects.auto_integrate` の参照は残らない）。
- 生成物: `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema` で `docs/api/v1/api-v1.schema.json` を再生成。`docs/api/v1/api.md`（§3.48・§3.69・§3.70）は手で書き換えた。
- 試験: `tests/cos_delivery_suite/project_auto_integrate.rs` を `repo_auto_integrate.rs` に書き換え（repo の読み書き・一覧と
  `GET /projects/{id}` の repos[]・remote/dir の作成と更新が 422 で何も書かない・案件の欄が 422）。`cos_ops_admin_config` に CoS の拒否 2 件。
  他の fixture（`Project` の欄の削除、`ProjectRepo` の欄の追加）を置き換えた。

## 検査（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo test --no-fail-fast -p task-api` → 全 test binary が成功（`cos_delivery_suite` 21 件に新しい repo の試験 3 件を含む）。
- `cargo test -p celeris-campaign` → `preflight_contract` を含め成功。`ablation_application`（4 件）だけが、`celeris` crate が `Project.auto_integrate` を参照していて compile できず失敗。`celeris` は rest unit の範囲。
- `cargo clippy -p task-api -p celeris-campaign -- -D warnings` と `cargo fmt --all -- --check` は exit 0。
- `bash scripts/dev/test-affected.sh` は `celeris`・`celerisctl` を対象に含むが、この unit の時点では両 crate が compile しないため走らせていない（rest unit と統合の後に再実行）。web の型検査も同じく未実行。`web/api/generated/types.ts` と web の `auto_integrate` 参照は web を触らない方針のため未変更（次の task の範囲）。
- consumers が書き換えた `crates/campaign/tests/preflight_contract.rs` は、store の trait を import していない compile 誤りがあったので、`PATCH /repos/{id}` で repo を有効にする形に直した。`inbox_notifications` の R3 の試験は、受信箱が D4 により有効な repo の task だけを出すので、有効な repo を持つ task に直した。

## 未解決
なし。web・`control-ts/src/` の型は範囲外。

## 本番への移行
store の進捗ファイルの節に従う（`PATCH /api/v1/repos/<id>` に `{"auto_integrate": true}`）。案件への `auto_integrate` 書き込みは 422 になる。

## 提案
なし。
