---
title: TS 制御層の本番運用準備と意図的な TS 方針改善（段階移行 2）— 親 task の分割
tasks: [01M6HG7T6ZKTX239KGH0QY4HFQ]
status: done
completed: 2026-11-04
updated: 2026-11-04
---
# TS 制御層の本番運用準備と意図的な TS 方針改善（段階移行 2）

親 task 01M6HG7T6ZKTX239KGH0QY4HFQ。前回親 01M6DCMB75G5F0DSE58ZNZMBAK（S0〜S6 done）の続き。この記録は親 task の分割・依存・停止条件を書く。実装・試験・計測は子 task が行い、子の記録は `agent-docs/progress/2026-11-03-ts-control-ops/<key>.md` に置く。

## 結論
- 親 task はこの task（01M6HG7T6ZKTX239KGH0QY4HFQ）を使う。`celerisctl ls` で未完の重複 task が無いことを確かめた（TS 制御層の未完 task はこの 1 件だけ）。
- 子 task を 8 件に分けた（下の表）。最初に並列で走るのは p0a-release-gate・p0b-cos-digest-persist・p1c-ops-observe の 3 件。
- この run（親）は分割と記録だけを行い、crates/・scripts/ を変えていない。本番・release・mode・guard・IEEE Access/K2/K4 の環境に触れていない。

## 精査した現行実装と未解決（最新 main = `git merge-base HEAD main`）
| 依頼 | 現行の file・関数 | 未解決（出所） |
|---|---|---|
| P0-A | `scripts/selfdeploy/release.sh`（control-ts の bundle を作らない。`CELERIS_SCRIPT_REQUIRE_BUNDLE` を設定しない）、`crates/celeris/tests/support/script_compare.rs`（bundle を `$TMPDIR` に作る、付記 S5-9/S5-13）、`scripts/dev/ts-verify.sh` | S3 未解決 10・S4 未解決 8・S4 の node_modules/.bin shim 書き換え（KB `control-ts-bundle-node-bin-side-effect`）。S5 を含む release の gate 秒は未計測（S6 未解決 4） |
| P0-B | `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` の `cos_digest_pass`・`apply_digest_pass`・`append_pass_record`（apply の後に別 transaction で `script_compare_log_append_observed`、失敗は警告だけ） | S6 未解決 5（apply 済み・記録なしの crash 窓、既知の制約 11） |
| P1-C | `crates/celerisctl`（script mode は rust・compare だけ、canary は curl）、`crates/celeris/src/daemon/script_guard.rs`（`script_compare_log` を毎回全件読む） | S5 未解決 6・提案 4、S6 既知の制約 2、RSS の退役繰り返しは未確認（S6「RSS・isolate 数」） |
| P1-D | 境界 ADR (f)（不一致は自動 rollback）、`scripts/dev/ts-pilot.sh`（200 文字打ち切りの pilot、差分 9 件は意図した変更） | 意図した差分を説明する仕組みが無い |
| P1-E | S5 の「本番への移行」1.〜10.、S6「決定一致率」の数え方 | 本番の一致率は未計測、本番は全領域 rust（S6 未解決 1・2） |

