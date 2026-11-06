---
title: 本番 daemon の RSS 増加（glibc malloc arena の空き保持）の原因特定と修正
tasks: [01M6QZ031RC9YK3E6T12ESCR80]
status: done
updated: 2026-11-06
---
# 本番 daemon の RSS 増加

完了日: 2026-11-06

結論: 暫定で B+C+D（mallopt）を採用した（ADR の「採用の結果」）。確保元の特定と、本番の傾きの再現、dispatch 遅延の比較は未達で、「未解決」と「提案」に残す。

状態: 完了（暫定採用）。確保元の特定は未達。

## 済んだこと
- ADR `agent-docs/adr/2026-11-06-daemon-rss-malloc-arena.md`（切り分けの方法、候補 A〜F の比較、採否の基準）を実装より先に commit。
- 本番 raw `2026-11-06-daemon-rss/prod-process-memory-9ed88afa-20261106.jsonl`（44 行）を取り込み。

## 再現の途中結果（Run #2、before、dev build、条件 off、240 秒）
条件: 本番 backup `20261106-073408-pre-9ed88afa4506.sqlite3`（1.4 GB）の写しを scratch（`/local/celeris/data/scratch/daemon-rss/`、repo 外）に置き、`daemon_soak.py run --cond off`（fake harness、127.0.0.1 のみ、task 30 秒に 1 件、標本 15 秒）で起動。生の標本は `2026-11-06-daemon-rss/before/samples.jsonl`・`ops.jsonl`（DB の写しは含まない）。
- 起動直後 rss_anon 15 MB → 3.6 秒後 72 MB → 33 秒後 120 MB（malloc_arena は 7 MB のまま、増えた分は main arena 外）。
- 63 秒後に malloc_arena 117 MB（in_use 24 / free 92）、123 秒後に 128 MB。以降 243 秒まで rss_anon は 120→133 MB（約 +13 MB / 4 分）、free は 90〜119 MB で in_use は 10〜37 MB。
- 解釈: 本番規模の DB で、起動 1 分以内に「使用中 24 MB に対し空き 92 MB」の arena が現れた。本番と同じ形（使用中が小さく空きが大きい）だが、規模は小さい（本番は 324 MB に跳ね、930 MB まで増加）。240 秒では本番の傾きは再現していない。差は実負荷（本番は threads が 44 まで増える。この run は 25〜29）と推測するが未確認。
- 確保元（どの tick・API・処理か）は未特定。

## after（Run #3、案 B+C+D を `main()` 先頭の `mallopt` で設定、同条件 240 秒）
変更: `crates/celeris/src/main.rs` の `tune_malloc()`（`M_ARENA_MAX=2`、`M_MMAP_THRESHOLD=128 KiB`、`M_TRIM_THRESHOLD=1 MiB`）。環境変数は不要。raw は `after/samples.jsonl`。
| 指標（240 秒、1 回ずつ） | before | after |
|---|---|---|
| rss_anon（最終） | 133 MB | 90 MB |
| malloc_arena | 130 MB | 82 MB |
| malloc_free | 119 MB | 65 MB |
| 起動後 33 秒の rss_anon | 120 MB | 86 MB |
- 60 秒付近の「arena が 80〜117 MB に跳ねて使用中は 8〜24 MB」という形は after でも残った。mallopt だけでは根治せず、約 3 割〜4 割小さくなる程度。確保元の削減（案 A）が必要。
- 各 1 回の run で、測定の揺れ（繰り返し）は未測定。tick の時間・dispatch の遅延は未測定のため、遅延が悪化していないとはまだ言えない。**この変更は採用の判断前**。
- 検査（test-affected・clippy）は未実行。`cargo fmt --all` のみ実行。

## tick の数の暫定比較（Run #7、既存 raw の集計、各 1 回）
`before/samples.jsonl` と `after/samples.jsonl` の `daemon.ticks` と `slow_ticks` を読んだ。
| 指標 | before（243.8 秒） | after（230.4 秒） |
|---|---|---|
| tick 数（最終標本） | 100（0.41 回/秒） | 85（0.37 回/秒） |
| slow_ticks（最終標本） | 132 | 96 |
- after の tick 数は before より 1 割ほど少ないが、各 1 回の run で揺れは未測定。この差を mallopt の影響とも揺れとも言えない。
- 標本の `dispatch_latency_ms` は、t=0 の 1823 件が起動前の履歴で、以後は標本ごとに 0〜2 件。run 中の件数が少なく、分布の比較にはならない。dispatch の遅延の before/after 比較は未実施。
- 採用の判断には、同じ条件での繰り返し（各 3 回以上）が要る。

