---
title: P1-I 長時間運用と観測
tasks: [01M6N7XYA1GC9WVPFDSQE39D6V]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P1-I 長時間運用と観測

完了日: 2026-11-05（仮想時刻 72 時間・非本番の実時間 26 分・4 条件の範囲。24/72 時間の実稼働は未実施）。attempt 2 で review の差し戻し 2 点（本番比較の指標、restart の計測）を直し、26 分の計測を測り直した。

## 結論
- **24/72 時間の実稼働は未実施である。** 人の承認の記録が無い（この task の受け入れ条件 2）。実施の手順は「24/72 時間の実稼働の手順（人の承認が要る）」に書いた。
- 非本番の 26 分の実時間の計測（isolate は窓の中で 1 個のまま）で、本番の標本と同じ経過秒の点・同じ式（2 点の差 ÷ 時間）で比べた RSS の傾きは次のとおりである。全区間（0→1,529 秒）は 76.0〜88.7 MB/時で、本番の 935.5 MB/時の 8.1〜9.5%。定常（600→1,529 秒）は 7.2〜18.5 MB/時で、本番の 746.9 MB/時の 1.0〜2.5%。増分（0→1,529 秒）は 32,968〜38,544 kB で、本番の 406,848 kB の 8.1〜9.5%。V8 の used heap は 26 分を通して最大 2.9 MB だった。**この負荷では、本番の増加は V8・compare では再現しなかった。** 本番の増加の原因は未確認のままである（下の「本番の観測との比較」）。
- **リース回収と restart を実測した。** 窓の後に、run 2 本とリース 2 本を持つ別 process（daemon の役）を SIGKILL し、新しい dispatcher（daemon と同じ孤児の回収の設定）で回収した。元の run が閉じるまで 3,150〜3,305 ms、再実行が終わるまで 4,727〜4,956 ms で、リースの期限（残り 660 秒）は待たなかった。持ち主が生きている間は 1 本も奪わなかった（対照）。回収の時間の大半は restart の後の最初の tick（3,030〜3,176 ms）で、blocked の task（622〜657 件）の数に比例して伸びる dispatch の遅延と同じ原因である。
- 仮想時刻の 72 時間（20 案件、36 時間目に restart）で、controller の評価は案件 × 時間の 1,440 回、LLM の起動は 0、restart の後の未消費の wake と未適用の効果は 0 だった。wake の行（1,860 行）と判断の行（1,440 行）は時間に比例して増え、消されない。この restart は tick の間で行い、判断の途中の lease の回収は測っていない。
- 実時間の計測で、dispatch の遅延（task の挿入から `routing_decided` と run の終わりまで）は blocked の task の数に比例して伸びた（p50 205 ms → 3,953 ms、blocked 0 → 655 件）。4 条件で同じなので V8 の影響ではない。本番の task の数では効いている可能性がある（未確認。提案 2）。

## main と既存実装の照合
- 基準は `git merge-base HEAD main`。`agent-docs/progress/2026-11-04-long-horizon.md`・a1〜a9、`2026-11-05-a9-llm-pilot.md`、`2026-11-05-a9-fix-arrival.md`、ADR `2026-11-05-a9-llm-pilot-findings`、`docs/ops/project-controller.md`、兄弟の p0a〜p1h を読んだ。
- 既にあったもの（再実装しない）: A6 の `project_controller_replay_resource_loop`（dispatcher の段、100 案件、RSS 36〜39 MB）と `project_controller_replay_resource_v8_100_projects`（isolate 1 個、RSS +3.3 MB）、A6 の仮想時刻の replay（crash 7 境界・3 日の時計の飛び等）、S6 の RSS・isolate の測定（generation 1 個で +22.5〜23.2 MiB）。
- 足りなかったもの: V8 heap を RSS と分けて読む口、off・V8 load・shadow・enabled を同じ負荷で揃えた比較、数十分以上の時系列、複数日の wake・判断の行の増え方、restart をまたぐ計測。A6 の未解決 6（複数日の RSS の推移）と a9 の未解決 4（実時間の数時間の連続稼働）。
- 旧 milestones API（410）・Project と Task の意味論・Rust の state/safety と TS の decision の分離・既存の認可には触れていない。TS policy・bundle は変えていない（verify の p50/p95 の規則の対象の変更は無い）。研究 K2/K4・IEEE Access の凍結評価、本番の config・DB・daemon・systemctl・promote には触れていない。