## 子 task と依存
| key | 内容 | 依存 | 止まる gate |
|---|---|---|---|
| p0a-release-gate | release/verify gate に bundle 作成・host API 適合・ESM/manifest hash 検証・`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` の実 bundle 試験を必須化、shim 書き換えの防止、cold/warm の時間計測 | なし | release の実行は人。gate の実測は手元の release.sh の dry 相当で行う |
| p0b-cos-digest-persist | apply と compare_log・guard 計数の原子化（同一 transaction か冪等 outbox）、crash 前/中/後・再起動・重複配送・CAS 競合・rollback の試験 | なし | 原子化できない場合は理由を ADR に書き cos_digest の canary/ts を API で拒否 |
| p1c-ops-observe | celerisctl script の generations list/register/retire・canary bps・guard clear（human のみ）、未配備 hash の扱い、script_tick_wait の log/metric、compare-log の cursor 走査、RSS 推移、route_priority canary 上書きの試験 | なし（p0a と並列） | guard 解除・昇格の権限を operator/CoS に広げない |
| p1d-intended-change | parity と behavior change の分離、版付き改善仕様・expected diff fixture・人の承認・golden 更新規約（ADR と実装）、200 文字 pilot の再評価 e2e | p0b | 未承認差分・安全違反・無効評価は fail-closed のまま |
| p1e-shadow-preflight | lane_rules の本番 compare の preflight・人の手順・集計 script（本番は変えない） | p0a、p1c | 本番の操作は人 |
| p1e-shadow-measure | 人の承認と実施の後、本番 compare の実件数（純粋判定 >=1000、CoS >=200）の読み取り専用の集計と報告 | p1e-shadow-preflight | 人の承認が無ければ question で止まる。canary→ts は別の人の承認 |
| p2f-expand-port | 次の移植領域の優先順の決定と 1 領域の同等移植 | p1e-shadow-measure | 本番 compare の観測が無ければ着手しない |
| p2g-llm-self-improve | 実 LLM の CoS/worker による TS 改善の非本番実験 | p1d-intended-change | 本番の承認は自律化しない。K2/K4 評価器・hidden 資料に触れない |

## 本番切替に要る人の判断
分割時（2026-11-03）の記録。1. は人の判断で実施済み（`lane_rules`・`cos_digest` が compare）。現状と残りの判断は下の「本番の現状」「次に要る人の判断」。

1. p1e-shadow-measure の前: lane_rules の generation 登録と mode `compare` への切替を本番で行うか（人が S5「本番への移行」4.・5. を実行する）。
2. compare の実件数が揃った後: canary（bps）→ ts へ上げるか（領域ごとに別の承認）。
3. p0a の gate を含む release を作り promote するか。
4. p0b が原子化できず canary/ts を禁止した場合、その禁止を受け入れるか。

## 子 task の結果（2026-11-04、親の再開 run で集約）
完了日（親の集約）: 2026-11-04。子 8 件はすべて done で、各子の記録の front matter は `status: done`。8 件の branch は全部 main の祖先（`git merge-base --is-ancestor celeris/<子 id> main` が 8 件とも 0）。この branch は main を merge した。

