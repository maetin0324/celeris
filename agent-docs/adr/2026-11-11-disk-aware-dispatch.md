# ADR 2026-11-11: 空きの見込みで dispatch を絞り、空きの下限を filesystem の大きさに比例させる

---
tasks: [01M75CV7TY7GMYY8PG3BE0SCBJ]
---

- 日付: 2026-11-11
- 状態: 実装済み（2026-11-11）
- 関連: [ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md)（`min_free_disk_mb` の全体停止）、[ADR-0075](0075-tiered-build-cache.md)（scratch pool・GC・実効上限）、[ADR 2026-10-10-local-disk-growth-paths](2026-10-10-local-disk-growth-paths.md)（release-build の上限）、[ADR 2026-10-07-build-tmp-hygiene](2026-10-07-build-tmp-hygiene.md) 付記 A4（disk_watch の critical）
- 進捗: [agent-docs/progress/2026-11-11-disk-aware-dispatch.md](../progress/2026-11-11-disk-aware-dispatch.md)

## 背景

2026-11-11 に `/local`（300 GB、btrfs compress=zstd:1）が 12:09Z の 80.4% から 12:59Z の 95.2% まで埋まった。
同時に Rust の task が 6〜7 本走り、それぞれの scratch の target（1 つ 40〜106 GB）と release-build の target
（88.0 GB）が並んだ。原因は 3 つある。

1. dispatch は今の空きだけを見ていた（`check_disk_space`。下限 `min_free_disk_mb` の既定 5120 MiB）。新しい run が
   これから書く量（seed から写した後の作り直し）と、走っている run の target がこれから増える量を見ない。空きが
   5 GiB を割るまで新しい run が始まり、走っている target は P0 なので GC は消せない。
2. 下限が固定の 5 GiB で、filesystem の大きさに比例しない。受信箱の critical（disk_watch の 95%）の後で止まる。
3. release-build の target の上限（`SD_RELEASE_TARGET_MAX_BYTES`、既定 64 GiB）は release.sh の build の**前**に
   だけ確かめていた（`sd_release_prune_enforce_limit`）。build で上限を超えて育った target（本番 88.0 GB）は、
   次の release の開始まで lease（TTL 48 時間、P0）のまま残る。adopt の経路は原因ではない（adopt の後も開始時の
   確認は通る）。

operator の計測（2026-11-11 19:3xZ）: task-core を変える task の target は seed から写して 20 分で 65.3 GB を
書き直し、seed のまま残ったのは 1.4 GB だった。低い crate を変える task では seed との共有をほぼ 0 と見るのが
実測に合う。

## 決定

### D1. dispatch の前に空きの見込みで判定する（`task_dispatch::disk_admission`）

run を起こす前（write-set の判定の後、lease の前）に、次の不等式が成り立たなければ run を始めず、task を
`ready` のまま待たせる（attempts を消費しない）。

```
空き − Σ 走っている target の増分の見込み − 今 tick に通した run の見込み − この run の見込み ≥ 予備
```

- **空き**: scratch pool の filesystem の statvfs の空き（`ScratchStatus.fs_free_bytes`）。
- **予備**: D2 の空きの下限（pool の filesystem の大きさ × `min_free_disk_pct`）。
- **repo の見込み**: 直近の scratch の走査で、target が pool にある同じ `repo_key` の owner（seed に選ばれた P3 の owner も
  含む）の測った大きさ（lease の `size_bytes`）の最大。GC の推定（ADR-0075 D2 の「未測定は同じ repo の最大」）と同じ定義・
  同じ入力で、関数も 1 つ（`task_worker::scratch::repo_max_sizes`）。target が消えて lease の記録だけが残る owner は数えない。
  測った target が無い repo の見込みは 0（判定しない。GC は全体の最大で補うが、dispatch は判定しない）。
- **走っている target の増分**: run か review の検査が今 target に書いている owner（分類の理由が `task running`・
  `task reviewing`・`work unit running`。`task_worker::scratch::target_written_now`。review の検査も task の owner の
  `CARGO_TARGET_DIR` で build する）と、lease の生きた release-build（`lease live`）ごとに
  `max(0, repo の見込み − 今の大きさ)`。未測定は大きさ 0 と見る。target がまだ pool に無い（seed を `.seed-copy-*` に
  写している最中。写し終えてから `target/` に rename する）run も、大きさ不明の増える owner として数える。class `p0` でも
  agent の lease と、まだ始まらない WU（`work unit queued`。その run の見込みはこの判定で数える）は数えない。
  release-build は D3 で build の後に上限を超えた target を作り直すので、次の build が見込みまで育てる量を
  常に予約として数える。
- **この run の見込み**: run が使う owner（並列 WU は `task-<id>/wu-<id>`、それ以外は `task-<id>`）の target が
  pool に既にあれば `max(0, repo の見込み − 今の大きさ)`（同じ task の続きは増分だけ）、無ければ repo の見込み全体
  （seed から写しても共有はほぼ 0 と見る）。
- **単位**: 測った大きさは見かけの大きさ（btrfs の圧縮前）で、statvfs の空きは disk 上の量。見込みには
  `[scratch] forecast_disk_ratio`（既定 1.0、範囲 (0, 1]）を掛けて disk 上の量に直す。既定 1.0 は圧縮の無い
  filesystem で正しく、圧縮のある filesystem では多めに見積もる（安全側）。本番の値は人が実測から決める
  （進捗の「本番への移行」）。
- **詰まらない**: worker run が 1 本も走っていなければ、見込みが足りなくても 1 本は通す（今 tick に通した run が
  あれば 2 本目からは判定する）。
