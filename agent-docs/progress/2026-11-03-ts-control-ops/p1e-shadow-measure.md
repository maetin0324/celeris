---
title: P1-E(2) lane_rules の本番 compare の実測の集計
tasks: [01M6HGFR1TVY299MQPZY7HQQG9]
status: blocked
updated: 2026-11-04
---
# P1-E(2) lane_rules の本番 compare の実測の集計

task 01M6HGFR1TVY299MQPZY7HQQG9（親 01M6HG7T6ZKTX239KGH0QY4HFQ の p1e-shadow-measure）。手順は [p1e-shadow-preflight.md](p1e-shadow-preflight.md) の「本番への移行（人の手順）」。基準は `git merge-base HEAD main`。

## 結論
- 人の承認と手順 0.〜5. の実施を、task のコメント・回答（2026-11-04T02:41Z、operator session が人の指示で実施）で確かめた。release は手順 1. で作って promote した current（script-bundle 入り）、generation 01M6J6X3HE4Z3MXM11GHJ1B6M4、`lane_rules` を compare にしたのは 2026-11-04T02:41:21Z。
- 手順 6. の集計を 2026-11-04T02:43Z に読み取り専用で実行した。**純粋判定は 1 件で、1000 件に届いていない**（report exit 4 = 矛盾は無いが件数不足）。synthetic で補っていない。task の指示どおり、件数を書いてここで止まる。
- 1 件の中身: match 1、mismatch・explained・ts_error・invalid_output・unavailable は 0、一致率 1.0、latency 431 µs（n=1）、安全違反 0、guard の停止 0、shadow outbox の残り 0。
- 見込み: 48.45 件/日（P1-E(1) の推定）で 1000 件まで約 20.6 日、2026-11-25 前後。負荷が変われば変わる。
- worker は本番の DB を `sqlite3 -readonly`（`PRAGMA query_only=ON`）で読み、journal と `~/p1e-samples.jsonl` を読んだだけで、何も書いていない。release・promote・mode・guard・config・systemd・K2/K4/IEEE Access の環境は変えていない。canary・ts への昇格はしていない。

## 実測（2026-11-04T02:43Z、events の上限 id 782871〜782878）
| 項目 | 値 |
|---|---|
| 記録（lane_rules） | 1（first/last 2026-11-04T02:41:58Z。この task の dispatch） |
| outcome | match 1、ほか 0 |
| 一致率 | 1.0（n=1） |
| clean_streak / 要件 | 1 / 1000（ready=false） |
| latency p50/p95/max | 431 / 431 / 431 µs |
| RSS（標本 1 件、pid 1659338） | 181,820 kB、増分 0（標本が 1 件なので増減は未確認） |
| threads | 29 |
| script_tick_wait | p50/p95/max 0 µs、tick_budget 0（compare は TS を待たないので 0 が正しい） |
| tick log（02:41:21Z 以後 5 行） | slow tick 0、tick budget 警告 0 |
| rollback | mode 変更 1（rust→compare、operator:claude-code）、rust への戻し 0、自動の戻し 0、generation の event 0、受信箱 script_control 0 |
| 領域 | lane_rules=compare（version 1）、cos_digest・route_priority=rust |

raw:
- [p1e-measure-report.jsonl](p1e-measure-report.jsonl) — `report --format json` の全出力（1 行、`_run` に時刻と exit）
- [p1e-measure-samples.jsonl](p1e-measure-samples.jsonl) — `~/p1e-samples.jsonl` の写し（標本 1 件、02:40:43Z）

## 実行したコマンドと exit（提出時の HEAD で実行）
- `journalctl --user -u celeris@<current の release> --since "2026-11-04 02:41:21" --no-pager > $TMPDIR/p1e/daemon.log` → exit 0、5 行
- `python3 -I scripts/dev/script-compare.py report --db /local/celeris/data/db/celeris.sqlite3 --samples ~/p1e-samples.jsonl --tick-log $TMPDIR/p1e/daemon.log --require-ready lane_rules --format json` → exit 4（1.5 秒）
- 同じく `--format table` → exit 4、`not ready: clean_streak 1 < 1000`、PROBLEM 行なし
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → 下の「検査」

## 使った file・関数（変更なし）
- `scripts/dev/script-compare.py` の `cmd_report`（`read_snapshot`・`aggregate`・`finish_group`・`rollback_summary`・`summarize_samples`・`summarize_tick_log`）。DB は `query` が `sqlite3 -readonly -cmd "PRAGMA query_only=ON"` で読む。
- コードは変えていない。この task の変更は進捗ファイルと raw の 2 file だけである。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0（docs のみの変更で試験対象なし。source-size gate 0 failure、check-doc-links・check-adr-numbers・check-human-text ok）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決
1. 純粋判定が 1/1000。一致率・outcome の分布・latency の分位・RSS の増減・tick への影響は、件数・標本が足りず判断できない。1000 件に届いたら（report が exit 0）、この task を再開して同じ 6. を流す。
2. 標本取りは operator の wrapper（`/local/celeris/data/scratch/p1e/sampler.sh`、user unit `p1e-sampler`）が動かしている。worker の sandbox からは user bus に繋がらず、unit が動いているかは未確認（`~/p1e-samples.jsonl` は 02:40:43Z の 1 件だけで、次は 10 分後の予定）。
3. compare の前の tick log と比べる基準（slow tick の頻度）はまだ取っていない。次の集計で compare の前の同じ長さの journal を取って比べる。
4. `cos_digest` は compare にしていない（200 件の条件は人が決める）。

## 次の gate
- 待ち: `lane_rules` の clean_streak が 1000 に届くこと（見込み 2026-11-25 前後）。report が exit 1（PROBLEM）になったら手順 7. で戻す。
- 人の判断: canary への昇格（別の承認。人の token `CELERIS_API_PRINCIPAL=human:web` だけが通る）。この task では行わない。手順は `celerisctl script --config ~/.config/celeris/config.toml mode lane_rules canary --expected-version <version> --generation <bundle> --canary-bps <bps> --mode-reason "<人の判断>"` を人の token で実行し、`domains` で canary を確かめ、report の `canary_selected` と `ts_side_clean_streak` を読む（guard の計数は mode が変わると 0 に戻る）。

## 提案
1. 手順 1. に「gate を変えた後の最初の release は current の release.sh で作られ、script-bundle/ が無い。preflight の `release_gate_has_script_steps` が fail なら空の commit を足して作り直す」と書く（operator の指摘、2026-11-04）。
2. 手順 4. の 1 行の sample は pid を固定で取るので、再起動で止まる。operator が使った「毎回 current の MainPID を引き直す wrapper」を `scripts/dev/` に置き、手順をそれに替える。