| key（task id） | 状態 | 主な file・関数 | 試験（子の記録） | raw records | 記録 |
|---|---|---|---|---|---|
| p0a-release-gate（01M6HGFR1T4M08TJM7KD39D443） | done | `scripts/selfdeploy/script-bundle-gate.sh`（release.sh の 4 段 control-ts-install・control-ts-bundle・script-bundle-verify・script-bundle-tests）、verify.sh の照合、`control-ts/pnpm-workspace.yaml` の `verifyDepsBeforeRun: error` | `scripts/dev/test-release-script-gate.sh`（11 通りの失敗＋通る場合）、test-affected exit 0 | `p0a-script-gate-cold.jsonl`・`p0a-script-gate-warm.jsonl`（足した秒 cold p50 29.350 秒・warm p50 26.156 秒） | [p0a-release-gate.md](2026-11-03-ts-control-ops/p0a-release-gate.md) |
| p0b-cos-digest-persist（01M6HGFR1TDAP02XY98ES1ZDA1） | done | `SqliteStore::cos_digest_apply_pass`（1 transaction・run ごとの SAVEPOINT）、outbox（migration 0107）と `cos_digest_shadow_finish`、ADR 2026-11-03-cos-digest-persist | `cos_digest_persist_` 8 件（crash 8 段・実 process abort・重複配送・CAS 競合・guard の戻し）で message 重複 0・記録欠落 0、test-affected 7200 passed | `p0b-apply-bench.jsonl`・`p0b-cos-ts.jsonl` | [p0b-cos-digest-persist.md](2026-11-03-ts-control-ops/p0b-cos-digest-persist.md) |
| p1c-ops-observe（01M6HGFR1TR6PJ8822DHW7NNNT） | done | `celerisctl script`（generations list/register/retire・canary bps・guard clear）、409 `script_bundle_not_deployed`、GET /daemon の `script_tick_wait`、guard の増分走査、`cos_read_only` | test-affected 5115 passed、guard clear の help に 403/422 | `p1c-ops-cycles.jsonl` | [p1c-ops-observe.md](2026-11-03-ts-control-ops/p1c-ops-observe.md) |
| p1d-intended-change（01M6HGFR1TD2G3H4JF6J67CTBM） | done | `control-ts/behavior-changes/`、`CompareOutcome::Explained`、migration 0108、`/api/v1/script/behavior-changes`（承認は human のみ）、ADR 2026-11-03-ts-intended-behavior-change、`scripts/dev/ts-behavior-eval.sh` | test-affected 7223 passed、canary e2e 4 件 | `p1d-eval/records.jsonl` | [p1d-intended-change.md](2026-11-03-ts-control-ops/p1d-intended-change.md) |
| p1e-shadow-preflight（01M6HGFR1TMY7PTGQ553FQGM7H） | done | `scripts/dev/script-compare.py`（preflight/report/sample、読み取り専用） | `script_compare_report` 8 passed、`tests/script-compare.sh` 35 項目、test-affected 1009 passed | `p1e-preflight-prod.json`（本番 preflight は exit 3）・`p1e-rehearsal.jsonl` | [p1e-shadow-preflight.md](2026-11-03-ts-control-ops/p1e-shadow-preflight.md) |
| p1e-shadow-measure（01M6HGFR1TVY299MQPZY7HQQG9） | done | 本番 DB を読み取り専用（`sqlite3 -readonly`）で `script-compare.py report` により集計。`script-compare.py` の `summarize_tick_log`（journal の `fields.message` だけを見る）と `estimate_volume`（WU の run を除く）を修正 | report exit 4（件数不足）。`lane_rules` 1/1000 件・`cos_digest` 1/200 件、どちらも match、他の outcome・安全違反・guard の停止・rust への戻しは 0。synthetic で補っていない。`tests/script-compare.sh` exit 0、test-affected exit 0 | `p1e-measure-report.jsonl`・`p1e-measure-samples.jsonl` | [p1e-shadow-measure.md](2026-11-03-ts-control-ops/p1e-shadow-measure.md) |
| p2f-expand-port（01M6HGFR1TCHVSDDNH0GR9SNBK） | done | 候補 5 件を順位づけし、1 位の Complexity Gate の得点（`execution_gate::decide_at` の規則表）を領域 `execution_gate` として同等移植（`crates/task-core/src/execution_gate/scorer.rs`、`control-ts/src/domains/execution_gate.ts`、migration 0109、ADR 2026-11-04-ts-execution-gate-domain）。本番の mode は `rust` のまま | fixture 2,337 件が Rust と TS で全件一致、本物の V8 の 4,674 呼び出しも全件一致。`script_compare_gate_` 20 passed・6 passed、test-affected 7273 passed | `p2f-rank.jsonl`・`p2f-gate-ts.jsonl`（gate-ts warm p50 8.267 秒・p95 8.618 秒、cargo 0 回）・`p2f-gate-host-latency.jsonl` | [p2f-expand-port.md](2026-11-03-ts-control-ops/p2f-expand-port.md) |
| p2g-llm-self-improve（01M6HGFR1TVSBS63STM8NH3XCS） | done | `trial.sh`・`collect.py`、課題 T1/T2 × TS/旧 Rust の子 4 件 | 4 試行 成功 4/4、人の介入 0、再採点 4 件 exit 0 | `p2g-llm-self-improve/`（rescore/trial.jsonl・usage.jsonl） | [p2g-llm-self-improve.md](2026-11-03-ts-control-ops/p2g-llm-self-improve.md) |

