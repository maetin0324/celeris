# Workspace 内の Cargo target 蓄積対策

---
tasks: [01M4FDM87P9QJEBSYW5XPZFM7T]
---

設計: [build/tmp hygiene ADR の 2026-10-09 付記](../../agent-docs/adr/2026-10-07-build-tmp-hygiene.md)。
運用: [build/tmp hygiene](../ops/build-tmp-hygiene.md#2026-10-09-workspace-target-対策の運用)。

## 実装

- 通常 worker・WorkUnit・子 task の既存 scratch 割り当てに reviewer を揃え、adapter env と `RunRequest.cargo_target_dir` の両方に記録する。owner は task ごと、並列 WU は WU ごと。既存の scratch GC の上限を使う。
- daemon の target sweep に scratch owner 配置を追加。DB で終端と確認できる task の target の古さ・上限を判定し、非終端・不明・外部 owner を保護する。pool lock と Cargo lock を使用する。
- 終端から既定 6 時間の workspace target を掃除する。通常 repo・旧 tree・WU の repo を対象とし、running run、Cargo lock、symlink を保護。専用 tombstone のみ再回収し、ソースと成果物を残す。
- 既存 scratch GC、WU キャッシュ掃除、workspace prune、cancelled worktree の片付けにも Cargo lock 確認を追加する。
- warn 通知に大きい target 上位 5 件を載せ、critical では新しい coding run とそのレビュー・検査を保留する。attempts は消費せず、使用率回復後に再開。保守 executor は critical の保留対象外。既存の `min_free_disk_mb` による全体保留は維持する。
- 新しい scope 設定は起動時・reload 時に反映する。監視試験は偽 probe/census、削除試験は小さな一時 fixture と固定時計を使う。

## 検証

- 対象試験: `cargo nextest run --offline -p task-dispatch -p task-worker -p celeris -E 'test(target_sweep) | test(workspace_targets) | test(workspace_prune) | test(disk_watch) | test(cargo_target) | test(every_cargo_path) | test(gc_dry_run_lists) | test(reviewer_uses_department_identity)' --no-fail-fast` — 64 passed（追加の安全性試験と最終変更は下記全体試験に含む）。
- `bash scripts/dev/test-parallel.sh` — exit 0。nextest 4,900 passed / 13 skipped、doc-test 成功（1 ignored）、計 167 binaries、failed 0、TMPDIR 残骸 0。
- `cargo clippy --workspace -- -D warnings` — exit 0、警告なし（`CARGO_NET_OFFLINE=true`）。

全体試験の前に `cargo build --offline -p celeris -p celerisctl` を実行する。長い run TMPDIR が既存 Unix socket 試験の制限に当たるため、試験専用 user/mount namespace 内だけで run TMPDIR を `/tmp` に bind する。UID は `--map-current-user` で維持し、mount 後に `setpriv` で capability を除去する。`CARGO_TARGET_DIR`・`CARGO_INCREMENTAL`・`CARGO_PROFILE_DEV_DEBUG`・compiler wrapper は受領値のまま。Cargo は offline で実行する。

ログは `/local/celeris/data/workspaces/01M4FDM87P9QJEBSYW5XPZFM7T/artifacts` の `focused-tests.log`、`test-parallel.log`、`clippy.log`。

`cargo fmt --all -- --check` と `git diff --check` も成功。

## 適用範囲

本番設定・DB・サービス・cron は変更していない。有効化は配送後に運用担当者が実施する。DB を開かない `celerisctl target sweep` は既存の共有 root のみで、scratch と workspace を含む正確な dry-run は daemon の保守 task を使う。

`shared_build_cache = false` と remote/container の既存の除外は維持する。上限は安全に回収できる項目に対するもので、稼働中 target を強制削除する hard quota ではない。
