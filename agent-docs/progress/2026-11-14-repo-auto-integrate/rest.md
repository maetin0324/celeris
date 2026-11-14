---
title: celeris・celerisctl・celeris-mcp・script-runtime: 残りの参照の置き換えと workspace の検査
tasks: [01M7BT5CNT1PRK0JM3GK60BHSF]
status: done
updated: 2026-11-14
---
# rest unit: 残りの参照の置き換えと workspace の検査

完了日: 2026-11-14（ADR 2026-11-14-repo-auto-integrate D1/D3/D8 の残り）

## 変更
- `celeris::delivery::tick_with`: delivery 行の task を読み、木の子を外した上で `task_ops::delivery::auto_integrates` が真のときだけ `advance` する。案件の欄の参照は無い。
- `celeris/src/delivery/tests.rs`: `tick_advances_only_repos_with_auto_integrate` が repo の欄を false→true に切り替え、止まる・進むを確かめる（`stored_delivery_with`・`merge_queued_with` は task が repo の行を使う fixture。他の試験は従来の fixture のまま）。
- `celerisctl projects show`: repo ごとに `auto_integrate=<値>` を出す（試験 `repo_lines_show_each_repos_auto_integrate`）。celerisctl は DB を読むだけなので、書き込みは `PATCH /api/v1/repos/{id}`。案件の欄を持つ旧い参照は無かった。
- celeris・celeris-mcp・script-runtime・`tests/e2e` の `Project.auto_integrate` fixture を削除し、`ProjectRepo` の literal に `auto_integrate: false` を足した。
- `celeris-campaign`: `ops_r1_trace` の受け付ける schema version に 126 を足した（この export は `projects`・`project_repos` を読まない）。`preflight_contract` は `TaskStore` の import と repo の `kind: git` を足した（有効化は local の git だけ）。

## 検査（提出時の HEAD で実行）
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0、4026 件成功（slow 7、skipped 11）。`celeris-campaign` と `celeris` の delivery・ops_r1 を含む。
- `cargo test -p celeris --lib delivery` → 50 件成功。
- `grep -rn auto_integrate crates` は repo の欄・migration・ADR 由来の語・API の 422 の説明だけ。web/ は触っていない。

## 未解決
なし。

## 本番への移行
1. 移行の前に（本番 DB の写しか読み取りで）有効な案件の remote・dir の行を数える:
   `SELECT r.id, r.name, r.kind, json_extract(r.location_json,'$.kind') FROM project_repos r JOIN projects p ON p.id = r.project_id WHERE p.auto_integrate = 1 AND NOT (r.kind='git' AND json_extract(r.location_json,'$.kind')='local');`
   該当行は移行後に自動取り込みの対象にならない。
2. release を promote して migration 0126 を適用する（人の操作）。
3. `benchfs-paper` を有効にする: `PATCH /api/v1/repos/01M7A3NAAKPM9ECBE2CR14V0BC` に `{"auto_integrate": true}`。確認は `celerisctl projects show <project id>` の repos 欄が `auto_integrate=true`。
4. 案件への `auto_integrate` の書き込みは 422 になる。

## 提案
なし。