## 変更（commit 件名）
- `p1i: V8 heap の統計を読む口と、4 条件の長時間計測・仮想 72 時間の試験を足す`
- `p1i: 4 条件の 26 分の実時間計測の raw と driver script を足す`
- `p1i: 計測計画と結果・本番の観測との比較を進捗と ADR に書く`
- `p1i: restart を窓の外に移し、run を持つ process の SIGKILL でリース回収を測る`（attempt 2）
- `p1i: 26 分の計測を測り直し、本番比較を同じ式に直して進捗と ADR に書く`（attempt 2）

| 場所 | 内容 |
|---|---|
| `crates/script-runtime/src/{actor,handle,lib}.rs` | `ScriptRuntimeHandle::heap_stats()`・`HeapStats`・`Control::HeapStats`（ADR [2026-11-05-phase3-p1i](../../adr/2026-11-05-phase3-p1i.md)） |
| `crates/celeris/tests/long_run_soak.rs` | 試験 `long_run_soak_{off,v8_load,shadow,enabled}`（`build_dispatcher`＋本物の bundle と V8、窓の中は restart なし。窓の後に `long_run_soak_lease_holder` を別 process で起こして SIGKILL し、リース回収を測る） |
| `crates/task-dispatch/src/dispatcher/tests/project_controller_soak.rs` | 試験 `project_controller_soak_72h_{off,shadow,enabled}`（注入した時計、20 案件、72 時間、36 時間目に restart） |
| `scripts/dev/long-run-soak.sh` | 4 条件を別 process で同時に走らせ、集計する（attempt 2 で相対 path の `--out` で落ちるのを直した） |
| `scripts/dev/long_run_soak_summary.py` | `soak.jsonl` の条件ごとの集計と、本番の 4 標本と同じ経過秒の点・同じ式の比較。restart の前に run かリースが無い raw は exit 2 |

## 計測計画
| 段 | 方法 | 条件 | 見るもの | 状態 |
|---|---|---|---|---|
| 1 仮想時刻 | `project_controller_soak_72h_*`（fake decider、注入した時計、5 分刻み 864 tick） | controller の mode off・shadow・enabled、20 案件、毎時の cron、6 時間ごとに root の完了、2 時間ごとに 5 案件へ外部の wake、36 時間目に restart | wake の行・未消費・判断の行・評価の数・blocked・未適用の効果・CoS 入力・LLM の起動・tick の段の遅延・wake から判断までの遅れ（仮想秒）・RSS | 実施 |
| 2 非本番の実時間 | `long-run-soak.sh --secs 1560 --tasks 5 --pace-ms 250 --seed-events 200000` | off・v8_load・shadow・enabled、各 1 process、events 200,000 行を seed。窓の中は restart なし（isolate 1 個）。窓の後に run 2 本を持つ別 process を SIGKILL | RSS・RssAnon・threads・V8 heap・isolate・比較行・events の行・dispatch の遅延・`store.list` の時間・run の数。restart では SIGKILL の前の running の run とリース、元の run の回収と再実行の終わりまでの時間、二重の実行の有無 | 実施（26 分、本番の観測と同じ長さ） |
| 3 非本番の daemon 全体で 24/72 時間 | 下の手順（人が実行） | 段 2 と同じ 4 条件、本物の daemon binary、fake harness | 段 2 と同じ。加えて `GET /daemon` の `script_tick_wait`・journal の slow tick | **未実施（承認なし）** |
| 4 本番の観測 | 本番 daemon の `/proc/<pid>/status` を 10 分ごとに読む（読み取り専用） | 今の本番の mode のまま | RSS・RssAnon・threads | **未実施（本番の操作は人）** |

