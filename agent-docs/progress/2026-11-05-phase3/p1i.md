---
title: P1-I 長時間運用と観測
tasks: [01M6N7XYA1GC9WVPFDSQE39D6V]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P1-I 長時間運用と観測

完了日: 2026-11-05（仮想時刻 72 時間・非本番の実時間 26 分・4 条件の範囲。24/72 時間の実稼働は未実施）

## 結論
- **24/72 時間の実稼働は未実施である。** 人の承認の記録が無い（この task の受け入れ条件 2）。実施の手順は「24/72 時間の実稼働の手順（人の承認が要る）」に書いた。
- 非本番の 26 分の実時間の計測で、RSS の増え方は 4 条件とも 19.7〜23.9 MB/時だった。本番の観測（26 分で 182→589 MB、約 917 MB/時）の 2〜3% である。V8 を載せると開始時の RSS は +22,652 kB 上がるが、その後の傾きは off と +4 MB/時しか違わない。V8 の used heap は 1.4〜2.6 MB で止まった。**この負荷では、本番の増加は V8・compare では再現しなかった。** 本番の増加の原因は未確認のままである（下の「本番の観測との比較」）。
- 仮想時刻の 72 時間（20 案件、36 時間目に restart）で、controller の評価は案件 × 時間の 1,440 回、LLM の起動は 0、restart の後の未消費の wake と未適用の効果は 0 だった。wake の行（1,860 行）と判断の行（1,440 行）は時間に比例して増え、消されない。
- 実時間の計測で、dispatch の遅延（task の挿入から `routing_decided` と run の終わりまで）は blocked の task の数に比例して伸びた（p50 259 ms → 3,954 ms、blocked 0 → 660 件）。4 条件で同じなので V8 の影響ではない。本番の task の数では効いている可能性がある（未確認。提案 2）。

## main と既存実装の照合
- 基準は `git merge-base HEAD main`。`agent-docs/progress/2026-11-04-long-horizon.md`・a1〜a9、`2026-11-05-a9-llm-pilot.md`、`2026-11-05-a9-fix-arrival.md`、ADR `2026-11-05-a9-llm-pilot-findings`、`docs/ops/project-controller.md`、兄弟の p0a〜p1h を読んだ。
- 既にあったもの（再実装しない）: A6 の `project_controller_replay_resource_loop`（dispatcher の段、100 案件、RSS 36〜39 MB）と `project_controller_replay_resource_v8_100_projects`（isolate 1 個、RSS +3.3 MB）、A6 の仮想時刻の replay（crash 7 境界・3 日の時計の飛び等）、S6 の RSS・isolate の測定（generation 1 個で +22.5〜23.2 MiB）。
- 足りなかったもの: V8 heap を RSS と分けて読む口、off・V8 load・shadow・enabled を同じ負荷で揃えた比較、数十分以上の時系列、複数日の wake・判断の行の増え方、restart をまたぐ計測。A6 の未解決 6（複数日の RSS の推移）と a9 の未解決 4（実時間の数時間の連続稼働）。
- 旧 milestones API（410）・Project と Task の意味論・Rust の state/safety と TS の decision の分離・既存の認可には触れていない。TS policy・bundle は変えていない（verify の p50/p95 の規則の対象の変更は無い）。研究 K2/K4・IEEE Access の凍結評価、本番の config・DB・daemon・systemctl・promote には触れていない。

## 変更（commit 件名）
- `p1i: V8 heap の統計を読む口と、4 条件の長時間計測・仮想 72 時間の試験を足す`
- `p1i: 4 条件の 26 分の実時間計測の raw と driver script を足す`
- `p1i: 計測計画と結果・本番の観測との比較を進捗と ADR に書く`

