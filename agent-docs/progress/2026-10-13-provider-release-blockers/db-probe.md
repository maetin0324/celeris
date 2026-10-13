# worker_db_read_only の run_db 調査

---
tasks: [01M4V68MRJWG2RPPQGK0ABZDXH]
---

## 原因と値の出所

`tests/e2e/tests/worker_db_read_only.rs:179-192` は `tempfile::tempdir()` 配下の `celeris.sqlite3` を作成し、その `db.display()` を埋め込んだ `$DB` に対して `celerisctl --db "$DB"` と raw append を実行する。従って `raw_db_write_exit=0` と `raw_wal_write_exit=0` は一時 DB とその WAL への書き込みを示し、本番 DB への書き込みではない。

試験出力の `run_db=/local/celeris/data/db/celeris.sqlite3` は、daemon がこの worker run 向けに設定した値ではない。`crates/task-dispatch/src/dispatcher/worker_task.rs:593-603` は `db_guard::installed()` がある場合だけ DB path を `followups_env` に渡す。`crates/task-dispatch/src/dispatcher/followups.rs:18-42` は task/run ID と followups file は常に追加するが、db_guard が無い場合 `CELERIS_RUN_DB` を追加しなかった。`worker_task.rs:54-62` が `with_env` 対応 adapter に追加 env を適用し、例えば `crates/task-worker/src/claude_code.rs:522-526` が起動するコマンドは親環境を継承する。そのため worker の子 process には、外側のこの run 自身の `CELERIS_RUN_DB` が残っていた。

同じ理由で外側の `CELERIS_FOLLOWUPS_FILE`、`CELERIS_TASK_ID`、`CELERIS_RUN_ID` も親環境から継承され得る。daemon が作る値は `followups.rs:26-38` の task/run ごとの値だが、値のない設定を env から削除していないことが欠陥だった。

## injected follow-up と本番影響

stub は `worker_db_read_only.rs:193` で `celerisctl --db "$DB" add --title injected ...` を呼ぶ。`crates/celerisctl/src/main.rs:252-258` は明示 `--db` を最優先し、`main.rs:260-278` の `followups_target` は `--db` 解決後の target と `CELERIS_RUN_DB` が一致するときだけ followups file を返す。`crates/celerisctl/src/commands/add.rs:213-220` はその file が返った場合に限り DB を開かずキューし、同 194-202 行の通常 `run` は指定 DB に task を作る。今回の明示 `--db` は一時 DB なので、本番 DB path との一致はなく、`injected` は外側 follow-up には流れず、一時 DB に書かれた。

指定された `/local/celeris/data/workspaces/01M4T94XWN31GW9SENK41GAP8F/artifacts` と `runs` の下を読み取り専用で `followups*.json` 検索したが、該当ファイルは存在せず、従って残存する `injected` 宣言も確認されなかった。本番 DB は開いていない。結論としてこの出力による本番 DB 書き込みは起きていない。

## 修正

db_guard がない場合、daemon が `CELERIS_RUN_DB` に空文字を明示して、adapter が継承 env を重ねるときにも外側の値を消す。`celerisctl` の `run_db()` は空値を無効として扱うため、この run が誤って外側 DB を run DB と認識しない。試験自身も子 celeris の起動時に外側の run 関連 env を明示的に除去する。
