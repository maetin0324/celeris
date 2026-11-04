---
title: P2-F — 次の TS 移植領域の順位づけと execution_gate の同等移植
tasks: [01M6HGFR1TCHVSDDNH0GR9SNBK]
status: done
completed: 2026-11-04
updated: 2026-11-04
---
# P2-F — 次の TS 移植領域の順位づけと `execution_gate` の同等移植

task 01M6HGFR1TCHVSDDNH0GR9SNBK（親 01M6HG7T6ZKTX239KGH0QY4HFQ の p2f）。基準は `git merge-base HEAD main`。設計は [ADR 2026-11-04-ts-execution-gate-domain](../../adr/2026-11-04-ts-execution-gate-domain.md)。

## 結論
- 着手の前提を確かめた。本番の compare 観測は 2 領域にある（[p1e-shadow-measure.md](p1e-shadow-measure.md): `lane_rules` 1 件・`cos_digest` 1 件、どちらも match）。ただし件数は要件（1000・200）に遠く、canary の根拠にはならない。
- 候補 5 件を順位づけし、1 位の **Complexity Gate の得点（`execution_gate::decide_at` の規則表）** を領域 `execution_gate` として同等移植した。
- 比較 fixture 2,337 件は Rust と TS で全件一致した。本物の deno_core host でも正順・逆順の 4,674 呼び出しで全件一致した。
- TS だけの変更の edit→activate は warm 20 試行で p50 8.267 秒・p95 8.618 秒、cargo 0 回、link 0 回で、目標（p50 30 秒・p95 60 秒）の内にある。S6 の基準 7.47〜7.52 秒より約 0.75 秒遅い（下の「性能」）。
- 本番では動かしていない。この領域の mode は migration 0109 で `rust` のまま入り、本番で compare にするのは人の判断である。

## 順位づけ
raw は [p2f-rank.jsonl](p2f-rank.jsonl)（2026-09-01 以後の git log と `cargo metadata`）。

| 順位 | 候補 | commit 数 / +− 行 | 再コンパイルの範囲（自分を含む crate） | 安全境界 | CoS の価値 |
|---:|---|---|---:|---|---|
| 1 | `execution_gate::decide_at` の規則表（`crates/task-core/src/execution_gate.rs`） | 9 / +1058 −576 | 14（`task-core`） | 純粋関数で、対象外・人の明示・閾値を Rust に残して切り出せる | 中。直行経路か計画かを決め、CoS と worker の手戻りに効く |
| 2 | 受信箱の分類 `is_noise` 等（`dispatcher/cos_chat/triage.rs`） | 20 / +901 −191（file 全体。分類以外を含む） | 5（`task-dispatch`） | cursor・挿入 transaction に近く、入力（`EventRow`・`InboxItem`・`Notice`）の直列化が大きい | 高 |
| 3 | 定期見直しの入力文 `review_text`（`dispatcher/cos_chat/review.rs`） | 3 / +323 −27 | 5 | 先に store 読み出しを外へ出す Rust の変更が要る。冪等 key の確認と投入が前後に混じる | 高 |
| 4 | `route_select` の順位（`route_select/{policy,lane,balance}.rs`） | 6 / +834 −14 | 14 | account・quota・予算に接する。`order_harnesses` は小さいが変更 1 回だけ | 低〜中 |
| 5 | `relay_rejudge::covers` | 2 / +132 −0 | 5 | 純粋だが小さい | 低 |

判断の理由: 変更の頻度と再コンパイルの範囲の積が最も大きく、純粋関数なので境界を動かさずに移せる `execution_gate` を選んだ。2 位の triage は頻度が高いが、数えた commit は分類以外の変更を含み（分類だけの頻度は未計測）、入力の DTO の設計が要るので次回の候補に残す。

