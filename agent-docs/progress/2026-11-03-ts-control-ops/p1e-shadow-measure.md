---
title: P1-E(2) lane_rules と cos_digest の本番 compare の実測の集計
tasks: [01M6HGFR1TVY299MQPZY7HQQG9]
status: done
completed: 2026-11-04
updated: 2026-11-04
---
# P1-E(2) lane_rules と cos_digest の本番 compare の実測の集計

task 01M6HGFR1TVY299MQPZY7HQQG9（親 01M6HG7T6ZKTX239KGH0QY4HFQ の p1e-shadow-measure）。手順は [p1e-shadow-preflight.md](p1e-shadow-preflight.md) の「本番への移行（人の手順）」。基準は `git merge-base HEAD main`。

## 結論
- **件数が足りないので、canary に進む根拠はまだ無い。** 2026-11-04T03:06:11Z の時点で `lane_rules` は 1/1000 件、`cos_digest` は 1/200 件である。report は exit 4（矛盾は無いが件数不足）。synthetic で補っていない。
- 2 件とも match で、mismatch・explained・ts_error・invalid_output・unavailable は 0、安全違反 0、guard の停止 0、rust への戻し 0、受信箱 0 である。n=1 ずつなので、一致率 1.0 は判断の材料にならない。
- **件数の見込みを訂正する。** P1-E(1) の 48.45 件/日（1000 件まで約 20.6 日、2026-11-25 前後）は誤りだった。WU の run を数えていた。WU の run を除くと **9.33 件/日、1000 件まで約 107 日（2027-02 中旬）** である。人が別 task 01M6J81DZ7C8SMY7HNZTW8GT0Z で検討している 100 件なら、同じ率で約 10.7 日である。`cos_digest` は 55.31 件/日で 200 件まで約 3.6 日（2026-11-08 前後）である。
- **RSS が増え続けている。** daemon の RSS は起動 26 秒後の 181,820 kB から、10 分ごとの標本で 391,292 kB、505,532 kB と増え、03:06:12Z の `/proc` の読みでは 588,668 kB（うち RssAnon 520,700 kB）である。TS 層が原因かは未確認である（compare の前の同じ daemon の RSS の基準が無い）。host の空きは約 83 GB あり、今すぐの危険は無い。
- worker は本番の DB を `sqlite3 -readonly`（`PRAGMA query_only=ON`）で読み、journal・`/proc`・`~/p1e-samples.jsonl` を読んだだけで、本番に何も書いていない。release・promote・mode・guard・config・systemd・K2/K4/IEEE Access の環境は変えていない。canary・ts への昇格はしていない。`cos_digest` を compare にしたのは operator（人の判断、03:00:10Z）である。

## 実測（2026-11-04T03:06:11Z、events の上限 id 783007、script_compare_log の上限 id 1）
| 項目 | `lane_rules` | `cos_digest` |
|---|---|---|
| mode・version | compare・1（02:41:21Z） | compare・1（03:00:10Z） |
| 記録 | 1（02:41:58Z） | 1（03:03:35Z） |
| match / mismatch / explained | 1 / 0 / 0 | 1 / 0 / 0 |
| ts_error / invalid_output / unavailable | 0 / 0 / 0 | 0 / 0 / 0 |
| 一致率 | 1.0（n=1） | 1.0（n=1） |
| clean_streak / 要件 | 1 / 1000 | 1 / 200 |
| latency p50/p95/max | 431 / 431 / 431 µs | 1,297 / 1,297 / 1,297 µs |
| 安全違反・隠れた差分 | 0・0 | 0・0 |

| 全体の項目 | 値 |
|---|---|
| generation | 01M6J6X3HE4Z3MXM11GHJ1B6M4（2 領域とも）。`route_priority` は rust |
| rollback | mode 変更 2（どちらも rust→compare、operator:claude-code）、rust への戻し 0、自動の戻し 0、generation の event 0、受信箱 script_control 0 |
| guard の停止・shadow outbox の残り | 0・0 |
| script_tick_wait（標本 3 件） | p50/p95/max 0 µs、tick_budget 0。compare は TS を待たないので 0 が正しい |
| tick log（compare の後 25 分、45 行） | slow tick 1、tick budget 警告 0 |