| 場所 | 内容 |
|---|---|
| `crates/script-runtime/src/{actor,handle,lib}.rs` | `ScriptRuntimeHandle::heap_stats()`・`HeapStats`・`Control::HeapStats`（ADR [2026-11-05-phase3-p1i](../../adr/2026-11-05-phase3-p1i.md)） |
| `crates/celeris/tests/long_run_soak.rs` | 試験 `long_run_soak_{off,v8_load,shadow,enabled}`（`build_dispatcher`＋本物の bundle と V8、半分で restart） |
| `crates/task-dispatch/src/dispatcher/tests/project_controller_soak.rs` | 試験 `project_controller_soak_72h_{off,shadow,enabled}`（注入した時計、20 案件、72 時間、36 時間目に restart） |
| `scripts/dev/long-run-soak.sh` | 4 条件を別 process で同時に走らせ、集計する |
| `scripts/dev/long_run_soak_summary.py` | `soak.jsonl` の条件ごとの集計と、本番の観測を並べた表 |

## 計測計画
| 段 | 方法 | 条件 | 見るもの | 状態 |
|---|---|---|---|---|
| 1 仮想時刻 | `project_controller_soak_72h_*`（fake decider、注入した時計、5 分刻み 864 tick） | controller の mode off・shadow・enabled、20 案件、毎時の cron、6 時間ごとに root の完了、2 時間ごとに 5 案件へ外部の wake、36 時間目に restart | wake の行・未消費・判断の行・評価の数・blocked・未適用の効果・CoS 入力・LLM の起動・tick の段の遅延・wake から判断までの遅れ（仮想秒）・RSS | 実施 |
| 2 非本番の実時間 | `long-run-soak.sh --secs 1560 --tasks 20 --pace-ms 250 --seed-events 200000` | off・v8_load・shadow・enabled、各 1 process、events 200,000 行を seed、半分で restart | RSS・RssAnon・threads・V8 heap・isolate・比較行・events の行・dispatch の遅延・`store.list` の時間・run の数・restart の後の running の run とリース | 実施（26 分、本番の観測と同じ長さ） |
| 3 非本番の daemon 全体で 24/72 時間 | 下の手順（人が実行） | 段 2 と同じ 4 条件、本物の daemon binary、fake harness | 段 2 と同じ。加えて `GET /daemon` の `script_tick_wait`・journal の slow tick | **未実施（承認なし）** |
| 4 本番の観測 | 本番 daemon の `/proc/<pid>/status` を 10 分ごとに読む（読み取り専用） | 今の本番の mode のまま | RSS・RssAnon・threads | **未実施（本番の操作は人）** |

合否の目安（段 3・4 で使う。段 1・2 では判定でなく記録）: 定常の RSS の傾きが off との差で +10 MB/時以内、V8 の used heap が 24 時間で 2 倍未満、isolate は generation の数（≤2）、restart の後の running の run とリースが 0、LLM の起動は fake harness の run の数と一致。

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
`bash scripts/dev/long-run-soak.sh --out <dir> --secs 1560 --tasks 20 --pace-ms 250 --seed-events 200000` → exit 0（13:50:29Z〜14:18:34Z、4 条件とも `test result: ok. 1 passed`）。