## 変更（file・関数）
- `crates/task-core/src/execution_gate/scorer.rs`（新規）: `ExecutionGateInput`・`ExecutionGateRecordedInput`・`ExecutionGateOutput`・`score`。`execution_gate.rs` の `decide_at` は対象外と人の明示の後に `score` を呼ぶ形にした。挙動は変えていない。
- `crates/task-core/src/script_compare/execution_gate_fixture.rs`（新規）・`examples/dump-execution-gate-fixtures.rs`（新規）・`control-ts/fixtures/execution_gate/cases.jsonl`（2,337 行）。
- `crates/task-core/src/script_compare.rs`: `ScriptDomain::ExecutionGate`。`script_guard.rs` の `readiness`: 純粋判定の 1000 件。`migrations/0109_script_execution_gate.sql`。
- `control-ts/src/domains/execution_gate.ts`（新規）: `parseInput`・`decide`・`fixed2`・`executionGate`。`index.ts`・`bundle.config.json`（op の権限は空）・`scripts/compare.mjs`。
- `crates/task-dispatch/src/script_policy.rs`: `PolicyScript::execution_gate`、`classify_execution_gate`（Rust の検証）、`CompareDraft::execution_gate`。`dispatcher/script_canary.rs` の `canary_execution_gate`、`dispatcher/work_units.rs` の `execution_gate_if_needed`。
- `crates/celeris/src/daemon/script_policy.rs`、`crates/script-runtime/src/generation.rs` の `KNOWN_DOMAINS`、`crates/script-runtime/src/bin/script-host-runner.rs` の採点の表（`execution_gate_check`・`execution_gate_broken`）。
- `scripts/dev/script-runner-cache.sh` の golden、`scripts/dev/edit-activate-baseline.sh` の scenario `gate-ts`、`scripts/dev/script-compare.py` の `REQUIRED`、`docs/api/v1/api.md`、schema の再生成。

### 移植で分かった差
Rust の `{ratio:.2}` は 2 桁のちょうど中間を偶数へ丸め（0.625→0.62）、JS の `toFixed(2)` は大きい方へ丸める（0.63）。`recent_budget_exhausted_ratio` は k/n なので 5/8 は実際に起きうる。TS の `fixed2` で 1/8 の奇数倍を判定して揃え、fixture に 0.375・0.625・0.875 を入れた。比較の正規化では隠していない。

## Rust に残したもの
対象外の規則（`out_of_scope_rule`）、人の明示、閾値の計算（`GateThreshold`）、`source`・`shadow`・`depth`・`policy_version`、`direct_route::evaluate`、`update_task` と event の書き込み、store・scheduler・認可・承認・予算・worker の起動。安全検査は 1 つも緩めていない。Rust の参照実装は消していない。

## 試験（提出時の HEAD で実行）
RESULTS

## 性能（raw）
| scenario（warm） | 試行 | p50 秒 | p95 秒 | cargo / link | raw |
|---|---:|---:|---:|---:|---|
| `gate-ts` | 20 | 8.267 | 8.618 | 0 / 0 | [p2f-gate-ts.jsonl](p2f-gate-ts.jsonl) |
| `policy-ts` | 5 | 8.197 | 8.401 | 0 / 0 | [p2f-policy-ts.jsonl](p2f-policy-ts.jsonl) |
| `cos-ts` | 5 | 7.746 | 8.440 | 0 / 0 | [p2f-cos-ts.jsonl](p2f-cos-ts.jsonl) |

- S6 の基準（`policy-ts` 7.519 秒・`cos-ts` 7.470 秒）より 0.28〜0.68 秒遅い。fixture 1 領域ぶんの unit・golden・integration が増えた分と考えられるが、段ごとの S6 との差は読んでいない（未確認）。目標の内にある。
- 検査は `edit-activate-baseline.sh --validate` と `--check-targets` が 3 file とも problems 0。
- 本物の host の 1 呼び出し（[p2f-gate-host-latency.jsonl](p2f-gate-host-latency.jsonl)、4,674 件）: p50 226 µs、p95 305 µs、p99 428 µs、最大 1,104 µs。本番の deadline 50 ms の内にある。
- 旧 Rust の経路で `execution_gate.rs` を変えた時の秒は測っていない。同じ `task-core` の `policy` の p50 234.481 秒（S6）が目安だが、同じ変更の値ではない。