### RSS と threads（pid 1659338、起動は 02:40:17Z）
| 時刻 | RSS（kB） | threads | 出所 |
|---|---:|---:|---|
| 02:40:43Z | 181,820 | 29 | 標本 |
| 02:50:43Z | 391,292 | 35 | 標本 |
| 03:00:43Z | 505,532 | 38 | 標本 |
| 03:06:12Z | 588,668 | 38 | `/proc/1659338/status`（標本の file の外） |

### tick の比較
| 区間 | 長さ | slow tick | 備考 |
|---|---:|---:|---|
| compare の前（01:39〜02:39Z、release を 2 回替えた区間） | 60 分 | 3 | 他に `slow tick phases` 12 行 |
| compare の後（02:41:21Z〜） | 25 分 | 1 | 03:02:14Z、6,788 ms、`dispatch_ms` 6,568 |

前後の頻度に差があるとは言えない（区間が短く、前の区間は再起動を含む）。後の 1 件の dispatch の中身は未確認である。compare の待ちではない（script_tick_wait は 0）。

### routing_decided と記録の突き合わせ（compare の後の 4 件）
4 件のうち記録になるのは 1 件で、残り 3 件は仕様どおり記録されない。記録の取りこぼしではない。
| 時刻 | 対象 | source | 記録 | 理由 |
|---|---|---|---|---|
| 02:41:58Z | task | default | あり | `decide_lane` |
| 03:01:02Z | task | system | なし | `source.policy_decides()` が false |
| 03:01:12Z | task（同じ task の再開） | default | なし | 直前の記録と input_digest・outcome が同じで追記しない |
| 03:02:19Z | work_unit | default | なし | `decide_lane_for_work_unit` は compare を呼ばない |

見込みの根拠（2026-10-20 以後の `routing_decided`）: task の run は default 476・human 241・system 537、WU の run は default 1,185。重複を抑えた記録は 141 件（141 task、15.11 日）で 9.33 件/日である。1 task からほぼ 1 件しか出ない。

raw:
- [p1e-measure-report.jsonl](p1e-measure-report.jsonl) — `report --format json` の全出力（1 行 1 回、`_run` に時刻と exit）。1 行目は 02:43Z の初回、2・3 行目が今回である。2・3 行目の `slow_ticks` 1 は message の照合に直した後の値で、実際の slow tick である
- [p1e-measure-samples.jsonl](p1e-measure-samples.jsonl) — `~/p1e-samples.jsonl` の写し（標本 3 件）

## 実行したコマンドと exit（提出時の HEAD で実行）
- `journalctl --user -u celeris@<current の release> --since "2026-11-04 02:41:21" --no-pager > $TMPDIR/p1e/daemon.log` → exit 0、45 行
- `python3 -I scripts/dev/script-compare.py report --db /local/celeris/data/db/celeris.sqlite3 --samples ~/p1e-samples.jsonl --tick-log $TMPDIR/p1e/daemon.log --require-ready lane_rules,cos_digest --format json` → exit 4
- 同じく `--format table` → exit 4、`not ready: clean_streak 1 < 1000` と `clean_streak 1 < 200`、PROBLEM 行なし
- `bash scripts/dev/tests/script-compare.sh` → exit 0
- `estimate_volume` は `script-compare.py` の関数を `--estimate-since 2026-10-20T00:00:00Z` と同じ引数で呼んだ → 9.33 件/日

