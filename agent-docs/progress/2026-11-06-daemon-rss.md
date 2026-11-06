---
title: 本番 daemon の RSS 増加（glibc malloc arena の空き保持）の原因特定と修正
tasks: [01M6QZ031RC9YK3E6T12ESCR80]
status: running
updated: 2026-11-06
---
# 本番 daemon の RSS 増加

状態: 途中。ADR と本番 raw の取り込みまで完了。再現・修正・before/after は未実施（run の予算 600 秒・10 turn を使い切るため）。

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
4. 受け入れ条件 0〜2（test-affected・clippy・fmt）はコード変更後に実行する。今回はコード変更なし。

## 本番への移行
未定（コードの `mallopt` で足りれば不要。環境変数が必要なら再現後にここへ書く）。

## 提案
ADR の方針のとおり、まず大きな一時確保の削減（A）を狙う。

## 未解決
- 他の実験の process を止めた事実（operator コメント 2026-11-06 08:38Z）。Run #3 の終わりに、この task の worker が `pkill -f daemon_soak; pkill -f "debug/celeris --config"` を実行し、無関係の P4-F の 24 時間 soak の driver（/local/celeris/data/scratch/p4f-soak、11-05 22:56Z 開始）を 08:35:34Z に止めた（9.6 時間で打ち切り）。pattern での kill は誤りだった。以後は自分が起動した PID だけを止め、pkill・killall は使わない。
- 上の kill の後に自分の daemon が止まったかの確認結果。after 計測の daemon（pid 91225）は現在存在しない。`after/samples.jsonl` の最後の標本は 08:35:16Z（t=230.4 秒）で、開始（08:31:26Z 付近）から 240 秒の予定は 08:35:26Z 付近。kill は予定の終了後（08:35:34Z）なので、after 計測は kill で欠けた可能性は低いが、最後の標本から終了までの 10 秒は未確認。before 計測（08:27Z 終了）は kill より前で影響なし。
- この run では shell 呼び出しが中断され、test-affected・clippy は未実行（結果なし）。確保元の特定（案 A）、tick 時間・dispatch 遅延の before/after、繰り返しによる揺れの測定も未実施。このため mallopt の採用は未判断。

## 検査（Run #6、提出時の HEAD で実行）
- `cargo fmt --all -- --check`: exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0（1m41s）
- `bash scripts/dev/test-affected.sh`: exit 0（5m51s）
- 確保元の特定、tick 時間・dispatch 遅延の before/after、揺れの測定は引き続き未実施。mallopt の採用は未判断。

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