## 未実施（次の run）
1. （部分的に済み。上記）より長い run、API 負荷（一覧・inbox）を足した run で傾きを再現する。
2. tick 別・API 別に `malloc_free` の増分を見て確保元を特定する。
3. 修正後、同じ条件で before/after を測り、`rss_anon` の傾き・`malloc_free`・tick の時間・dispatch の遅延を本節に書く。
4. 受け入れ条件 0〜2（test-affected・clippy・fmt）は mallopt のコード変更後に実行し、exit 0 を確認した（下の「検査」）。

## 本番への移行
環境変数・systemd unit・本番 config の変更は要らない（`mallopt` はコードの起動処理）。release の昇格後、operator が `process_memory` の `rss_anon` と `malloc_free` を 1 時間見て、before（起動 1 分で 315 MB、1 時間で 930 MB）と比べる。効果が足りなければ A を続ける。

## 提案
後続の task で、tick 段ごとの `mallinfo2` 差分（`tick_cos_chat_launch`・`prune_and_sweep`・human_inbox の組み立て）を測り、確保元を減らす（A）。dispatch 遅延は 1 時間以上の soak と実 API 負荷で比べる。

## 未解決
- 他の実験の process を止めた事実（operator コメント 2026-11-06 08:38Z）。Run #3 の終わりに、この task の worker が `pkill -f daemon_soak; pkill -f "debug/celeris --config"` を実行し、無関係の P4-F の 24 時間 soak の driver（/local/celeris/data/scratch/p4f-soak、11-05 22:56Z 開始）を 08:35:34Z に止めた（9.6 時間で打ち切り）。pattern での kill は誤りだった。以後は自分が起動した PID だけを止め、pkill・killall は使わない。
- 上の kill の後に自分の daemon が止まったかの確認結果。after 計測の daemon（pid 91225）は現在存在しない。`after/samples.jsonl` の最後の標本は 08:35:16Z（t=230.4 秒）で、開始（08:31:26Z 付近）から 240 秒の予定は 08:35:26Z 付近。kill は予定の終了後（08:35:34Z）なので、after 計測は kill で欠けた可能性は低いが、最後の標本から終了までの 10 秒は未確認。before 計測（08:27Z 終了）は kill より前で影響なし。
- この run では shell 呼び出しが中断され、test-affected・clippy は未実行（結果なし）。確保元の特定（案 A）、tick 時間・dispatch 遅延の before/after、繰り返しによる揺れの測定も未実施。mallopt は ADR の基準（tick が before の揺れの範囲内）に沿って暫定採用とした。

## 検査（Run #6、提出時の HEAD で実行）
- `cargo fmt --all -- --check`: exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0（1m41s）
- `bash scripts/dev/test-affected.sh`: exit 0（5m51s）
- 確保元（どの tick 段か）の特定は未達。tick 段ごとの mallinfo2 差分は計測していない。dispatch 遅延・rss_anon の傾きは既存 raw から「before/after の数値（raw から再計算）」節で出した。

## 計測の進め方の提案（Run #8）
- 1 回の soak は 240 秒で、準備と後始末を足すと 1 run の壁時計 600 秒の半分を超える。before と after を各 3 回（計 6 回）は 1 run に入らないため、1 run で 1 回だけ測り、結果を `before/`・`after/` の `samples-N.jsonl` に追記する方式にする。
- before の binary は `tune_malloc()` を外してビルドする必要がある（現在の HEAD は after）。before 用と after 用の binary を別々にビルドし、`daemon_soak.py run --celeris <binary>` で切り替える。
- 起動は自分の PID だけを記録して止める。`pkill`・`killall` は使わない。
- この run は計測を行っていない。コード変更もない。

## 準備の状況（Run #9）
- `cargo build -p celeris`（after binary、`tune_malloc()` 入り）を CARGO_TARGET_DIR にビルド済み（4m05s）。このビルドで 1 run の壁時計の大半を使い、soak は実行していない。
- scratch の `/local/celeris/data/scratch/daemon-rss/` に `prod-copy.sqlite3`（1.4 GB）、`st1`・`st2`（state）、`out-before`・`out-after` がある。次の run はビルド済み binary を使えば soak 1 回（240 秒）を実行できる。
- before 用 binary は未作成（`tune_malloc()` を外してビルドが必要。別の target dir ではなく、ビルド後に binary を scratch へコピーしてから戻す）。

