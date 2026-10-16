---
title: 案件ごとの自動取り込み（auto_integrate）
updated: 2026-10-16
status: done
tasks: [01M51M4CB94RD39Z20QPM0KGQD]
---

# 案件ごとの自動取り込み（auto_integrate）

設計は [ADR: 自動取り込みを案件の属性にする](../adr/2026-10-16-project-auto-integrate.md)。

完了日: 2026-10-16。

## 変更の要点

- migration 0083（schema 83）: `projects.auto_integrate INTEGER NOT NULL DEFAULT 0`。既存の config 値
  `01M2WTS3DKNZBSZ2JMVB4CZMBW` は同じ migration で 1 にする。`Project.auto_integrate`・`TaskStore::project_set_auto_integrate`。
- delivery: `begin`（task-ops）・`tick`・`advance`（celeris）・review 起動（task-dispatch）は案件行の `auto_integrate` を読む。
  取り込み先は task の repo 行の path。自己リポジトリ（`[selfdeploy] repo`）以外は `merge_reviewed` 成功で `Ready`（release なし）に
  なり、`prepare.sh`・promote に進まない。push は `[selfdeploy] push` かつ `push_remote` が実在するときだけ（remote なしは push しない）。
- API: `PATCH /api/v1/projects/{id} {auto_integrate}`。Web: 案件画面「自動取り込み」の切替（`web/features/projects/auto-integrate.tsx`）。
- 削除: `[selfdeploy] delivery_projects`（config 欄・`DeliveryPolicy.projects/repo`・reload の写し・検査）。書くと読み込みエラー。
  `docs/ops/selfdeploy.md`・`config/celeris.example.toml`・ADR-0051/0121 に追記。

## 証拠

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0、passed 5383 / failed 0（全出力: artifacts/test-parallel.log）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `cd web && corepack pnpm@12.6.0 install --frozen-lockfile && corepack pnpm@12.6.0 typecheck && corepack pnpm@12.6.0 test` → exit 0
  （vitest 全 pass、`features/projects/auto-integrate.test.tsx` が切替の PATCH と表示を固定）。
- 試験: `non_self_repo_is_merged_without_release_or_push`・`non_self_repo_with_a_remote_is_pushed_but_still_has_no_release`・
  `tick_advances_only_projects_with_auto_integrate`（celeris delivery）、`project_auto_integrate`（task-api）、
  `selfdeploy_delivery_projects_is_removed_and_rejected`（config）、task-ops delivery 試験（無効な案件は `project_not_enabled`）。

## 本番への移行

人が実行する（この task は本番に触っていない）。順序が重要で、**新 release を verify・昇格する前に config から `delivery_projects` を消す**
（残っていると新 binary は config の読み込みエラーで起動しない）。release.sh の verify 段（staging）も本番 config を読むため、行が残っていると check 1（start-and-migrate）が `unknown field delivery_projects` で落ちる（2026-10-16 の配送で実際に起きた）。旧 binary は欠けた欄を既定で読むので、先に消しても N-1 は動く。

1. `~/.config/celeris/config.toml` をバックアップし、`[selfdeploy]` の `delivery_projects = [...]` の行を削除する。
   `cp ~/.config/celeris/config.toml ~/.config/celeris/config.toml.before-auto-integrate` →
   `grep -n delivery_projects ~/.config/celeris/config.toml`（削除後は 0 件）。
2. release を verify・promote する（migration 0083 が自己改善案件 `01M2WTS3DKNZBSZ2JMVB4CZMBW` の `auto_integrate` を 1 にする）。
   確認: `sqlite3 -readonly <db> "select id, auto_integrate from projects where auto_integrate = 1"` に当該 ID が出る。
3. asmr-archive 案件（`01M4ZXVMKZKKZS1HHANZ9B0NPX`）を有効にする（operator が API で）:
   `curl -X PATCH -H "Authorization: Bearer $TOKEN" -d '{"auto_integrate": true}' http://<host>/api/v1/projects/01M4ZXVMKZKKZS1HHANZ9B0NPX`
   または Web の案件画面の「自動取り込み」。以後の root task は レビュー合格で asmr-archive の main へ取り込まれる（release・push なし）。

## 未解決事項

- 取り込み完了（release なし）の `Ready` は、通知文と `integrations` の行で見分ける。`DeliveryState` に専用の状態は足していない。
- task の repo が 2 件以上（`multiple_repos`）は従来どおり取り込まない。

## 提案

- 非 self repo の取り込み後に reviewed 済み worktree を片付ける仕組みは未着手（ADR-0051 のとおり証跡として保持）。
