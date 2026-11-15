---
title: "健全性v2 R6 survey: TS 二重 policy の終了条件・評価基盤の分離・B0 の後の未使用候補"
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R6 survey

完了日: 2026-11-15

コードは変えていない。機械向けの結果は同じ dir の `survey.json`（鍵 `ts_domains`・`eval_split`・`new_candidates`）。
本番の値は読み取り専用で読んだ（書き込みはしていない）。

## 結論
- TS 制御層の 3 領域（`lane_rules`・`route_priority`・`cos_digest`）は、ADR 2026-11-02-ts-control-layer-boundary の S6-3 の条件 1〜5 をどれも満たさない。3 領域とも Rust 参照実装を残す。
  Rust の再検証・安全条件・script-guard・generation の登録/切替/drain は S6-3 の「消さないもの」なので対象外。
- campaign・routing-eval・jev-classifier・`crates/script-runtime/vendor` は 4 件とも repo の外へ出さない（keep）。
  分離しても再現性は良くならず、依存の負荷の主因（campaign の test target 111 個）も減らない。外に出した行数は削減に数えない。
- B0 の後に増えた候補は 2 件で、2 件とも ambiguous（消さずに人に示す）。B0 の後に足された config 欄 4 件はどれも読まれていて、使われない欄は 0 件。

## (1) TS 3 領域と S6-3 の条件
本番の値は `python3 scripts/dev/script-compare.py report --db $CELERIS_RUN_DB`（sqlite3 -readonly・`PRAGMA query_only=ON`、
bounds `upto_event_id` 937415・`upto_log_id` 571）と `/local/celeris/state/script-bundles/` の一覧で読んだ。

| 条件 | lane_rules | route_priority | cos_digest |
|---|---|---|---|
| 本番の mode | compare（2026-11-04T02:41:21Z から） | rust（generation 無し） | compare（2026-11-04T03:00:10Z から） |
| 1 本番で ts 30 日・件数 | 未達: ts の日数 0、記録 124 件（要件は純粋判定 1 万件） | 未達: 記録 0 件 | 未達: ts の日数 0、記録 571 件（要件は CoS 経路 2,000 件） |
| 2 golden の凍結 | 未達 | 未達 | 未達 |
| 3 fail-closed の既定 | 未達 | 未達 | 未達 |
| 4 戻し先 2 世代 | 未達（bundle 1 個） | 未達 | 未達 |
| 5 golden 更新の手順 | 未達 | 未達 | 未達 |
| 判定 | Rust 参照を残す | Rust 参照を残す | Rust 参照を残す |

- 条件 1 の内訳: `lane_rules` は match 110・ts_error 14（全て timeout）、`cos_digest` は match 563・ts_error 8（timeout 7・failure 1）。
  2 領域とも mismatch・安全違反・隠れた差分・自動の戻しは 0 件。`lane_rules` の全入力の網羅（39,366 件、2026-11-04）は全件一致だが、S6-3 の 1 の件数には数えない（境界 ADR の 2026-11-04 付記）。
- 条件 2: fixture は commit 済みだが、task-core の example（`dump-lane-fixtures`・`dump-route-priority-fixtures`・`dump-cos-digest-fixtures`）が Rust 参照の出力から作り、
  `crates/celeris/tests/script_compare_{lane_rules,route_priority,cos_digest}.rs` が各行を Rust 参照と照合する。runner は file の期待値で採点するが、生成と照合が Rust に依存する。
- 条件 3: 境界 ADR に領域ごとの fail-closed の既定の決定が無い（S6-3 の 3 の本文に例示があるだけ）。試験も無い。
- 条件 4: bundle は 1 個（sha256 は `survey.json` と report の出力の `bundle_sha256`）、`script_generations` は `01M6J6X3HE4Z3MXM11GHJ1B6M4`（active）の 1 行。
- 条件 5: ADR 2026-11-03-ts-intended-behavior-change D3 は「旧挙動の fixture は Rust 参照の出力のまま」とする二重実装の間の手順で、凍結 golden を人が review して更新する手順ではない。

## (2) 評価基盤の分離
件数は `cargo tree --offline -e normal`（と `-e normal,dev,build`）の unique 行、test-affected の頻度は `cargo metadata` の dev を含む逆依存閉包（test-affected.sh と同じ規則）を
2026-10-15 以後の非 merge commit 2,569 件（crate を触ったもの 1,143 件）に当てて数えた。重い build はしていない。

