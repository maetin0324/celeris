---
title: P4-D Project の wake・判断・compare の記録の保持と GC
tasks: [01M6PAMC3S9QN740JWEQTXV60J]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P4-D Project の wake・判断・compare の記録の保持と GC

完了日: 2026-11-05。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の子（人の依頼 seq 2343）。基準は `git merge-base HEAD main`（P4-C の後）。
ADR は `agent-docs/adr/2026-11-05-phase4-p4d-retention.md`、運用は `docs/ops/project-controller.md` の「記録の保持と GC（P4-D）」。

## 結論
- `project_wakes`・`project_controller_decisions`・`script_compare_log` の行を、条件を満たすものだけ archive（JSONL＋sha256）に書いてから消す GC を実装した。events は触らない。
- 消さない行: 未消費の wake、`cron` 以外の wake、pending・applied・suppressed・failed の判断、案件の最後の判断と最初の判断、案件ごとの新しい 256 件、`pc/resume/%`・`pc/authority-required` の判断、TS が適用した compare の記録。判断と compare の記録は DB の trigger も同じ条件で DELETE を拒む（許可の行 `project_retention_permit` が同じ transaction に無ければ全部拒む）。
- 30 日・100 万行（694 案件）で wake 582,960 → 199,872 行、判断 582,960 → 235,960 行、compare 49,968 → 12,079 行。効果のある判断 83,280 行と最後の判断 694 行は全部残った。
- 既定は無効（`[project_controller.retention] enabled = false`）。本番の config・release・systemd は触っていない。

## 実装
- migration 0117（`crates/task-core/migrations/0117_project_retention.sql`）: `project_retention_permit`、追記専用の `project_retention_runs`、`project_controller_state(last_decision_id)` の部分索引、判断と compare の記録の削除の trigger の置き換え。0116（P4-C の予約、未使用）は `RESERVED_VERSIONS` に入れた。0115（P4-B）は main に取り込まれ、予約は 116 だけ。SCHEMA_VERSION 117。
- store: `crates/task-core/src/store/project_retention.rs`（`retention_gc`・`retention_stats`・`retention_project_counts`）、型は `crates/task-core/src/project_retention.rs`。
- dispatcher: `crates/task-dispatch/src/dispatcher/project_retention.rs`（`tick_project_retention`。`tick_project_controller` の後、`interval_secs` ごと）。`dispatcher.rs` の行は増やしていない。
- config: `crates/celeris/src/config/project_controller.rs` の `[project_controller.retention]`（`enabled` なら絶対 path の `archive_dir` が必須、0 は設定の誤り）。
- 可視化: `GET /api/v1/projects/{id}/controller` の `summary.retention`、画面 `web/features/projects/project-controller-panel.tsx` の `controller-retention`。

## 試験（提出時の HEAD で実行）
| 試験 | 確かめること |
|---|---|
| `task-core` `project_retention_gc_deletes_only_eligible_rows_and_archives_them` | 消す行・残す行の全種、archive の行数と sha256、2 回目は 0 件 |
| `project_retention_direct_delete_is_rejected_without_permit` | 許可なしの DELETE、許可があっても pending・applied・最後の判断・再開・許可待ち・TS 適用の行は trigger が拒む。GC の記録は UPDATE・DELETE できない |
| `project_retention_crash_after_archive_keeps_rows_and_rerun_completes` | archive の後・commit の前の crash で行・許可の行・記録が残らず、次の GC が完了する |
| `project_retention_concurrent_gc_and_controller_writes_keep_pending_rows` | 2 つの GC（別接続）と controller の書き込みを並行させ、各行を 1 回だけ消し、未消費の wake と pending の判断が残る |
| `project_retention_passes_at_the_same_time_never_overwrite_archives` | 同じ時刻の pass が archive を上書きしない（作業中に見つけた不具合の回帰） |
| `celeris` `project_controller_retention_config_requires_archive_dir_and_positive_values` | config の検証 |
| `task-dispatch` `project_retention_soak_gc_bounds_rows_without_changing_decisions` | 6 案件・仮想 72 時間・36 時間目に restart。GC あり・なしで判断の数・効果のある判断・未消費の wake・pending の効果が一致し、行は上限で頭打ち |
| `task-core` `project_retention_scale_30d_before_after_gc`（既定 20,000 行） | 30 日分で GC の前後の query・大きさ、archive の行数 = 消した行数 |
| web `project-controller-panel.test.tsx` の P2-J の試験 | `controller-retention` の表示 |

