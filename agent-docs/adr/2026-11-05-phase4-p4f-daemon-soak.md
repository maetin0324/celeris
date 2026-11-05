---
tasks: [01M6PAMC40X023V30F4T9QH2GW]
---
# P4-F 実 daemon 全体の長時間の資源の計測と、process の memory の内訳の記録

P1-I（`2026-11-05-phase3-p1i.md`）は `build_dispatcher` の dispatcher だけを測り、本番の RSS の増加（2026-11-04、182→589 MB／1,529 秒）を V8・compare では再現しなかった。残った候補（daemon 全体の背景の仕事・SQLite の page cache・malloc の arena）を同じ 4 条件で切り分けるため、本物の daemon binary を測る道具と、daemon の中からしか読めない値の記録を足す。

## 決定
- D1 `process_memory` の log 行: daemon は起動直後と 60 秒ごとに 1 行（info、target `celeris::daemon::process_memory`）を出す。値は `/proc/self/status` の `VmRSS`・`RssAnon`・`RssFile`・`Threads`、`sqlite3_status64` の page cache の bytes（`PAGECACHE_OVERFLOW`）と `MEMORY_USED`・最大・malloc の回数、glibc の `mallinfo2`（arena・mmap・使用中・空き）、`[script]` があれば生きた generation ごとの V8 heap の合計（`GenerationRegistry::heap_stats`）。DB には書かない。tick と別の task で読み、tick を待たせない。本番では journal に 1 日 1,440 行増える。API の欄（`GET /daemon`）には足さない（schema と web の型を変えずに段 4 の本番の読み取りに使える）。
- D2 実測で分かった制約: bundled の SQLite は memstatus が無効で、`MEMORY_USED`・malloc の回数は 0 になる。page cache の bytes は数える。この 2 欄は残す（memstatus を有効にする変更は性能に効くので、この決定では行わない）。
- D3 driver: `scripts/dev/daemon-soak/daemon_soak.py`（`run`・`summary`・`plan`）と `scripts/dev/daemon-soak.sh`（4 条件を別の一時 home・別 port で同時に走らせる）。fake harness `soak_harness.py`（LLM 0。実行の run は `--sleep` 秒だけ run を持つ）。条件は P1-I と同じ名前で、`off` は `[script]` なし、`v8_load` は generation を登録して mode は rust、`shadow` は `lane_rules`・`cos_digest` を compare（本番の 2026-11-04 と同じ領域）、`enabled` は `lane_rules` を ts・`cos_digest` を compare。案件の controller は `shadow`・`enabled` で shadow。CoS chat（案件 thread に 5 分に 1 通）、CoS の一次対応、cron、API、worker・reviewer の run は本物の daemon の経路を通る。mode の行は一時 DB に直接置く（synthetic、本番の昇格の計数を経ない）。
- D4 標本（schema `celeris.daemon-soak/1`）: `/proc/<pid>` の status・maps（非 main の arena の数の推定: 64 MiB 境界の名前なし `rw-p` で続く `---p` と合わせて 64 MiB）・fd の数、最後の `process_memory` 行、`GET /daemon` の ticks・in_flight・`script_tick_wait`、読み取り専用の接続で数えた DB の行（task・状態別・run・running の run・task と WU のリース・events・wake・未消費の wake・判断・比較・chat の run と message・受信箱・cron の run）、event の取り込みの遅れ（`feed_cursor` の 4 個と events の最大 rowid の差）、dispatch の遅延（task の `created` から最初の `routing_decided` まで）、log の slow tick の数、DB の大きさ、disk の空き。
- D5 集計: 計測の窓は最初の process の標本だけ（restart の後は別 process）。傾きは P1-I と同じ 2 点の式・同じ経過秒（0・600・1,529 秒）。VmRSS に加えて RssAnon の傾きを出し、合否の目安は RssAnon で off と比べる（debug の binary の file の page は host の memory の具合で増減する。D7 の実測）。残差は「RssAnon の増分 −（malloc の使用中＋空き＋mmap＋V8 の total heap の増分）」で、負なら malloc の増分で説明が付く。
- D6 門と自動停止: `--secs` が 3,600 秒を超える run は人の承認の file（`approved_by = human:<名前>`・`scope = non-production`・`max_secs`・`conditions`・`expires_at`）が要る。Celeris の worker の run の中（`CELERIS_RUN_ID`）では承認があっても 1 時間を超える run を断る（exit 2）。`--state`・`--out` が本番の path の下なら断る。自動停止は壁時計・RSS 4,096 MB・disk の空き 5 GB・DB 4 GB（24 時間以上では `plan` の `recommended_args` で上げる）・daemon の終了・API の 3 回連続の失敗（既定値。引数で変える）。止めた理由は `ops.jsonl` と summary に残る。
- D7 合否の目安（段 3 の 24/72 時間で使う。短時間では記録）: RssAnon の 600 秒以後の傾きが off との差で +10 MB/時以内、V8 の used heap が開始の 2 倍未満、generation は 2 以下、run を持ったまま SIGKILL した restart の後に running の run とリースが 0、自動停止で止まっていない。
- D8 before/after: `daemon-soak.sh --celeris <bin> --label <名前>` で別の binary を同じ負荷で測り、`summary` に複数の dir を渡して並べる。P4-C・P4-D の後の性能の回帰はこの形で測る（DB の表が無ければ数えないので、版の違う binary でも読める）。

## 境界
- 本番の DB・config・systemd・releases・daemon に触れない。実 LLM を呼ばない。24/72 時間の実稼働、本番の読み取り（段 4）、本番の off の再起動は人の承認の後に人が実行する（P1-I の段 3・4）。
- 旧 milestones API（410）・project controller の権限・human_required・許可の保護には触れない。TS の bundle・policy は変えない。
