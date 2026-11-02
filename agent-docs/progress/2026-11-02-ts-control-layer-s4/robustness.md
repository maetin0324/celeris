---
title: S4 robustness — digest pass の順序・再配送・retry・cancel・再起動・版変化の試験
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 robustness — digest pass の順序・再配送・retry・cancel・再起動・版変化の試験

完了日: 2026-11-02（unit robustness）。本番コードの挙動は変えていない（足したのは試験と、試験専用の `#[cfg(test)]` hook 1 つ）。

## 結論
- `crates/task-dispatch/src/dispatcher/tests/cos_digest_script.rs` に試験 11 件を足した（試験名はすべて `cos_digest_script_` で始まる）。偽の `PolicyScript`、注入した試験の時計、`tokio::time::pause`（`start_paused`）、worker と compare task の join だけで待つ。sleep と CPU 負荷は使わない。
- どの試験でも同じ不変条件を確かめる（`assert_invariants`）。
  - digest message は冪等 key ごとに 1 件
  - requeue は item ごとに 1 回まで。試験 DB に置いた trigger が `running`→`pending` の遷移を `test_requeue_log` に書き、その行で数える。
  - `script_compare_log` の記録は run ごとに 1 件まで
- Rust の結果が TS の有無で変わらないことは、同じ場面を 2 通りで走らせて確かめる（`same_with_and_without_ts`）。1 つは mode `rust` で PolicyScript 無し、もう 1 つは mode `compare` で echo の script。比べるのは message（key・text・cards・run_id）、item（source_key・state・run_id・reason）、requeue の log で、byte 一致を見る。
- 障害は試験 DB 上で起こし、外した後の tick で回収されるかを見る。
  - 書き込みの失敗: SQLite の trigger（`RAISE(ABORT)`）
  - 読み出しの失敗: `cos_inbox_items` を一時的に改名する
- 試験の補助として、`cos_chat_triage.rs` の fixture から `triage_dispatcher` を切り出した。同じ DB で dispatcher を作り直すために使う。`script_compare.rs` には `#[cfg(test)] abort_script_compares` を足した。in-flight の compare を打ち切り、dispatcher が落ちた状態を作る。

## 試験と場面（目的の番号）
1. 順序 `cos_digest_script_order_item_update_before_and_after_run_end`
   - item の判断 → run の終了 → digest の順では、digest に判断が載る。
   - run の終了 → digest → 遅れた判断の順では、2 回目の digest は出ない。completed の run の未処理 item は、同じ pass の退避（fallback）に既に渡っており、遅れた判断は効かない（`cos_triage_resolve` は false）。
   - stopped の run だけが 1 回 requeue される。
2. 再配送 `cos_digest_script_redelivery_same_run_twice`
   - 新しい tick を 2 回挟み、digest_due を同じ tick の中で再設定して 2 回処理する。
   - message・item・requeue は 1 回目と同じ。TS の呼び出しは run ごとに 1 回。
3. retry
   - `cos_digest_script_retry_after_failed_message_write`: 次の tick で message が 1 件書かれる。記録も 1 件で、match。
   - `cos_digest_script_retry_after_failed_read`: 走査が失敗する。次の tick で digest 1 件、requeue 1 回、記録 1 件。
   - `cos_digest_script_retry_after_failed_requeue_write`: message の後の requeue が失敗する。下の「見つけた事」を参照。
4. cancel
   - `cos_digest_script_ts_cancelled_at_deadline`: TS が返らない。deadline で `ts_error`（timeout）が記録される。stopped と interrupted の run は 1 回ずつ requeue される。
   - `cos_digest_script_dispatcher_dropped_mid_compare`: compare の途中で task を abort し、dispatcher を作り直す。digest 済みの run は再び比べない（呼び出し 0 回・記録 0 件）。
5. 再起動
   - `cos_digest_script_restart_same_db`: digest の前に落ちた run は、再起動後の reconcile で 1 回だけ digest される。digest 済みの run は、再起動を 2 回しても増えない。
   - `cos_digest_script_restart_after_failed_requeue`: requeue が失敗したまま再起動する。message は 1 件、requeue は 0 回、item は退避のまま。
6. 版と freshness `cos_digest_script_policy_version_change_and_fresh_session`
   - FakeAdapter の本物の CoS run（worker の join で待つ）で確かめる。`[cos.triage] policy_version` を 7 に上げて reload すると、次の triage run は session reason `policy_changed` で fresh に始まり、記録される版も 7 になる。
   - 2 つの run の digest は各 1 件で、compare の記録も各 1 件（match）。session の理由は TS の有無で同じ。
7. mode と generation `cos_digest_script_mode_back_to_rust_and_generation_change_mid_compare`
   - compare が in-flight の間に mode を rust に戻す。in-flight の記録は元の generation で 1 件だけ残る。
   - rust の間の run は TS に渡らない。
   - 新しい generation で compare に戻すと、次の run はその generation で 1 件記録される。再配送しても増えない。

## 見つけた事（挙動は変えていない）
- requeue の書き込みが message の後で失敗すると、requeue は再試行されない。run は digest 済みになり、`cos_triage_runs_without_digest` に載らないため。ただし item は `running` のまま残らない。同じ pass の `triage_fallback` が、stopped・interrupted の run の item を「CoS run が途中で止まった」（CoS 無効なら「CoS が無効」）として人へ渡す。このため二重の requeue も、取り残された item も出ない。失うのは「次の CoS run で扱い直す」機会だけである。この apply では compare の記録も作らない（hook の規則どおり、記録は apply が全部成功した run だけ）。
- message の書き込みが失敗した tick も同じで、requeue より先に fallback が item を人へ渡す。次の tick の digest は退避の結果を載せる。
- `digest_snapshot` は `chat_run_get` の失敗を `ok()` で捨て、reason 無しで digest を書く。一度書いた message は冪等 key で固定されるため、一時的な読み出しの失敗で reason が恒久的に抜ける。不具合として task-dispatch の中で直すかは close unit の判断に回す。直すなら NotFound 以外の失敗で次の tick に回す挙動変更になり、言語移行とは別の差分にすべきため、この unit では直していない。

## 受け入れ条件と証拠（提出時の HEAD で実行）
- 条件 0（(1)〜(7) の試験があり通る）: `cargo nextest run -p task-dispatch -E 'test(/cos_digest_script/)'` → 11 passed。続けて 3 回走らせ、3 回とも 11 passed。
- 条件 1（二重の message・requeue・compare 記録が 0）: 11 件すべてで `assert_invariants` が通る。
- `bash scripts/dev/test-affected.sh` → exit 0（対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 3,014 passed・9 skipped）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## commit（件名）
- phase s4: robustness: digest pass の順序・再配送・retry・cancel・再起動・版変化の試験を足す

## 未解決事項
- 上の「見つけた事」の 2 点（requeue の部分失敗、run の読み出し失敗で reason が抜ける）。

## 提案
- task-core の store に、digest の message 書き込みと requeue を 1 つの transaction で行う API を足す（範囲外のため直していない）。部分失敗が無くなり、requeue の機会を失わない。compare の記録も必ず作られる。
- `digest_snapshot` の run の読み出しで、NotFound 以外の失敗は Err にして次の tick に回す（挙動変更として別の差分にする）。