合否の目安（段 3・4 で使う。段 1・2 では判定でなく記録）: 定常の RSS の傾き（2 点の式）が off との差で +10 MB/時以内、V8 の used heap が 24 時間で 2 倍未満、isolate は generation の数（≤2）、run を持ったまま止めた restart で元の run が 1 回ずつ閉じ、リースの期限を待たずに回収され、restart の後の running の run とリースが 0、LLM の起動は fake harness の run の数と一致。

## 結果 1: 仮想時刻 72 時間（raw `p1i/soak-virtual.jsonl`）
| 指標（72 時間の終わり） | off | shadow | enabled |
|---|---:|---:|---:|
| tick | 864 | 864 | 864 |
| 評価（decide）の数 | 0 | 1,440 | 1,440 |
| 判断の行 | 0 | 1,440 | 1,440 |
| wake の行（消されない） | 0 | 1,860 | 1,860 |
| 未消費の wake（最後の正時の後 2 tick） | 0 | 0 | 0 |
| blocked の案件 | 0 | 0 | 0 |
| 未適用の効果 | 0 | 0 | 0 |
| CoS 入力・LLM の起動 | 0・0 | 0・0 | 0・0 |
| wake から判断まで 最大（仮想秒） | - | 600 | 600 |
| tick の段 p50 / p95（µs） | 258 / 416 | 825 / 22,593 | 831 / 21,985 |
| RSS の増分（kB） | 7,636 | 8,884 | 8,556 |

- 評価の数は毎時の cron の照合（20 案件 × 72 時間 = 1,440）に畳まれ、root の完了 240 件・外部の wake 180 件は同じ評価に合流した。
- restart（36→37 時間目）の前後で評価は 1 時間 20 回のまま続き、wake の取りこぼしは無かった。
- wake の行は 1 案件 1 時間あたり約 1.3 行、判断の行は 1 行ずつ増え、消費済みの行を消す仕組みは無い（A6 の未解決 2 と同じ。提案 1）。
- 試験の不変条件（`project_controller_soak_72h_*` の assert）: LLM 0、CoS 入力 0、未適用の効果 0、off は評価 0、shadow・enabled は未消費の wake 0、評価は 1,420 以上・上限以下、wake の遅れは 600 秒以内。

## 結果 2: 非本番の実時間 26 分・4 条件（raw `p1i/soak26/`）
`bash scripts/dev/long-run-soak.sh --out agent-docs/progress/2026-11-05-phase3/p1i/soak26 --secs 1560 --tasks 5 --pace-ms 250 --seed-events 200000` → exit 0（2026-11-05 14:47:50Z〜15:14:43Z、4 条件とも `test result: ok. 1 passed`）。attempt 1 の raw（窓の途中で restart した版）はこの raw で置き換えた。

