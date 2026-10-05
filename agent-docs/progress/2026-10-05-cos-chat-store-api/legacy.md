# 旧 CoS Console 会話の legacy スレッド移行

task-core の通常の書き込み可能な store open で schema migration の直後に移行する。旧 `messages` の `node_id='cos'` を案件ごとと NULL の全体スコープごとに 1 本の legacy thread へ写し、`(created_at,id)` 順に seq を付ける。role、本文、run id、task id と旧 metadata の出典を保存する。旧行は削除せず、旧 CoS conversation session は retire する。

`legacy_message_id` の UNIQUE と未移行行の照合で再実行時の二重取り込みを防ぐ。`feed_cursor` には走査した旧 rowid の最大値を進捗として残す。SQLite の VACUUM が暗黙の rowid を振り直しても行を見落とさないよう、正しい移行済み判定には watermark の大小を使わない。新しい chat message の trigger が FTS に本文を反映する。

確認: `cargo test -p task-core chat_ --lib`（23 件成功）、`cargo clippy -p task-core --all-targets -- -D warnings`（成功）。旧 CoS の稼働中 run の drain と互換 Console の二重表示回避は cos-run へ申し送る。