### 本番の現状（p1e-shadow-measure.md の 2026-11-04T03:06:11Z の集計）
- 本番の `lane_rules`（02:41:21Z）と `cos_digest`（03:00:10Z）は mode `compare`・version 1 で、どちらも operator（人の判断）が rust から compare にした。`route_priority` は `rust`。`execution_gate` は本番に入っていない（migration 0109 を含む release は未作成）。
- 記録は `lane_rules` 1/1000 件・`cos_digest` 1/200 件で、どちらも match。report は exit 4（件数不足）で、canary に進む根拠はまだ無い。synthetic で補っていない。
- 件数の見込み: `lane_rules` は 9.33 件/日で 1000 件まで約 107 日（2027-02 中旬）。100 件なら約 10.7 日（要件の見直しは別 task 01M6J81DZ7C8SMY7HNZTW8GT0Z）。`cos_digest` は 55.31 件/日で 200 件まで約 3.6 日。
- daemon の RSS は起動直後の 181,820 kB から 26 分で 588,668 kB に増えた。原因は未確認。

### 次に要る人の判断（いずれも未実施）
1. 本番で `execution_gate` を compare にするか（p2f-expand-port.md「本番への移行」。migration 0109 を含む release の作成・promote、bundle の登録、mode の変更）。
2. `lane_rules`・`cos_digest` の canary→ts への昇格。件数が 1000・200 に達した後で、領域ごとに別の承認（人の token だけが通る）。
3. release の作成・promote。
4. guard の解除。

### TS-only の高速経路の回帰確認（統合後の HEAD で実測）
`bash scripts/dev/edit-activate-baseline.sh --scenario <policy-ts|cos-ts> --mode warm --reps 20 --series parent-integrated --out <file>` を main 取り込み後の木（clean）で実行し、`--check-targets` で照合した。

| scenario | 試行 | p50（秒） | p95（秒） | cargo 0 回 | S6 基準 p50 | raw |
|---|---|---|---|---|---|---|
| policy-ts | 20 | 7.487 | 7.699 | 20/20 | 7.47〜7.52 | [parent-policy-ts.jsonl](2026-11-03-ts-control-ops/parent-policy-ts.jsonl) |
| cos-ts | 20 | 7.569 | 7.678 | 20/20 | 7.47〜7.52 | [parent-cos-ts.jsonl](2026-11-03-ts-control-ops/parent-cos-ts.jsonl) |

目標（p50≤30 秒・p95≤60 秒）の内で、S6 基準から +0.05 秒以内。重い検査は release gate（p0a の 4 段）に置き、`ts-verify.sh` の高速経路は変えていない。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（main 取り込み後の差分は進捗と raw だけで「試験対象の変更なし」、文書検査 ok）。子の実装の試験は各子の記録の数値（上の表）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `--check-targets` → policy-ts・cos-ts とも exit 0（problems 0）。

## 未解決
- 本番の compare の件数は `lane_rules` 1/1000・`cos_digest` 1/200 で、一致率は判断できない（n=1）。`execution_gate` は本番の記録 0 件。
- 本番 daemon の RSS の増加（182→589 MB、26 分）の原因は未確認。compare の前の基準が無い。
- `execution_gate` の `tree::gate::unit_gate` は比較の対象外で、canary・ts でも Rust の規則表で判定する。TS だけで規則を変える前に、計画の検証と dispatch の判定のずれを設計で決める。
- 本番で execution_gate を compare にすること、canary→ts の昇格、release の作成・promote、guard の解除は、いずれも未実施（上の「次に要る人の判断」）。
- p0a: cargo-test 段の秒の増分は未計測（次の release の gate.json で確かめる）。
- p0b: 本番に旧い crash 窓の行が残っているかは未確認。
- p1c: 本番の script_tick_wait と RSS は未確認。
- 各子の記録の未解決節を参照。

## 経緯（親の review の保留）
- 子の完了前に親の review が 2 回走り条件 0・1 で不合格になった。人の回答で子の完了まで保留し、2026-11-04 に「終わったので続けてください」の回答で再開した。

## 提案
なし。