## 計測（raw は `agent-docs/progress/2026-11-05-phase4/p4d/`）
command（release build の test binary を直接起こす。`$BIN` は `cargo test --release -p task-core --test project_retention_scale --no-run` の出力）:
`RETENTION_SCALE_ROWS=<n> RETENTION_SCALE_OUT=<raw>.json $BIN --exact project_retention_scale_30d_before_after_gc`（exit 0、3 回とも）。
計測の規則は判断と compare の保持を 7 日にした（既定の 30 日では 30 日分の行に候補が無い）。wake は既定の 7 日。

| 項目 | 10 万（69 案件） | 100 万（694 案件）1 回目 | 100 万 2 回目 |
|---|---|---|---|
| wake 行（前 → 後） | 57,960 → 19,872 | 582,960 → 199,872 | 同じ |
| 判断 行 | 57,960 → 23,460 | 582,960 → 235,960 | 同じ |
| compare 行 | 4,968 → 1,202 | 49,968 → 12,079 | 同じ |
| DB file（GC の後は空き page） | 21.3 MB（空き 10.8 MB） | 199.8 MB（空き 107.9 MB） | 同じ |
| `VACUUM` の後（人の操作の見込み） | 9.0 MB・0.17 秒 | 76.6 MB・1.1 秒 | — |
| 1 評価の store の読み（wakes_pending・wakes_all・decisions 256・decisions 100000・last_input_at の和） | 4.5 → 2.1 ms | 3.1 → 1.9 ms | — |
| `project_controller_due`（tick ごと） | 2.9 → 0.9 ms | 7.7 → 9.0 ms（差なし。案件の数に比例） | — |
| GC の回数・1 回の中央値・最大 | 9 回・325 ms・385 ms | 78 回・1,067 ms・5,597 ms | 78 回・527 ms・1,224 ms |
| GC の合計・archive | 2.5 秒・26.7 MB | 97.3 秒・268.6 MB（767,977 行） | 41.6 秒 |

- 100 万の 2 回の差（97 秒と 42 秒）は同じ binary・同じ入力。計測中の host の load average は約 35（他の worker と共用）。原因は未確認（fsync の待ちと推定）。
- 索引の効果: `last_decision_id` の索引の無い版では 100 万の GC の 1 回の中央値が 1,282 ms・合計 96 秒（`scale-release-1000000-noindex.json`。この版は archive の file 名が重なり上書きしていたので archive の数は無効、時間だけ参照）。
- dispatcher の tick（debug build、`retention-soak.jsonl`、6 案件・72 時間・15 分刻み 288 tick）: GC なし p50 28.2 ms・p95 58.2 ms、GC あり p50 32.0 ms・p95 150.4 ms（毎時の GC の tick が p95 に入る）。行は wake 504 → 108、判断 432 → 84、判断の数 432 は両方同じ。
- 30 日の仮想時刻を dispatcher の tick で回す版（`PROJECT_RETENTION_SOAK_DAYS=30`、2 条件で約 12 分の見込み）は未実施。30 日は store の段（上の表）で測った。

## 失敗したこと・直したこと
- archive の file 名が `now`（同じ仮想時刻）と pass の中の連番だけで、同じ時刻の 2 回目の pass が前の file を上書きしていた（100 万の 1 回目の計測で archive が 4.2 MB しかないことで見つけた）。`create_new` で一意の名前にし、回帰の試験を足した。
- 候補の SELECT の「最後の判断か」の照合が `project_controller_state` を全行読み、案件の数の 2 乗で遅かった。0117 に部分索引を足した。
- worktree の target の v8 の build script の出力が古い seed の path を指し、`cargo nextest` が `rusty_v8` を見つけられなかった。`cargo clean -p v8`（この task の target だけ）で直った。製品 code の問題ではない。

## 修正: script_generation_e2e の切替をまたぐ digest pass（fix-digest-switch-race）
`crates/celeris/tests/script_generation_e2e.rs` の `stage_switch_to_b` が、`digest_run("run-b-compare")` の記録が A の generation（`mode_changed` の fallback、TS は `timeout`、elapsed 201,533 µs）になって `assert_eq!(record.generation, id_b)` で落ちた（前回の決定的検査）。

