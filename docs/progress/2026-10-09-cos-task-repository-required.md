# CoS 起票のリポジトリ検査と案件の後付け

---
tasks: [01M4F5MBRPYGEK75001AMJKZ6Z]
---

設計: [ADR](../adr/2026-10-09-cos-task-repository-required.md)。

## 実装

- `task-ops::repo_requirement`: coding、または objective・acceptance の repo コマンド/path を字句で検出する。
- `task-api`: CoS `task.create` は案件・repo 不足を 422 `repository_required` で拒否し、拒否の監査を保存する。
  人の POST は `celeris-warning` 応答ヘッダを返す。既定 status は従来の ready のまま。
- `task-ops::edit`: 未実行 draft/ready に加え、lease の無い blocked と子 task も project_id の後付けを受け付ける。
  親が案件を持つ場合は同じ案件に限る。repo の省略時は親、次に案件 primary を使う。
  保存 transaction 内でも状態と親の案件を確認し、dispatch と競合した後付けを拒否する。
- 計画の task unit から作る子の project/repos 継承は既存実装で成立していたため、回帰試験を追加した。
- `cos-operator` の起票節に規則・確認 API・JSON 例・既存 task の修復手順を追加（リポジトリ版 v4）。
  稼働 KB の旧版は読み取りだけ行い、旧版用の `kb-cos-operator.patch` と適用情報を run 成果物に保存した。

## 検証

- API 新規試験 3 件: CoS の拒否・監査と正常起票・再送の冪等性、人の警告、PATCH の原子性、blocked 子の修復と running の拒否。
- ops 新規試験: 字句判定、旧 actions での拒否、blocked 子の案件・repo 後付け、task unit の継承。
- dispatcher 新規試験: 一時ローカル git repo を登録し、実行済み blocked 子に案件と repo を後付け。
  回答して通常 dispatch すると worker の cwd と判定コマンドが専用 worktree を使い、追跡ファイルと旧 run の成果物が残ることを確認した。
- store 新規試験: 後付けの読み取り後に lease を取得する順序を固定し、競合で task と event が書かれないことを確認する。

API 3 件と workspace 実行試験 1 件は exit 0。
試験は一時 DB・ローカル git・fake worker を使用し、LLM・外部ネットワーク・負荷注入は使わない。

## 本番への適用

本番の設定・DB・daemon・KB は変更していない。コード配送後、運用者がリポジトリ版 skill を KB へ取り込む。
既存の blocked task は `PATCH /api/v1/tasks/<id>` に `project_id` と `repos` を送り、応答の task を確認してから
既存の回答操作で再開する。PATCH だけでは blocked を解除しない。CoS の操作表には task PATCH が無いため、人の管理 API で行う。


## 全体検査の経過

初回は 4,885 passed / 3 failed / 14 ignored（exit 100）。旧 CoS actions の成功 fixture が案件・repo を
持たない coding task を起票していたため、登録済み repo を渡す形へ修正した。該当 3 試験は単独で全件合格。
2 回目は 4,887 passed / 1 failed / 14 ignored（exit 100）。既存の
`reviewer_run_shares_concurrency_and_is_deferred_when_at_capacity` が固定 250ms 待ちで worker 完了を仮定していた。
既存の `await_worker_completion` に置き換え、adapter の遅延も除去した。単独試験は合格（commit `ba944985`）。

全体試験は Unix socket のパス長制限を避けるため、隔離 user/mount namespace 内で run の `$TMPDIR` を `/tmp` に
bind し、UID を維持して、capability を除去してから実行した。実ファイルは run の TMPDIR 内にあり、host の
`/tmp`・本番サービスは変更しない。CARGO_TARGET_DIR 等のコンパイル設定は引き継いだ値のまま。

```sh
unshare --user --map-current-user --keep-caps --mount --propagation private bash -euc '
  mount --bind "$TMPDIR" /tmp
  test "$(stat -Lc %d:%i /tmp)" = "$(stat -Lc %d:%i "$TMPDIR")"
  setpriv --inh-caps=-all --ambient-caps=-all --bounding-set=-all \
    env TMPDIR=/tmp CELERIS_TEST_JOBS=2 bash scripts/dev/test-parallel.sh
'
```

生成型は API の `UPDATE_SCHEMA=1 cargo test -p task-api schema --lib`、Web の生成スクリプト、
GUI の `json2ts` で更新した。GUI の pnpm install は共有 store が read-only のため実行できず、
既存の GUI の json2ts を読み取り専用で使い、gen-types.mjs と同じ引数でこの worktree の出力だけを生成した。
生成差分は project_id の説明だけ。API 文書の GUI 側の写しは `scripts/sync-gui-docs.sh` で同期した。


3 回目は既存の `db_maintenance::tests::spawned_tasks_run_on_their_interval_and_stop_cleanly` が失敗した。
300ms 後に stop すると最初の backup コピーを cancel する順序になり得たため、atomic rename 後の backup ファイルが
現れてから停止する試験へ変更した（保険の上限 60 秒、負荷注入なし）。単独試験は合格（commit `32e72aa4`）。製品コードは変更していない。


## 最終結果

- `bash scripts/dev/test-parallel.sh`: **exit 0、4,888 passed / 0 failed / 14 ignored**。
  同時実行 2、nextest 157 binaries + doctest 10 crates、nextest 402.4 秒・doctest 10.4 秒。一時ファイル残骸 0。
  run 成果物の `test-parallel-complete.log` が最終ログ。
- `cargo clippy --workspace -- -D warnings`: **exit 0**（`clippy.log`）。以後の Rust 変更は上記の既存試験の修正のみ。
- `cargo build -p celeris -p celerisctl`: exit 0（e2e 用バイナリの準備）。
- `cargo fmt --all -- --check`、`git diff --check`、API schema の一致試験、Web 生成 schema の `--check`: exit 0。
- `scripts/sync-gui-docs.sh --check`、文書レイアウトと変更文書のリンク検査: exit 0。
- cos-operator の skill validator と、旧 KB 読み取り snapshot に対する `git apply --check`: exit 0。

CoS API の拒否・監査・成功、blocked 子への PATCH、次 run の worktree 配置、計画子への継承、保存時競合の拒否を
含む全体検査を完了した。本番適用・実 LLM での起票は実施していない。