| 指標 | off | v8_load | shadow | enabled | 本番の観測 |
|---|---:|---:|---:|---:|---:|
| 窓の壁時計（秒） | 1568.5 | 1566.5 | 1571.9 | 1569.7 | 1529（4 標本の間） |
| dispatch 数 | 655 | 655 | 625 | 620 | - |
| RSS 0 秒（kB） | 45,104 | 67,564 | 67,764 | 67,776 | 181,820 |
| RSS 約 600 秒（kB） | 75,056 | 100,528 | 101,440 | 102,228 | 391,292 |
| RSS 約 1,529 秒（kB） | 78,072 | 102,432 | 106,308 | 106,196 | 588,668 |
| 増分 0→1,529 秒（kB）・本番比 | 32,968・8.1% | 34,868・8.6% | 38,544・9.5% | 38,420・9.4% | 406,848 |
| 傾き 全区間 0→1,529 秒（MB/時）・本番比 | 76.0・8.1% | 80.5・8.6% | 88.7・9.5% | 88.6・9.5% | 935.5 |
| 傾き 定常 600→1,529 秒（MB/時）・本番比 | 11.4・1.5% | 7.2・1.0% | 18.5・2.5% | 15.0・2.0% | 746.9 |
| RSS 最初の cycle の後（kB） | 71,532 | 93,556 | 94,280 | 94,272 | - |
| RssAnon 増分（kB） | 12,388 | 16,604 | 19,536 | 19,988 | - |
| threads 開始 → 終了 | 4 → 6 | 10 → 12 | 10 → 12 | 10 → 12 | 29 → 38 |
| V8 used heap 開始 / 最大 / 終了（B） | - | 1,451,584 / 1,451,584 / 1,451,584 | 1,415,104 / 2,874,280 / 2,627,464 | 1,415,104 / 2,881,496 / 2,539,552 | - |
| V8 total heap 最大（B） | - | 3,407,872 | 4,456,448 | 4,456,448 | - |
| isolate の作り直し・timeout（窓の中） | - | 0・0 | 0・0 | 0・0 | 0（restart なし） |
| 比較行（窓の中） | 0 | 0 | 625 | 620 | 2 |
| events の行（終了） | 207,860 | 207,860 | 208,125 | 208,060 | 783,007 |
| dispatch の遅延 p50 最初 → 最後の cycle（ms） | 205 → 3,953 | 206 → 3,934 | 211 → 4,176 | 210 → 4,205 | - |
| `store.list` 最初 → 最後（µs） | 10,728 → 47,513 | 11,142 → 47,506 | 10,420 → 46,718 | 10,694 → 46,308 | - |
| blocked の task（窓の終わり） | 655 | 655 | 625 | 620 | - |

- 「本番比」は試験の値 ÷ 本番の値。どちらも同じ式（2 点の差 ÷ 2 点の間の時間）で、試験は本番の標本と同じ経過秒に最も近い標本を使う（`long_run_soak_summary.py` の `rss_at_offsets`。ずれは 6 秒以内）。MB は 1,024 kB。attempt 1 の「2〜3%」は、本番の 2 点の傾きと試験の最小二乗の定常の傾きを割った値で、指標が揃っていなかった。最小二乗の傾きは集計から消した。
- RSS の増分の大半は最初の cycle（seed の後の最初の dispatch、+26,428〜26,516 kB）で、その後は 4 条件ほぼ同じ。定常の傾きの off との差は -4.2〜+7.1 MB/時で、600 秒と 1,529 秒の 2 点だけの値なのでこの幅は誤差を含む。
- V8 の値は窓の 26 分を通した 1 個の isolate の値である（generation の番号は開始と終了で同じ、作り直し 0）。
- `shadow`・`enabled` の比較の記録は dispatch 1 件に 1 行。`v8_load` は 0 行（isolate を載せても TS を呼ばない）。
- fake harness は question を返すので task は blocked で残る。dispatch の遅延と `store.list` の時間は、blocked の task の数とともに 4 条件で同じように伸びた。

## 結果 3: run を持ったまま止めた restart とリース回収（raw `p1i/soak26/soak.jsonl` の `before_crash`・`restart`・`after_restart` の行）
手順（`crash_and_reclaim`）: 窓の終わりに親の dispatcher と isolate を捨てる。同じ DB に task を 2 件入れ、同じ test binary の `long_run_soak_lease_holder` を別 process で起こす（daemon の役。`daemon_instances` に自分の行を書き、heartbeat を打つ。harness は hold file がある間は結果を返さない）。DB で running の run 2 本とリース 2 本を確かめ、孤児の回収を有効にした dispatcher で 1 tick して奪わないこと（対照）を確かめてから SIGKILL する。hold file を消し、新しい isolate と dispatcher（`OrphanTakeover`、`pid_alive` は `/proc`、heartbeat の窓 60 秒）で tick し、時間を測る。