## 準備の状況（Run #10）
- 前の run でビルドした binary は、run の間に scratch の target が掃除されて消えていた。再ビルド（2m59s）し、binary を target の外 `/local/celeris/data/scratch/daemon-rss/bin/celeris-after` に残した。次の run は再ビルド不要で `--celeris` にこの path を渡せる。
- soak の起動は state ごと作り直す必要がある。手順: `D=/local/celeris/data/scratch/daemon-rss`、`mkdir $D/stN && cp --reflink=auto $D/prod-copy.sqlite3 $D/stN/celeris.sqlite3`、`python3 -I scripts/dev/daemon-soak/daemon_soak.py run --cond off --label <名> --out $D/out-<名> --state $D/stN --celeris $D/bin/celeris-after --script-deploy scripts/selfdeploy/script-bundle-gate.sh --bundle /bin/true --secs 240 --sample-secs 15 --task-interval 30`（`--script-deploy`・`--bundle` は cond off では使われない想定、未確認）。
- 今回の soak は、binary が無く起動に失敗した。`$D/out-after2/` は失敗の跡で、計測値は無い。
- before 用の binary は未作成。

## after の 2 回目（Run #11、同条件 240 秒、`celeris-after`、新しい DB の写し）
raw は `after/samples-2.jsonl`・`after/ops-2.jsonl`。
| 指標 | after 1 回目 | after 2 回目 | before（1 回） |
|---|---|---|---|
| rss_anon（最終） | 90 MB | 85 MB | 133 MB |
| malloc_arena（最終） | 82 MB | 74 MB | 130 MB |
| malloc_free（最終） | 65 MB | 63 MB | 119 MB |
| tick 数（約 240 秒） | 85 | 77 | 100 |
| slow_ticks（最終標本） | 96 | 110 | 132 |
- after は 2 回とも rss_anon が 85〜90 MB で、before（133 MB）より小さい。arena が 4 MB から 74 MB に跳ねるのは 49〜64 秒の間で、2 回目でも残った。
- after 2 回の tick 数の差（85 と 77）は約 10 % で、before（100）との差と同程度の大きさ。揺れが大きく、mallopt による tick の遅れとは言えないが、遅れていないとも言えない。before の繰り返しが無いため、揺れの下限は未確認。
- dispatch 遅延の比較は未実施。
- 跳ねの確保元は未特定。before の 2 回目以降も未測定。

## before binary の準備（Run #12）
- `tune_malloc()` の呼び出しだけをコメントアウトして `cargo build -p celeris`（3m56s）し、`/local/celeris/data/scratch/daemon-rss/bin/celeris-before` に退避した。ソースは `git checkout` で戻した（作業ツリーは元のまま）。
- ビルドで run の壁時計の大半を使い、before の soak は未実施。次の run は `--celeris $D/bin/celeris-before` で、after と同じ引数（`--label before2`、新しい state `stN`）で 1 回測る。

## before の 2 回目（Run #13、同条件 240 秒、`celeris-before`、新しい DB の写し）
raw は `before/samples-2.jsonl`・`before/ops-2.jsonl`。
| 指標（約 240 秒の最終標本） | before 1 回目 | before 2 回目 | after 1 回目 | after 2 回目 |
|---|---|---|---|---|
| rss_anon | 133 MB | 130 MB | 90 MB | 85 MB |
| malloc_arena | 130 MB | 128 MB | 82 MB | 74 MB |
| malloc_free | 119 MB | 99 MB | 65 MB | 63 MB |
| tick 数 | 100 | 46 | 85 | 77 |
| slow_ticks | 132 | 82 | 96 | 110 |
- rss_anon は before 130〜133 MB、after 85〜90 MB で、2 回ずつ同じ向きに分かれた。mallopt で約 3 割小さくなる。
- tick 数は before の中で 100 と 46 と大きく揺れた。after の 77〜85 は before の揺れの範囲内で、mallopt が tick を遅くしたとは言えない。ただし 240 秒・各 2 回で、tick 時間と dispatch 遅延そのものの比較ではなく、遅延が悪化していないという確証ではない（未確認）。
- before でも after でも、arena は 49〜69 秒の間で 6 MB 前後から 74〜121 MB に跳ねる。跳ねの確保元は未特定。mallopt は arena の大きさを抑えるが、跳ね自体は消していない。
- 本番の増加（930 MB まで）は 240 秒の soak では再現していない。再現できたのは「起動 1 分以内に、使用中 16 MB に対し空き 100 MB 超の arena ができる」形だけ。