| 指標 | off | v8_load | shadow | enabled | 本番の観測 |
|---|---:|---:|---:|---:|---:|
| 壁時計（秒） | 1568.3 | 1578.1 | 1641.1 | 1636.5 | 1560.0 |
| dispatch 数 | 660 | 660 | 640 | 640 | - |
| RSS 開始（kB） | 44,720 | 67,372 | 67,188 | 67,008 | 181,820 |
| RSS 終了（kB） | 81,684 | 104,580 | 107,436 | 105,460 | 588,668 |
| RSS 増分（kB） | 36,964 | 37,208 | 40,248 | 38,452 | 406,848 |
| RSS 傾き 定常（MB/時） | 19.7 | 23.6 | 23.9 | 21.9 | 916.9 |
| RssAnon 増分（kB） | 17,572 | 18,008 | 20,472 | 19,252 | - |
| threads 終了 | 6 | 12 | 12 | 14 | 38 |
| V8 used heap 終了 / 最大（B） | - | 1,463,016 / 1,463,016 | 1,997,440 / 2,563,936 | 1,970,304 / 2,563,568 | - |
| V8 total heap 最大（B） | - | 3,407,872 | 4,194,304 | 4,194,304 | - |
| V8 の作り直し・timeout | - | 0・0 | 0・0 | 0・0 | - |
| 比較行 / dispatch | 0 | 0 | 1.0 | 1.0 | 2 件／26 分 |
| events の行（終了） | 207,920 | 207,920 | 208,320 | 208,320 | 783,007 |
| dispatch の遅延 p50 最初 → 最後の cycle（ms） | 259 → 3,954 | 259 → 4,013 | 267 → 4,309 | 273 → 4,307 | - |
| `store.list` 最初 → 最後（µs） | 11,283 → 46,874 | 11,623 → 47,781 | 11,824 → 47,005 | 11,392 → 46,011 | - |
| blocked の task（終了） | 660 | 660 | 640 | 640 | - |
| restart の回復（ms）・後の running の run | 1,988・0 | 2,017・0 | 2,131・0 | 2,108・0 | - |
| リースの残り（終了） | 0 | 0 | 0 | 0 | - |

- RSS の増分の大半は最初の cycle（seed の後の最初の dispatch、+26,296〜28,032 kB）で、その後は 20 MB/時前後で 4 条件ほぼ同じ。
- `shadow`・`enabled` の比較の記録は dispatch 1 件に 1 行。`v8_load` は 0 行（isolate を載せても TS を呼ばない）。
- fake harness は question を返すので task は blocked で残る。dispatch の遅延と `store.list` の時間は、blocked の task の数とともに 4 条件で同じように伸びた。

## 本番の観測との比較（条件の揃え方）
| 条件 | 本番（2026-11-04 02:40〜03:06Z） | 段 2 の `shadow` | 揃ったか |
|---|---|---|---|
| 長さ | 26 分（起動 26 秒後から） | 26 分（seed の後から） | 揃った |
| V8 | generation 1 個・isolate 1 個 | isolate 1 個（restart で作り直し 1 回） | 揃った |
| compare の領域 | `lane_rules`・`cos_digest` | `lane_rules` だけ | 揃わない（CoS chat の pipeline が無い） |
| compare の件数 | 2 件（26 分） | 640 件（26 分） | 試験の方が 320 倍多い |
| DB | events 783,007 行 | 200,000 行を seed、終了時 208,320 行 | 約 1/4 |
| process の中身 | daemon 全体（API・web 中継・cron・chat・実 worker・ssh） | `build_dispatcher` の dispatcher と fake harness | 揃わない |
| threads | 29 → 38 | 10 → 12 | 揃わない |

- 試験は本番より compare を 320 倍多く呼んで、RSS の傾きは本番の 2.6%（23.9 / 916.9 MB/時）だった。本番の 26 分の +407 MB は V8 の heap（最大 2.6 MB）と compare の回数では説明できない。
- 本番の RssAnon（520,700 kB）は匿名の memory で、SQLite の page cache・malloc の arena（threads 29→38 で arena が増える）・daemon の背景の仕事のどれかが候補である。どれかは未確認。
- 本番の値は 1 回の起動の 4 標本で、off と V8 load の本番の基準は無い。同じ daemon で off（`[script]` なし）の 26 分を測れば、V8 の寄与を本番で切り分けられる（段 4、人の操作）。