| 指標 | off | v8_load | shadow | enabled |
|---|---:|---:|---:|---:|
| SIGKILL の前の running の run・リース | 2・2 | 2・2 | 2・2 | 2・2 |
| SIGKILL の時点のリースの残り（秒） | 660 | 660 | 660 | 660 |
| 持ち主が生きている間に奪った run（対照） | 0 | 0 | 0 | 0 |
| blocked の task（restart の時） | 657 | 657 | 627 | 622 |
| isolate の起動・dispatcher の組み立て（ms） | 0・114 | 8・113 | 8・113 | 8・112 |
| restart の後の最初の tick（ms） | 3,030 | 3,063 | 3,176 | 3,167 |
| SIGKILL から元の run が閉じるまで（ms） | 3,150 | 3,192 | 3,305 | 3,296 |
| SIGKILL から再実行が終わるまで（ms） | 4,727 | 4,800 | 4,956 | 4,956 |
| 元の run の `interrupted: … orphan …`・`orphan_takeover:` の記録 | 2・2 | 2・2 | 2・2 | 2・2 |
| 再実行の run（task ごと 1 本） | 2 | 2 | 2 | 2 |
| restart の後の dispatch の後の running の run・リース | 0・0 | 0・0 | 0・0 | 0・0 |

- 元の run は孤児の回収で 1 回ずつ閉じ、task ごとに新しい run が 1 本起きて終わった。二重の実行は残らなかった。リースの期限（660 秒）は待たなかった。
- 回収の時間の大半は最初の tick（約 3 秒）である。同じ restart を blocked 約 60 件の短い計測（`p1i/soak-short/`）で行うと、回収は 454〜493 ms、再実行の終わりまで 670〜727 ms だった。tick の時間は blocked の task の数とともに伸びる（未解決 4・提案 2）。
- 測っていないもの: SIGTERM による正常な停止（drain）の restart、リースの期限切れの経路（孤児の回収を持たない dispatcher）、v2 の WU のリース、判断の途中の project controller の lease。これらは既存の試験（`orphan_takeover.rs`・`drain_hand_off.rs`・A6 の crash の境界）が受け持ち、時間は測っていない。

## 本番の観測との比較（条件の揃え方）
| 条件 | 本番（2026-11-04 02:40:43〜03:06:12Z） | 段 2 の `shadow` | 揃ったか |
|---|---|---|---|
| 長さ・標本 | 1,529 秒、4 標本（0・600・1,200・1,529 秒） | 1,572 秒、125 cycle。本番と同じ経過秒に最も近い標本を使う | 揃えた |
| 比べる式 | 2 点の差 ÷ 時間 | 同じ式・同じ経過秒 | 揃えた |
| 開始の点 | daemon の起動 26 秒後 | seed と isolate の起動の後、最初の dispatch の前 | 揃わない（本番は起動の 26 秒の分を含まない。試験は最初の dispatch の +26 MB を含む） |
| V8 | generation 1 個・isolate 1 個・restart なし | isolate 1 個・窓の中は restart なし | 揃えた |
| compare の領域 | `lane_rules`・`cos_digest` | `lane_rules` だけ | 揃わない（CoS chat の pipeline が無い） |
| compare の件数 | 2 件 | 625 件 | 試験の方が約 300 倍多い |
| DB | events 783,007 行 | 200,000 行を seed、終了時 208,125 行 | 約 1/4 |
| process の中身 | daemon 全体（API・web 中継・cron・chat・実 worker・ssh） | `build_dispatcher` の dispatcher と fake harness | 揃わない |
| threads | 29 → 38 | 10 → 12 | 揃わない |