| 対象 | 判定 | normal 依存 | 逆依存 | test-affected に入る割合 | 主な根拠 |
|---|---|---|---|---|---|
| campaign | keep | 235 | celeris・task-dispatch の dev | 91%（1,036 件）、test target 111 | K2/K4 の凍結 manifest は Celeris の commit を pin しない。1 commit で評価器・fixture・Celeris がそろうことが再現の前提。内部 API を使うので分けると保守が 2 repo に割れる |
| routing-eval | keep | 174 | campaign（normal）・celeris（dev） | 25%（287 件） | K2 の入力 manifest の形式（routing-eval-manifest/1）の持ち主 |
| jev-classifier | keep | 148 | celeris（normal、本番） | 0.3%（4 件） | 評価基盤ではなく本番の要求分類 adapter |
| script-runtime vendor | keep | （member でない） | celeris-script 経由で本番 | vendor 自体は試験対象外 | 外に出すと git source の [patch] になり、ADR 2026-11-02-deno-core-vendored-serde-json-features が却下した形になる |

- IEEE Access の評価の code は repo に無い（ADR の記述だけ）。測定用 release・state dir・pin した binary で再現するので、分離と関係しない。
- campaign の負荷の対策は repo の分離ではなく test target の束ね（R6 で扱わないと決めた「celeris-campaign の 111 target」。close が提案に書く）。

## (3) B0 の後に増えた候補
`python3 agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-scan.py` を提出時の HEAD で走らせ、B0 の `dup-candidates-scan.json` と比べた。
未参照の pub は 36 件 → 23 件（新規 1 件）、未参照の script は 6 件 → 35 件（新規 35 件のうち 34 件は誤検出）。

| 候補 | files | 利用の根拠 | 分類 |
|---|---|---|---|
| `set_knowledge_probe` | `crates/task-dispatch/src/dispatcher.rs:1182`（と `probe_ports.rs:69`・`routing_runtime.rs:451` の呼び先） | 定義の行の外に呼び手が無い。B0 の後に呼び手の試験が消えた。ADR-0052 D1 の到達性の検査の差し替え口で、今その検査を差し替える試験が無い | ambiguous |
| `scripts/tests/test_architecture_map_diffs.py` | 同 | check-health.sh（pattern `test_architecture_map.py`・`test_repo_health*.py`）・repo-health の profile・test-affected のどれも走らせない | ambiguous |

- 誤検出: `scripts/dev/repo-health/health/gitio.py`（module として import）、`schema/examples`・`bad-examples` の 31 file（`test_repo_health_contract.py` が glob で読む）、
  `test_repo_health_archmap.py`・`test_repo_health_structure.py`（check-health の discover で走る）。
- config 欄: B0 の base から足された `fallback_after_secs`・`forecast_disk_ratio`・`min_free_disk_pct`・`queued_wake_secs` はどれも config の外で読まれる。使われない欄は 0 件。
- B0 からある未参照の pub 22 件（19 件は B0 で delete と判定）は B0 の判定と R6 の prune 段の WU に任せ、ここでは数えない。

## 証拠（提出時の HEAD で実行）
- `python3 scripts/dev/script-compare.py report --db $CELERIS_RUN_DB` → exit 0、problems 0（出力は run の成果物 dir の `script-compare-report.json`）
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/dup-candidates-scan.py` → exit 0（出力は run の成果物 dir の `dup-candidates-scan-head.json`）
- `cargo tree -p <crate> --offline -e normal --prefix none | sort -u | wc -l` → 235・174・148・198（celeris-script）
- `git diff --stat $CELERIS_WU_BASE -- crates web config control-ts scripts` → 差分 0
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」、check-health は verdict=pass・new 0・worsened 0）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0、`sh scripts/dev/progress-index.sh --check` → ok

## 未解決事項
- `set_knowledge_probe` と `test_architecture_map_diffs.py` の扱いは人の判断を待つ（消さずに残している）。
- `lane_rules` の compare の ts_error 14 件は全て timeout で、網羅の証拠の条件（timeout 0）を満たさず canary に進めない。S6-3 の 1 より前の (f) の段で止まっている。

## 提案
- `test_architecture_map_diffs.py` は check-health.sh の pattern に足して走らせる案を推す（検査器の変更なので別の review の対象）。
- S6-3 の 2・3・5 は本番の観測（条件 1）を待たずに repo で進められる（4 は本番の state dir の作業）。進めるなら領域ごとの fail-closed の既定（条件 3）の ADR の付記から始める。