## 24/72 時間の実稼働の手順（人の承認が要る。未実施）
承認の範囲: 非本番の host の上で、本番と別の `CELERIS_HOME`・別 port の daemon を 4 個（off・v8_load・shadow・enabled）24 時間（延長で 72 時間）動かす。harness は fake（LLM の課金なし）。本番の DB・config・systemd には触れない。CPU は dispatch の間隔（tick 50 ms・投入 1 件／10 秒）で抑える。
1. `cargo build --release -p celeris` の binary と `long-run-soak.sh` が作る bundle を使い、条件ごとに一時の home に config（`[script] state_dir`、fake harness、`[cos]` なし）を書く。`v8_load` は generation を登録して mode rust、`shadow` は compare、`enabled` は canary を経て ts にする（`celerisctl script`）。
2. 各 daemon の pid の `/proc/<pid>/status`（VmRSS・RssAnon・Threads）を 10 分ごとに読み、`celeris.long-run-soak/1` の行で追記する。`GET /api/v1/daemon` の `script_tick_wait` と、`sqlite3 -readonly` で `script_compare_log`・`events`・`project_wakes` の行数も同じ時刻に読む。task は `celerisctl add` で 10 秒に 1 件入れる。
3. 12 時間目に各 daemon を止めて起こし直す（restart）。running の run とリースが 0 に戻ることを `sqlite3 -readonly` で確かめる。
4. 終わったら `python3 -I scripts/dev/long_run_soak_summary.py <soak.jsonl> --json … --markdown …` で集計し、上の合否の目安で判定する。
人が決めること: 実施の可否、host、24 時間か 72 時間か。段 4（本番 daemon の RSS を読むだけ、off との比較には本番の `[script]` を外す再起動が要る）を行うか。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `PROJECT_CONTROLLER_SOAK_RAW_DIR=<dir> cargo nextest run -p task-dispatch project_controller_soak_` → 3 passed（93.7 秒）。raw は `p1i/soak-virtual.jsonl`。
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 CELERIS_SCRIPT_BUNDLE_DIR=<bundle> cargo nextest run -p celeris --test long_run_soak` → 4 passed（既定の小さい負荷、4.4 秒）。
- `bash scripts/dev/long-run-soak.sh --out <dir> --secs 0 --seed-events 2000` → exit 0（raw `p1i/soak-short/`）。
- `bash scripts/dev/long-run-soak.sh --out <dir> --secs 1560 --tasks 20 --pace-ms 250 --seed-events 200000` → exit 0（raw `p1i/soak26/`、壁時計 28 分）。
- `bash scripts/dev/test-affected.sh` → exit 0（3,496 tests run: 3,496 passed、9 skipped、script bundle SKIP 0 件、test-ts-verify 20 passed、文書検査 ok）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。`test -f agent-docs/progress/2026-11-05-phase3/p1i.md` → exit 0。

## 失敗ケース（この run の中で起きて直したもの）
1. 仮想 72 時間の試験の最初の版は `every wake is consumed` で落ちた（shadow・enabled）。毎時の cron の wake は正時の tick で積まれ、次の tick で消費される。最後の正時の後に 2 tick 進めてから数えるように直した（製品のコードの不具合ではない）。
2. 同じ試験は 1 分刻み（4,320 tick）で 312 秒かかった。5 分刻み（864 tick）にして 94 秒にした。
3. `cargo nextest run -p task-dispatch` が `could not find native static library rusty_v8` で落ちた。中身の空の `v8-*` の build dir を消して作り直すと通った（既知）。

## 未解決
1. 24/72 時間の実稼働と本番の観測（段 3・4）は未実施。人の承認が要る。
2. 本番の RSS の増加（182→589 MB／26 分）の原因は未確認。この試験の範囲では V8・compare は原因でない（傾きは本番の 2〜3%）。daemon 全体（API・cron・chat・実 worker）・SQLite の page cache・malloc の arena は試していない。
3. `cos_digest` の compare は CoS chat の pipeline が要り、段 2 では比べていない。
4. dispatch の遅延と `store.list` の時間が blocked の task の数に比例して伸びる（660 件で 4 秒）。本番の件数でどれだけ効くかは未確認。
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
