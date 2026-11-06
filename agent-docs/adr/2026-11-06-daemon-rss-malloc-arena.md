# daemon の RSS 増加: glibc malloc の arena が空きを抱え続ける件の切り分けと直し方

- 日付: 2026-11-06
- 状態: 採用（暫定: B+C+D を `mallopt` で採る。A は確保元が未特定のため未着手で、後続の仕事として残す。結果は「採用の結果」節）
- 関連: P4-F（`2026-11-05-phase4-p4f-daemon-soak`）、`crates/celeris/src/daemon/process_memory.rs`

## 背景

本番 release 9ed88afa4506（2026-11-06 07:34Z 起動）の `process_memory` 44 行（1 分ごと）で、`rss_anon` は 9 MB → 315 MB（1 分後）→ 930 MB（08:17）と増えた。
glibc の `malloc_in_use` は 21〜92 MB でほぼ横ばいで、`malloc_free`（arena 内の空き）が 279 MB → 922 MB に増え、`malloc_arena` ≈ `rss_anon` だった。
V8 heap は 3.4 MB、sqlite の pagecache は 4〜28 MB で主因ではない。threads は 28 → 44。
つまり漏れ（使用中の増加）ではなく、大きな一時確保の後に arena が OS へ返されない断片化・保持と読める。
非本番の 26 分 soak は +17〜19 MB で再現しなかった。差は DB の大きさ（task 約 2,200、events 多数）と実負荷と考える。

## 切り分けの方法

1. 本番 DB の backup の写しを scratch に置く（repo・KB・progress には入れない）。fake harness のみ、cluster・browser・通知・remote・本物の LLM・外部接続を全部切った config で daemon を起動する。本番 DB と本番 state dir には書かない。
2. `scripts/dev/daemon-soak/` で 1 分ごとの `process_memory` を取り、起動直後の跳ね（本番は 1 分で 324 MB）が再現するか見る。
3. 確保元の特定は、tick の段・API 別に `malloc_free` の増分を見る。具体的には (a) 各 tick の前後で `mallinfo2` の差を debug log に出す試験用の計測、(b) API を止めて tick だけ、tick を止めて API だけの 2 条件の比較、(c) 疑いの処理（CoS 一次対応の human_inbox の毎 tick の組み立て、一覧の全件読み込み）の単体での確保量。
4. 計測の指標: `rss_anon` の傾き（MB/分）、`malloc_free`、tick の時間（p50/p99）、dispatch の遅延。

## 直し方の候補の比較

| 候補 | 効果の見込み | 性能の懸念 | 備考 |
|---|---|---|---|
| A. 大きな一時確保の削減（全件読み込みの絞り込み・差分化） | 根本。確保元が分かれば最も確実 | なし（むしろ tick が速くなる） | 確保元の特定が前提。本命 |
| B. `mallopt(M_ARENA_MAX, 2)` | thread ごとの arena の乱立（threads 44）による保持を抑える | arena の lock 競合が増える可能性 | 起動時に `mallopt` を呼ぶので環境変数は不要 |
| C. `M_MMAP_THRESHOLD` を下げ（例 128 KiB 固定）大きな確保を mmap にする | 大きな一時が解放時に OS へ返る | mmap/munmap の呼び出しが増える | 動的 threshold を固定すると glibc の自動調整が止まる |
| D. `M_TRIM_THRESHOLD` を下げる | top の空きを返しやすい | 中間の空きには効かない | 断片化には限定的 |
| E. 重い tick の後の `malloc_trim(0)` | 全 arena の空きを返す | 呼び出し中に arena lock を取り tick が止まる。コストを測る必要 | 周期（例 60 秒に 1 回）で呼べば上限が付く |
| F. allocator の置き換え（jemalloc / mimalloc） | 断片化に強い | 依存の追加、sandbox・静的 link の影響、観測（`mallinfo2`）の作り直し | 規模が大きい。A〜E で足りなければ検討 |

## 決定（測定前の方針と採否の基準）

- まず A を確保元の特定で狙う。A で `malloc_free` の増加が止まれば B〜E は入れない（仕組みを増やさない）。
- A だけで足りない場合は、B と C を `mallopt` で起動時に設定する案と、E を定期呼び出しにする案を、同じ条件の before/after で比べて 1 つ以上を採る。F は採らない（A〜E で足りない場合のみ別 ADR）。
- 採否の基準: 同じ DB・同じ config・同じ時間で `rss_anon` の傾きが下がり、tick の p99 と dispatch の遅延が測定の揺れ（before の複数回の run の幅）を超えて悪化しないこと。悪化した案は採らない。
- 環境変数（`MALLOC_ARENA_MAX` 等）は本番の systemd unit の変更になるため worker は設定しない。コード側の `mallopt` を優先し、環境変数が必要なら進捗の「本番への移行」節に手順を書く。

## 採用の結果（2026-11-06 追記）

採ったのは B+C+D（`M_ARENA_MAX=2`、`M_MMAP_THRESHOLD=128 KiB`、`M_TRIM_THRESHOLD=1 MiB`）を `main()` 先頭の `mallopt` で設定する案。環境変数は使わない。
A（確保元の削減）を先に狙う方針だったが、本番規模の DB の写しでの 240 秒 soak では確保元を特定できなかった（tick 段の `mallinfo2` 差分は未計測。時間の重なりでは `tick_cos_chat_launch` と `prune_and_sweep` が候補）。確保元の特定を待つと本番の RSS 増加（930 MB まで）が続くため、基準の「A だけで足りない場合」に進み、B+C+D を暫定で採った。E と F は採っていない。
根拠（同じ DB の写し・同じ config・240 秒、各 2 回）: rss_anon は before 130〜133 MB、after 85〜90 MB。malloc_free は before 99〜119 MB、after 63〜65 MB。tick 数は before 46〜100、after 77〜85 で、after は before の揺れの範囲内に収まった。slow tick の total_ms 中央値は before 2945 ms、after 2276 ms。したがって採否の基準（tick が揺れを超えて悪化しない）に反する証拠は無い。ただし 240 秒・各 2 回で、dispatch 遅延の分布の比較は未実施（標本あたり 0〜2 件で比較にならない）。
限界: 起動 1 分以内の arena の跳ねは after でも残り、mallopt は大きさを約 3 割抑えるだけで根治ではない。本番の傾きは 240 秒では再現していない。本番で効果が足りなければ A（確保元の特定）を続ける。数値の raw は `agent-docs/progress/2026-11-06-daemon-rss/`。

## 確保元の特定の結果（2026-11-06 追記）
tick 段別の `mallinfo2` 差分（`CELERIS_TICK_MALLOC_TRACE=1` で有効、既定は無効）と、API を 1 本ずつ送る soak で測った（進捗 `2026-11-06-daemon-rss.md` の「確保元の特定」）。
- 基準（tick のみ）で arena は 1 分後 100 MB。広げた段は `tick_cos_chat_launch`（+34 MB、最大 +32 MB）。
- 読み取りの GET を 1 秒に 1 本足すと、`GET /inbox` と `GET /projects` が arena を約 200 MB（基準の約 2 倍、使用中は 20〜36 MB）まで広げ、`GET /tasks` は 128 MB。
- A（確保元の削減）は `task_ops::inbox::inbox` と projects の集計の全走査の見直しで、複数 crate にまたがるため本 ADR では入れない。決定は変わらず、B+C+D を暫定のまま採り、A は別 task（進捗の「提案」）に回す。計測用の環境変数は性能の理由で既定を無効にした。