- **対象**: scratch が有効で、run の cargo の target を結び付ける repo（`cargo_target_repo`）があり、この run の
  見込みが 0 より大きいもの。Remote の run と見込み 0 の run（Rust でない repo、続きで増分が無い run）は判定しない。
- **呼び出し**: `Gate::holds_run`（`dispatch_run.rs` から `&self` で呼ぶ。状態は `Gate` の中の Mutex）。Dispatcher の field・method は増やさない（health の `size.responsibility-growth`）。試験は `Gate.probe` で空きと大きさを注入する。
- **記録**: 待たせた run（task と WU）ごとに、待ち始めに 1 回だけ `tracing::info!`（`disk forecast: run held`）を
  残す。受信箱・通知には出さない。run が始まる・task が `ready` でなくなると記録を消す。
- 旧い挙動（今の空きだけで決める dispatch）は残さない。`check_disk_space` の全体停止（D2 の下限を割ったら全部止める）
  はこの判定の前段としてそのまま働く。

### D2. 空きの下限を filesystem の大きさに比例させる（`[dispatch] min_free_disk_pct`）

`[dispatch] min_free_disk_mb`（固定 MiB）を `[dispatch] min_free_disk_pct`（filesystem の容量に対する割合、既定 10、
0 で無効、範囲 [0, 100)）に置き換える。各 filesystem の下限 = 容量 × pct / 100。既定 10 は使用率 90% で新しい run を
止め、disk_watch の warn（80%）と critical（95%）の間に入る。`check_disk_space`・scratch の実効上限（ADR-0075 D1）・
seed の更新の保留・`celerisctl scratch` が同じ下限を使う（`scratch_gc::min_free_bytes`）。

旧い欄 `min_free_disk_mb` は読み込みエラー（置き換え先を示す）にする。

### D3. release-build の target の上限を build の後にも確かめる

release.sh は終了時（成功・失敗とも。EXIT trap）に、この build の開始時刻より古い workspace の成果物を刈り
（`sd_release_prune_stale_test_binaries`、開始時と同じ規則）、続けて上限を確かめる（`sd_release_prune_enforce_limit`。
超えていれば target を作り直す）。この処理を `sd_release_target_settle` にまとめる。上限を超えた target が lease の
まま次の release まで残らない。

### D4. scratch の status に見込みと待たせている run を出す

`ScratchStatus.admission`（`GET /api/v1/metrics/scratch` と `celerisctl scratch status`）に、空き・予備・係数・
走っている target の増分の合計・repo ごとの見込み・owner ごとの見込み（大きさ・見込み・増分）・待たせている run
（task・WU・owner・repo・見込み・理由・待ち始め）を出す。待たせている run の正本は daemon の判定のメモリ
（`disk_admission::Gate`）だけで、file には書かない。daemon は snapshot（`GET /api/v1/metrics/scratch`）を組むときに
メモリから `held` を埋める。`celerisctl scratch status` は pool を走査して組むので、見込みは出すが `held` は
`null`（分からない）とし、表では「daemon だけが知る」と API を示す（`last_gc` と同じ扱い）。

付記（2026-11-11、review の指摘）: 初版は pool の `admission.json` に書き出し、daemon の status もそれを読んでいた。
verify の daemon（`celeris --mode verify`）は本番と同じ config・同じ scratch pool で dispatch の pass を回すので、
最初の pass で空の一覧を書き、本番の status の `held` が実際の待ちと食い違った。共有の写しを廃止し、メモリだけを
正本にした。待たせている run の記録は、候補を照合する pass の始めに、前の pass で判定に来なかった run の分を消す
（枠が埋まって照合しない pass では消さない）。

付記（2026-11-12、attempt 2 の review の申し送り）: 初版の見込みは (1) target の消えた lease の記録（最長 7 日残る）も
repo の最大に数え、本番では今ある target の最大 68.1 GB に対し 153.2 GB になっていた。(2) class `p0` の owner をすべて
「走っている」と数え、lease の生きた release-build（48 時間）だけで増分 90.2 GB になり、run が 1 本でも走っていれば
新しい run がほぼ待つ見込みだった。repo の見込みを GC と同じ定義（target が pool にある owner の最大）にし、増分を
数える owner を `target_written_now` に絞った。WU の分類の理由は `work unit in progress` を `work unit running`
（Running）と `work unit queued`（Pending・Ready・NeedsContinuation）に分けた（class はどちらも P0 のまま）。

付記（2026-11-12、attempt 4 の review の指摘）: (1) 走っている run の割り当ては seed の写しの数分のあいだ target が
pool に無く、その owner の増分が 0 になって次の tick の run が通った。target が無くても `target_written_now` の owner は
大きさ不明の増える owner として数える（`samples_from_owners`）。(2) release-build の target は build 1 回で約 88 GB
になり上限 64 GiB を毎回超える。D3 は終わりに作り直すので空きは戻るが、次の build がまた書く。lease の生きた
release-build の増分を数える（上の「走っている target の増分」。attempt 2 の付記で一度外したのは、target が上限を
超えて残っていた間の古い大きさに引きずられた数え方を直すためで、今は D3 で target が小さく保たれる）。

## 結果

- 同時に走る Rust の task の数は空きの見込みで決まる。見込みは repo の最近の最大なので、大きな target を作る task が
  並ぶと 2 本目以降が待つ。
- 既定（係数 1.0）は圧縮のある `/local` では多めに見積もり、並列度が下がる。人が実測から係数を下げられる。
- release-build の target は release の後も上限以下に収まる。上限を超えた回は次の release が作り直しのぶん長くなる。
