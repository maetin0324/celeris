---
title: 保存層（task-core）: repo の auto_integrate 欄と案件の欄の削除
tasks: [01M7BT5CNT1PRK0JM3GK60BHSF]
status: done
updated: 2026-11-14
---
# 保存層（task-core）: repo の auto_integrate 欄と案件の欄の削除

完了日: 2026-11-14（ADR 2026-11-14-repo-auto-integrate D1/D2/D4/D7/D8 の task-core 分）

## 変更
- migration 0126（ADR D7 の SQL）と `migration_registry.rs` の登録、`SCHEMA_VERSION` を 126 へ。
- `ProjectRepo.auto_integrate: bool`（serde 既定 false）、`project_repos` の読み書きの列。
- `RepoError::AutoIntegrateNotLocalGit` を新設。`validate_upsert` が、kind = git かつ location = local 以外で
  auto_integrate = true になる行を拒否する。store の `repo_create_tx`・`repo_update_tx` がこれを通るので、
  API・CoS・CLI のどの経路も同じ拒否になる（API の 422 への写しは後続 unit）。
- 削除: `Project.auto_integrate`、`SnapshotProject.auto_integrate`、`project_set_auto_integrate`（trait・impl・`_tx`）、
  案件の行を読む・書く SQL の列。
- `DeliverySkipReason::ProjectNotEnabled` → `RepoNotEnabled`（code `repo_not_enabled`）。旧 `project_not_enabled` は
  この enum の `#[serde(alias)]` 1 か所だけで読み替える（events は書き換えない）。
- 0012 の backfill（`backfill_project_repos`）は 0126 より前のスキーマで走るので、`repo_write_tx` ではなく 0012 の列だけを書く INSERT にした。
- 試験: migration 0126（auto_integrate=1 の案件の local git の行だけ 1、remote・dir・無効案件の行は 0、案件の列が消える）、
  store の拒否（remote・dir の作成、local→remote への更新、remote の行を有効にする更新。拒否は何も書かない）、
  旧 reason の alias。schema を巻き戻す既存の試験（cluster_job・cron・roadmap・script_domains）は新しい列に合わせた。
- 生成物: `docs/api/v1/event.schema.json`（reason の enum 名。task-core の schema 試験が要求）、
  `control-ts/fixtures/project_controller/invariants.jsonl`（`SnapshotProject` の欄を消したため）。`control-ts/src/` の型は未変更（後続 unit の範囲）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo test -p task-core` → lib 1357 passed、script_fixtures 15 passed（下の「未解決」の clippy 注記を参照）
- 他 crate は触っていない。`ProjectRepo`・`Project`・`project_set_auto_integrate` を使う crate は次の unit が直すので、
  この WU 単独では workspace 全体は compile しない。

## 未解決
- clippy の既存の指摘（`cluster_job/tests.rs`・`chat/store_tests.rs`）は同じ変更で直した。`cargo clippy -p task-core --all-targets -- -D warnings` と `cargo fmt --all -- --check` は exit 0。
- `cargo clippy --workspace -- -D warnings` は他 crate の置き換え後に通す。

## 本番への移行
- 移行の前に、有効な案件の remote・dir の行を数える:
  `SELECT r.id, r.name, r.kind, json_extract(r.location_json,'$.kind') FROM project_repos r JOIN projects p ON p.id = r.project_id WHERE p.auto_integrate = 1 AND NOT (r.kind='git' AND json_extract(r.location_json,'$.kind')='local');`
  該当行の task は移行後に reviewer 必須でなくなる。
- `benchfs-paper` を有効にするのは人の操作（後続の API unit の後）: `PATCH /api/v1/repos/01M7A3NAAKPM9ECBE2CR14V0BC` に `{"auto_integrate": true}`。

## 提案
なし。

## 生成物の再生成（check 不合格の対応）
- `cargo test -p task-core` が `store::tests::event_row_schema_matches_committed` と `script_fixtures_project_controller_match_committed_fixture` で落ちた。理由は、理由名の改名と案件の欄の削除に対し、コミット済みの生成物が古かったこと。
- `UPDATE_SCHEMA=1 cargo test -p task-core event_row_schema` で `docs/api/v1/event.schema.json` を、`dump-project-controller-fixtures` で `control-ts/fixtures/project_controller/invariants.jsonl` を作り直した。
- 提出時の HEAD で `cargo test -p task-core` は全て成功（1357 + 1 + 15 + 3 件）。`cargo fmt --all -- --check` も成功。