- 同じ式・同じ区間で、試験の RSS の傾きは本番の 8.1〜9.5%（全区間）、1.0〜2.5%（定常）だった。試験は本番より compare を約 300 倍多く呼んでいる。本番の 26 分の +407 MB は、V8 の heap（最大 2.9 MB）と compare の回数では説明できない。
- 本番の RssAnon（520,700 kB）は匿名の memory で、SQLite の page cache・malloc の arena（threads 29→38 で arena が増える）・daemon の背景の仕事のどれかが候補である。どれかは未確認。
- 本番の値は 1 回の起動の 4 標本で、off と V8 load の本番の基準は無い。同じ daemon で off（`[script]` なし）の 26 分を測れば、V8 の寄与を本番で切り分けられる（段 4、人の操作）。

## 24/72 時間の実稼働の手順（人の承認が要る。未実施）
承認の範囲: 非本番の host の上で、本番と別の `CELERIS_HOME`・別 port の daemon を 4 個（off・v8_load・shadow・enabled）24 時間（延長で 72 時間）動かす。harness は fake（LLM の課金なし）。本番の DB・config・systemd には触れない。CPU は dispatch の間隔（tick 50 ms・投入 1 件／10 秒）で抑える。
1. `cargo build --release -p celeris` の binary と `long-run-soak.sh` が作る bundle を使い、条件ごとに一時の home に config（`[script] state_dir`、fake harness、`[cos]` なし）を書く。`v8_load` は generation を登録して mode rust、`shadow` は compare、`enabled` は canary を経て ts にする（`celerisctl script`）。
2. 各 daemon の pid の `/proc/<pid>/status`（VmRSS・RssAnon・Threads）を 10 分ごとに読み、`celeris.long-run-soak/1` の行で追記する。`GET /api/v1/daemon` の `script_tick_wait` と、`sqlite3 -readonly` で `script_compare_log`・`events`・`project_wakes` の行数も同じ時刻に読む。task は `celerisctl add` で 10 秒に 1 件入れる。
3. 12 時間目に、run が走っている間に各 daemon を SIGKILL して起こし直す（restart）。止める前に running の run とリースが 1 以上あることを、起こした後にそれが 0 に戻り、元の run が 1 回ずつ `orphan_takeover` で閉じることを `sqlite3 -readonly` で確かめる。
4. 終わったら `python3 -I scripts/dev/long_run_soak_summary.py <soak.jsonl> --json … --markdown …` で集計し、上の合否の目安で判定する。
人が決めること: 実施の可否、host、24 時間か 72 時間か。段 4（本番 daemon の RSS を読むだけ、off との比較には本番の `[script]` を外す再起動が要る）を行うか。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `PROJECT_CONTROLLER_SOAK_RAW_DIR=<dir> cargo nextest run -p task-dispatch project_controller_soak_` → 3 passed（attempt 1。raw は `p1i/soak-virtual.jsonl`。attempt 2 ではコメントだけ直し、test-affected の中で再び 3 passed）。
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_BUNDLE_DIR=<bundle> cargo nextest run -p celeris --test long_run_soak` → 4 passed、1 skipped（`long_run_soak_lease_holder` は `#[ignore]`。親が別 process で起こす）、2.9 秒。
- `bash scripts/dev/long-run-soak.sh --out agent-docs/progress/2026-11-05-phase3/p1i/soak-short --secs 0 --seed-events 2000` → exit 0（raw `p1i/soak-short/`）。
- `bash scripts/dev/long-run-soak.sh --out agent-docs/progress/2026-11-05-phase3/p1i/soak26 --secs 1560 --tasks 5 --pace-ms 250 --seed-events 200000` → exit 0（raw `p1i/soak26/`、壁時計 27 分）。
- TEST_AFFECTED_RESULT
- `cargo clippy --workspace -- -D warnings` → CLIPPY_RESULT。`cargo fmt --all -- --check` → FMT_RESULT。
- `sh scripts/dev/progress-index.sh --check` → INDEX_RESULT。`test -f agent-docs/progress/2026-11-05-phase3/p1i.md` → exit 0。