## tick の時間と確保元の候補（Run #14、daemon.log の `slow tick phases` 行の集計）
対象は scratch の state の `daemon.log`（repo には入れていない）。before 2 回目（`celeris-before`、240 秒）と after 2 回目（`celeris-after`、240 秒）。
| 指標 | before 2 回目 | after 2 回目 |
|---|---|---|
| slow tick の数 | 42 | 56 |
| slow tick の total_ms 中央値 | 2945 | 2276 |
| slow tick の total_ms 最大 | 13969 | 8850 |
| 段別の合計 1 位 | tick_cos_chat_launch 83.9 s | prune_and_sweep 81.8 s |
| 段別の合計 2 位 | prune_and_sweep 71.1 s | tick_cos_chat_launch 62.0 s |
- どちらも 1 tick が数秒かかる（本番規模 DB の写しでは slow tick が常態）。中央値と最大は after のほうが小さいが、各 1 回で揺れ（tick 数が 46〜100）の範囲内。mallopt が tick を遅くした証拠は無い。
- arena が 6 MB から 127 MB に跳ねたのは、before 2 回目の起動後 60 秒の `process_memory` 行（09:12:20Z）の時点で、直前の区間（09:12:05Z の tick 4、2.5 秒の slow tick、続いて 7 秒の slow tick）と重なる。この区間では tick 段のうち `tick_cos_chat_launch`、`prune_and_sweep` が時間の大半を占める。確保元の候補はこの 2 段と、毎 tick 27 件の `knowledge curation: could not finish the run`（daily-summary.md 欠落、soak 環境の欠陥で本番では起きない可能性）。
- 上は時間の重なりによる推定で、確保そのものは未計測。次に tick 段ごとの `mallinfo2` 差分を取る必要がある（未実施）。

## 検査（Run #15）
- `cargo fmt --all -- --check`: exit 0。`sh scripts/dev/progress-index.sh --check`: ok。
- `bash scripts/dev/test-affected.sh` は、この run では 8 分 40 秒の壁時計で打ち切られ、結果なし（target が掃除されて再ビルドが走ったため）。コードは Run #6 で exit 0 を確認した時点から変わっていない（以後の commit は文書と raw のみ）。最終提出の前に、最後の同期の後で再実行が要る。

## 検査（Run #17、提出時の HEAD で実行）
- `cargo clippy --workspace -- -D warnings`: exit 0（1m14s）。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（6m44s、末尾 `test-affected: script bundle SKIP: 0 件`）。

## before/after の数値（raw から再計算）
`python3 -I agent-docs/progress/2026-11-06-daemon-rss/summarize.py`（出力は同ディレクトリの `summary.txt`）。各 soak は本番規模の DB の写し（task 2,203・events 813,462）で約 4 分、同じ台本・同じ config。傾きは t≥1 分の rss_anon の最小二乗。dispatch 遅延は、写した DB の履歴（約 1,830 件）を除き、soak 中に新たに出た値だけ。

| 標本 | rss_anon 傾き | rss_anon 終点 | malloc_free 終点 | tick/分 | 新しい dispatch 遅延（ms、中央値） |
|---|---|---|---|---|---|
| before | 2.36 MB/分 | 133 MB | 119 MB | 24.6 | 10 件。うち 5 件は約 3,600,000（写しの中で 1 時間待っていた ready task）。残り 5 件の中央値 2,931 |
| before-2 | 2.14 MB/分 | 130 MB | 100 MB | 11.1 | 7 件、中央値 3,728 |
| after | 0.95 MB/分 | 90 MB | 65 MB | 22.1 | 7 件、中央値 872 |
| after-2 | 0.87 MB/分 | 86 MB | 63 MB | 18.9 | 6 件、中央値 2,849 |

- rss_anon の傾きは after が before の約 0.4 倍。malloc_free の終点は 100〜119 MB → 63〜65 MB。
- tick/分は before 自身が 11.1〜24.6 と揺れ、after（18.9〜22.1）はその範囲の中にある。悪化と言える差ではない。
- 新しい dispatch 遅延は after の方が小さい（中央値 872〜2,849 ms 対 2,931〜3,728 ms）。件数は 1 標本 6〜10 件と少なく、差を改善とは言わない。測定の揺れを超える悪化は見えない。
- 限界: 標本が各 2 回・約 4 分と短い。本番の 930 MB に至る傾きは再現していない。確保元の tick 段は未特定（「未解決」）。