## 未解決
1. 本番の compare の記録はこの領域に 0 件である。bundle の登録と mode の変更は人の手順で、まだ行っていない。
2. 本番の件数の見込みは未確認である。1 task の最初の dispatch に 1 回で、`lane_rules` の 9.33 件/日に近いと予想するが、本番の DB を読んでいない。この値なら 1000 件まで約 107 日で、要件の見直しの task 01M6J81DZ7C8SMY7HNZTW8GT0Z の対象に入れるかを人が決める。
3. `tree::gate::unit_gate` は比較の対象でなく、canary・`ts` でも Rust の規則表で判定する（ADR D4）。同等の間は差が出ない。TS だけで規則を変える段になったら、計画の検証と dispatch で判定がずれるので、その前に設計を決める。
4. `ExecutionGateInputs` の `multi_environment`・`recent_budget_exhausted_ratio` は dispatcher では既定値のままで、本番ではこの 2 信号は常に効かない（既存の挙動。ADR-0072）。fixture と TS は同等に移した。
5. S2-3 の例外として TS に本文を渡している。記録には残さないが、bundle の中の code は本文を読める。op の権限は空なので外には出せない。
6. daemon の RSS の増加（P1-E(2) の未解決 2）の原因は未確認で、この領域の影響も測っていない。

## 次の gate
- 人の判断: 本番で `execution_gate` を compare に上げるか。手順は下の「本番への移行」。canary・`ts` は人の token だけが通る別の承認で、この task では行わない。
- 次の移植候補は順位 2 の triage の分類。分類だけの変更頻度を先に数える。

## 本番への移行（人の手順。この task では実行していない）
1. この変更を含む release を作って promote する（release gate が script の段を通す）。migration 0109 は起動時に当たり、`execution_gate` の行が mode `rust` で入る。config の変更は無い。
2. 配備された bundle を登録する: `celerisctl script --config ~/.config/celeris/config.toml generations` で確かめ、手順は [p1e-shadow-preflight.md](p1e-shadow-preflight.md) の「本番への移行」と同じ。
3. `celerisctl script --config ~/.config/celeris/config.toml domains` で version を読み、`celerisctl script --config ~/.config/celeris/config.toml mode execution_gate compare --expected-version <version> --generation <bundle> --mode-reason "<理由>"` を実行する。
4. 確認: `python3 -I scripts/dev/script-compare.py report --db <本番 DB> --require-ready execution_gate`（読み取り専用）で件数を読む。
5. 戻す: `celerisctl script --config ~/.config/celeris/config.toml mode execution_gate rust --expected-version <version>`。release を戻す前にこの行を `rust` に戻す。

## 提案
1. 次に領域を足すときのために、領域の追加で触る箇所（`ScriptDomain`・migration・`KNOWN_DOMAINS`・runner の表・`GOLDEN_SPECS`・`compare.mjs`・`bundle.config.json`・`build-bundle.test.mjs`）の一覧を `agent-docs/guides/` に置く。
2. `lane_rules` に続いて `execution_gate` も 1 task 1 件なので、純粋判定の 1000 件の要件を領域の頻度に合わせて見直す（task 01M6J81DZ7C8SMY7HNZTW8GT0Z）。

## commit（件名）
- p2f: Complexity Gate の得点を TS の領域 execution_gate に同等移植する（compare・canary・ts の結線、fixture、runner、試験）
- p2f: ts-verify の試験に execution_gate の壊れた規則の場合を足し、bundle の領域の一覧を直す
- p2f: 順位づけと計測の raw、進捗、地図、境界 ADR の付記を足す