## 失敗ケース（この run の中で起きて直したもの）
1. 仮想 72 時間の試験の最初の版は `every wake is consumed` で落ちた（shadow・enabled）。毎時の cron の wake は正時の tick で積まれ、次の tick で消費される。最後の正時の後に 2 tick 進めてから数えるように直した（製品のコードの不具合ではない）。
2. 同じ試験は 1 分刻み（4,320 tick）で 312 秒かかった。5 分刻み（864 tick）にして 94 秒にした。
3. `cargo nextest run -p task-dispatch` が `could not find native static library rusty_v8` で落ちた。中身の空の `v8-*` の build dir を消して作り直すと通った（既知）。
4. attempt 1 の review で差し戻された 2 点: (a) 本番比較の「2〜3%」は本番の 2 点の傾きと試験の最小二乗の傾きの比で、指標が違った。(b) `long_run_soak` の restart は run の終わりを待った後に起き、回収の対象（running の run・リース）が 0 で、assert は必ず通った。attempt 2 で、比較を同じ式・同じ経過秒に直し、restart を「run 2 本とリース 2 本を持つ別 process の SIGKILL」に替え、前に run とリースがあることを assert と集計（exit 2）で確かめるようにした。
5. attempt 2 で `long-run-soak.sh` に相対 path の `--out` を渡すと、test binary を `crates/celeris` で起こすため log の path が解決できず exit 1 になった。`--out` を絶対 path に直してから使うようにした。

## 未解決
1. 24/72 時間の実稼働と本番の観測（段 3・4）は未実施。人の承認が要る。
2. 本番の RSS の増加（182→589 MB／1,529 秒）の原因は未確認。この試験の範囲では V8・compare は原因でない（同じ式で、傾きは本番の 8.1〜9.5%（全区間）・1.0〜2.5%（定常））。daemon 全体（API・cron・chat・実 worker）・SQLite の page cache・malloc の arena は試していない。試験の開始の点は本番（起動 26 秒後）と揃っていない。
3. `cos_digest` の compare は CoS chat の pipeline が要り、段 2 では比べていない。
4. dispatch の遅延と `store.list` の時間が blocked の task の数に比例して伸びる（655 件で 4 秒）。restart の後の孤児の回収も最初の tick を待つので、同じ件数で約 3 秒かかった。本番の件数でどれだけ効くかは未確認。
7. restart の計測は SIGKILL（異常終了）の 1 形だけで、各条件 1 回・run 2 本。SIGTERM の drain、リースの期限切れの経路、v2 の WU のリース、判断の途中の project controller の lease の回収の時間は測っていない。仮想 72 時間の restart は tick の間で行い、lease の回収は含まない。
5. wake の行と判断の行は消されず、時間に比例して増える（20 案件で最初の 24 時間に wake 620 行・判断 460 行）。
6. V8 heap は試験の中の handle からしか読めない。本番の daemon の `GET /daemon` には出ない。

## 提案
1. 消費済みの wake と効果なし（`none`）の判断の行の保存期間を決め、古い行を片付ける cron job を置く（A6 の未解決 2 と合わせて別 task。events は追記専用のまま）。
2. dispatch の tick で blocked の task を毎回読む経路を測り、件数に比例しない形にする（別 task。`store.list` 全件の読みが候補）。
3. `GET /api/v1/daemon` に script の `heap_stats` と VmRSS・RssAnon・threads を足し、本番で V8 と RSS を分けて読めるようにする（設計の判断が要る。この task では足していない）。
4. 本番の RSS の原因を切り分けるには、段 4 を off と shadow の 2 回（各 26 分）で行うのが最も安い。

## 次の gate
- final review（受け入れ条件 0〜3）。
- 人の判断: 段 3（非本番の 24/72 時間）と段 4（本番の RSS の読み取りと off の比較）を行うか。