原因（確認済みと未確認を分けて書く）:
- 確認済み（記録の中身と `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` の読み）: 落ちた記録は、dispatcher の tick が切替（`human_mode(compare, B)`）の前に mode の行（`ts`・A）を読んで始めた pass のもの。`ts` の pass は TS の call を最大 200 ms（`COS_DIGEST_PASS_DEADLINE`）待ち、打ち切られると `reads.list` がその時点の store を走査する。試験は切替の後 200 ms の内に `run-b-compare` を入れたので、この pass がその run を拾い、`cos_digest_mode_changed` が切替を見つけて Rust の command を `mode_changed` で apply した。記録の generation は pass の始めの A。
- 確認済み（再現試験）: 同じ順序（`ts` の A で pass の call が返事をしない間に B へ切替 → run が終わる → 200 ms で打ち切り）を task-dispatch の試験 `cos_digest_pass_ts_switch_mid_pass_records_late_run_under_old_generation` で決定的に作り、切替の後に終わった run の記録が A・`mode_changed`・Rust の apply・`TsError` になり、digest message が 1 件だけのことを確かめた。偽の `PolicyScript` の hook（`stall_at_start`）が切替と run の終わりを call の中で行う。CPU 負荷は使わない。
- 未確認: pass が試験の待ち（A の in_flight == 1）の前後のどちらで始まったか、と TS の call が 200 ms 返らなかった理由（A の isolate が held pass の op 待ちの後ろで call を進めなかったと推定）。どちらでも、mode の行を切替の前に読んだ pass が切替をまたいだことは変わらない。
- 製品の挙動は仕様どおりと判断した: 切替をまたいだ pass は Rust で apply し `mode_changed` を記録する（ADR 2026-11-03-cos-digest-persist 2）。run の digest は欠けず二重にもならない。製品 code は変えていない。
- P4-D の GC の段（`dispatcher/project_retention.rs` の `tick_project_retention`）は、`[project_controller.retention]` が無いと最初の `Option` の判定で返る。この e2e の config にはその節が無いので GC は走らず、tick の時間を変えない（code の読みで確認。時間の計測はしていない）。競合は P4-D の前から起こり得た。

変更:
- `crates/celeris/tests/script_generation_e2e.rs`: `stage_switch_to_b` で、最初の記録が A なら A・`ts`・Rust の apply・`mode_changed` であることを確かめ、別の run（`run-b-compare-after`）でもう 1 回 digest を起こして B・`compare` の記録を確かめる。tick は 1 回に digest pass を 1 つだけ走らせ、次の tick は mode の行を読み直すので、切替をまたぐ pass は高々 1 つ。2 件目は必ず切替の後に始まった pass の記録になる。使われていない変数 `held` の束縛も外した。
- `crates/task-dispatch/src/dispatcher/tests/cos_digest_pass.rs`: 再現試験と、偽の script の `stall_at_start` を足した。
- P4-D の GC の実装と migration 0117 は変えていない。

コマンドと exit（提出時の HEAD で実行）:
- `cargo nextest run -p task-dispatch -E 'test(cos_digest_pass_)'`: exit 0、13 passed（再現試験を含む）。
- `cargo nextest run -p celeris --test script_generation_e2e`: exit 0、1 passed。続けて 3 回回して 3 回とも passed（9.8〜18.6 秒）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0、nextest 3,436 passed（10 skipped）、progress の hash 検査 0、script bundle の SKIP 0 件。

## commit（件名）
- `p4d: wake・判断・compare の記録の保持と GC（migration 0117、archive と許可の行、[project_controller.retention]、summary.retention）`
- `p4d: 進捗と計測の raw`
- `p4d: script_generation_e2e の切替をまたぐ digest pass の競合を直す`

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 7,707 passed（22 skipped）、web vitest 711 passed、tsc -b 0、test-ts-verify 20 passed、文書検査・progress の hash 検査 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 境界
- 本番の DB・config・release・systemd・shadow は触っていない。実 LLM は使っていない。24〜72 時間の実稼働はしていない（仮想時刻だけ）。
- milestones API（410）・human_required・許可の検査は変えていない。TS（V8）は判断のまま、GC は Rust の store と dispatcher だけ。
- events は消さない。判断と compare の記録の「追記専用」は「GC の許可つきで効果 `none`・TS 非適用の行だけ消せる」に変わった（ADR D4）。

## 本番への移行
- release が migration 0117 を当てる（表と trigger の置き換えだけ。行は動かさない）。戻すときは `docs/ops/project-controller.md` の「schema migration の戻し方」と同じ（`--restore-db`）。
- GC を使うなら人が `[project_controller.retention] enabled = true` と `archive_dir`（絶対 path）を書いて再起動する。初回は古い行が多く、表ごとに 1 時間 5,000 行ずつ消える。