## 変更（file・関数）
本番の値を読んで、集計の道具の誤りを 2 件直した。どちらも数え方の修正で、比較の判定・ready の条件・exit は変えていない。
- `scripts/dev/script-compare.py` の `summarize_tick_log`・新しい `tick_log_message`: journal の JSON の行は `fields.message` だけを見る。これまでは行の全体から語を探していたので、前回の worker の報告（`worker finished` の outcome）に入っていた「slow tick」「script_tick_wait」を数えていた（本番で slow_ticks が 1 多く出た）。JSON でない行は従来どおり行の全体から探す（数え漏らさない側）。
- `scripts/dev/script-compare.py` の `estimate_volume`: `record.work_unit_id` のある run を除く。WU の run は `decide_lane` を通らず（`crates/task-dispatch/src/dispatcher/work_units.rs` の `decide_lane_for_work_unit`）記録を作らない。
- `scripts/dev/tests/script-compare.sh`: JSON の log の集計と WU の run を除く見込みの試験を足した。
- `p1e-shadow-preflight.md`: 件数の見込みに訂正を書いた。
- 読んだだけの file: `crates/task-dispatch/src/dispatcher/dispatch_run.rs` の `decide_lane`、`crates/task-core/src/store/script_domains.rs` の重複の抑え。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0（`scripts/dev/tests/script-compare.sh` が全項目 ok、Rust の試験対象なし、文書検査 ok）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決
1. 純粋判定が `lane_rules` 1/1000、`cos_digest` 1/200。一致率・outcome の分布・latency の分位は判断できない。
2. RSS の増加の原因が未確認である。compare の前の daemon の RSS の基準が無い。標本は `p1e-sampler`（operator の wrapper）が取り続けている。頭打ちになるかは次の集計で読む。
3. slow tick の前後比較は区間が短く、結論を出せない。03:02:14Z の `dispatch_ms` 6,568 の中身は未確認である。
4. report の table は記録が 0 件の領域の行を出さない（`--require-ready` の exit は正しく 4 になる）。今回の 03:01Z の実行で `cos_digest` の行が出なかった。

## 次の gate
- 人の判断 1: `lane_rules` の 1000 件を待つと約 107 日かかる。要件を改める判断は別 task 01M6J81DZ7C8SMY7HNZTW8GT0Z にある。その task の根拠には 9.33 件/日を使う。
- 人の判断 2: canary への昇格（別の承認。人の token `CELERIS_API_PRINCIPAL=human:web` だけが通る）。この task では行わない。手順は、report が exit 0 になった後に `celerisctl script --config ~/.config/celeris/config.toml mode lane_rules canary --expected-version <version> --generation <bundle> --canary-bps <bps> --mode-reason "<人の判断>"` を人の token で実行し、`domains` で canary を確かめ、report の `canary_selected` と `ts_side_clean_streak` を読む（guard の計数は mode が変わると 0 に戻る）。
- 再集計: 上の「実行したコマンド」を流す（読み取り専用。何度でも）。`cos_digest` は 2026-11-08 前後に 200 件に届く見込みである。report が exit 1（PROBLEM）になったら手順 7. で戻す。

## 提案
1. 手順 1. に「gate を変えた後の最初の release は current の release.sh で作られ、script-bundle/ が無い。preflight の `release_gate_has_script_steps` が fail なら空の commit を足して作り直す」と書く（operator の指摘、2026-11-04）。
2. 手順 4. の 1 行の sample は pid を固定で取るので、再起動で止まる。operator が使った「毎回 current の MainPID を引き直す wrapper」を `scripts/dev/` に置き、手順をそれに替える。
3. 件数の分母を増やすなら、WU の run の lane も compare の対象にできるかを検討する（`decide_lane_for_work_unit` は今 compare を呼ばない。WU の run は 2 週間で 1,185 件ある）。ただし WU は task の lane を上限にする別の判定なので、同じ入力にならない。設計の判断が要る。
4. 記録が 0 件の領域も `--require-ready` にあれば table に `no records` の行を出す。
5. daemon の RSS を compare の前から取る基準を、次の領域（`route_priority`）を上げる前に取る。

## commit（件名）
- p1e: lane_rules の本番 compare の preflight・集計・標本取りの読み取り専用 script と一時 DB の試験、手順の通し（前の task）
- p1e(2): 本番 lane_rules compare の初回の集計（1/1000 件）と raw を進捗に書く
- p1e(2): 集計の誤りを 2 件直し、lane_rules と cos_digest の集計と見込みの訂正を進捗に書く