## 未解決
- 本番の有効化と archive の置き場の容量（100 万行で約 270 MB）の決定は人。古い archive を消す仕組みは無い。
- `VACUUM` は人の操作（daemon を止める）。GC は file を縮めない。
- （解消）0115（P4-B）は main に取り込み済みで、`RESERVED_VERSIONS` は 116 だけ（integration-repair-1 で外した）。0115 は `project_wakes` を作り直すが、wake の表には trigger を置いていないので守りは変わらない。
- trigger は最大 id の守りを持たない（GC の DELETE 文の条件だけが守る）。手で打つ DELETE は許可の行が無いので trigger が拒む。
- `retention_stats` は表全体の `COUNT(*)` で 100 万行で約 0.1〜0.25 秒。API には出していない（案件ごとの数だけ summary に出す）。

## 次の gate
- 人: 本番で GC を有効にするか、`archive_dir` と保持の秒数を決める。
- release gate（全体の試験）は `scripts/selfdeploy/release.sh` で人が回す。

## 提案
- `project_controller_due` は案件ごとに `MIN(id)` を引くので案件の数に比例する（694 案件で約 8 ms）。案件が千を超えるなら 1 回の集計 query にする。
- archive を日ごとに 1 file にまとめ、古い file を圧縮する運用の道具。

## 配送の局所修復 repair-3（merge_base）
- 原因: main が「phase4 cos-usage」の 2 commit だけ進み、task branch が main の子孫でなくなった。
- 対処: main へ rebase（衝突なし）。コード変更なし。
- 検査（提出時の HEAD で実行）: `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace -- -D warnings` exit 0、`bash scripts/dev/test-affected.sh` exit 0（3440 passed、10 skipped）、`git merge-base --is-ancestor main HEAD` exit 0。

## 修正: GC が最大 id の行を消して compare の id が使い直される（fix-compare-id-reuse）
- 欠陥（reviewer の指摘、確認済み）: `script_compare_log` の id は AUTOINCREMENT の無い `INTEGER PRIMARY KEY`（0104）。GC が最大 id の行を消すと次の INSERT が同じ id を使い直し、script guard の `sweep`（`log_cursor` より大きい id だけを `script_compare_log_after` で読む）が新しい違反の行を見落として全体の停止が起きなかった。条件は GC が有効で、最新の compare の記録が `compare_keep_secs` より古く TS 適用でないとき。
- 直し: `crates/task-core/src/store/project_retention.rs` の 3 表の条件（`WAKE_GUARD`・`DECISION_GUARD`・`COMPARE_GUARD`）に「表の最大 id の行ではない」（`id < (SELECT MAX(id) FROM <表>)`）を足した。候補の SELECT と DELETE の付け直しの両方が同じ定数を使う。`retention_stats` の `wakes_consumed_cron` は同じ条件で数え、`compare_log_gc_eligible`（消せる compare の行）を足した。`compare_log_ts_applied` は TS 適用の条件だけで数える（前は「GC の条件の否定」で数えていた）。
- 試験:
  - `project_retention_keeps_max_id_rows_so_compare_ids_are_not_reused`（task-core）: 全行が保持の期間より古く TS 適用でない状態で GC し、3 表とも最大 id の行が残ること、次の compare の id が GC の前の最大 id ＋1 になること、判断では最初・新しい窓のどちらでもない最大 id の行が残ることを確かめる。
  - `script_guard_sweep_sees_violation_appended_after_retention_gc`（celeris、`script_guard_stop_tests.rs`）: cursor を GC の前の最大 id に置いた見回りが、GC の後に入った違反の行を拾って全体を止めることを確かめる。`COMPARE_GUARD` の最大 id の条件を一時的に外すと、この試験は `halted after gc` で落ちた（exit 100、見回りが止めない）。条件を戻して passed。
- 文書: ADR に D9（付記 2026-11-06）、`docs/ops/project-controller.md` の「記録の保持と GC（P4-D）」に同じ扱いを書いた。ADR D8 と本進捗の 0115・RESERVED の古い記述を現状（予約は 116 だけ）に直した。
- コマンドと exit（提出時の HEAD で実行）:
  - `cargo nextest run -p task-core project_retention_`: exit 0、7 passed。
  - `cargo nextest run -p celeris script_guard_sweep_`: exit 0、2 passed。
