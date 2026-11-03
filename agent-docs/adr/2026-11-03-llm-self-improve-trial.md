---
tasks: [01M6HGFR1TVSBS63STM8NH3XCS]
---
# ADR 2026-11-03: 実 LLM の worker による TS 方針の自己改善の非本番試験（P2-G）の設計

- 状態: 採用（実装済み。記録は [p2g-llm-self-improve](../progress/2026-11-03-ts-control-ops/p2g-llm-self-improve.md)）。
- 関係: [TS の意図した方針改善](2026-11-03-ts-intended-behavior-change.md)（改善仕様・期待差分 fixture・承認）、
  [TS の高速検証](2026-11-02-ts-fast-verify.md)（ts-verify）、[境界 ADR](2026-11-02-ts-control-layer-boundary.md) 付記 S5・S6（pilot・測定）、
  [worker は subagent・LLM CLI を起動しない](2026-10-07-worker-no-subagents-no-llm-cli.md)。

## 結論
- LLM は Celeris の子 task の worker（通常の routing で選ばれた source・model）だけが使う。この試験の script は LLM を呼ばない。
  親の run も LLM の CLI・API を起動しない。
- 課題は 2 件（T1 背景を 300 文字で切る・T2 項目の見出しを 80 文字で切る。`scripts/dev/llm-improve/tasks/`）。
  課題ごとに TS の経路と旧 Rust の経路の 2 つの子 task を作る（2×2＝4 件）。同じ課題文・同じ採点・同じ安全条件で比べる。
- 採点は決定的（`scripts/dev/llm-improve/trial.sh`）。課題の改善仕様（`celeris.script-behavior-change/1`）と期待差分 fixture は
  Rust の参照実装と仕様から example（`crates/task-core/examples/behavior-change-cases.rs` の `gen`）が作る。
  TS の候補は `ts-behavior-eval.sh --changes <課題>`、Rust の候補は同じ example の `score` で、同じ欄（旧 fixture の差分・
  期待差分の一致・食い違い）を数える。合否は両方とも「旧 fixture 2066 件の差分 0・期待差分 fixture の食い違い 0」。
- TS の経路は採点の後に `ts-pilot.sh`（`TS_PILOT_PATCH`・`TS_PILOT_SPEC`）で、ts-verify 10 段→候補 bundle→一時 DB の daemon へ
  配備・登録→compare→人でない主体の canary と承認の 403→試験 daemon の人の token の承認→canary→再起動の復元→rust への rollback
  →再起動の復元までを通す。旧 Rust の経路は edit-activate-baseline.sh の cos-digest と同じ 6 段（2 crate の check・clippy・試験・
  celeris の debug・release build）と採点まで。旧経路の activate（release・promote・再起動）と rollback は本番の操作なので行わない。
- 候補の範囲は patch で決める: TS は `control-ts/src/` の中だけ、Rust は `crates/task-core/src/`・`crates/task-dispatch/src/` の中だけ。
  外を変える patch は scope の段で落とす（採点の file・golden・検証の道具を候補に変えさせない）。採点の file は候補の作業ツリーではなく
  HEAD の kit と旧 fixture から読む。
- 子 task は source を commit しない。候補は `candidate.patch` として記録の dir に残し、親が同じ trial.sh で採点し直す。

## 記録するもの
| 項目 | 出所 |
|---|---|
| source・model・harness | 子 task の event（`routing_decided`・`WorkerFinished` の run） |
| token・cost・walltime | 子 task の `WorkerFinished.usage`・`metrics.wall_ms` の和 |
| 成功率 | 子 task の最後の trial の `ok`（決定的な再採点） |
| 検証失敗・再試行数 | 子の trial.jsonl の行数（ok でない行）、子 task の attempts・check の失敗の event |
| 人の介入 | 子 task の event の人の回答・comment・決定 |
| 異常時の復帰 | trial の rollback・再起動の復元の秒（TS）、候補の失敗の段（`first_failed_stage`） |

## 守ること
- 本番の host・DB・mode・release・guard、IEEE Access・K2・K4 の測定環境と評価器・hidden 資料に触れない。課題・採点の file は
  この repo の `scripts/dev/llm-improve/` と旧挙動の fixture だけで、K2・K4 の資産を読まない。
- 承認・昇格の人の判断を自律化しない。試験 daemon の「人の token」は一時 DB の中だけの token で、本番の承認ではない（記録に書く）。
- 既存の安全検査・承認拒否・fail-closed を緩めない。比較の正規化を足さない（改善仕様の byte 一致だけ）。

## 不採用
- **親の run が自分で課題を解く**: 親の run も実 LLM だが、試験の道具を作った run なので課題の答えを知っている。token の内訳も分けられない。
- **課題の改善仕様を候補の LLM に書かせる（自由課題）**: 採点の正解を候補が決めることになり、Rust の経路と比べられない。次の gate に回す。
- **試験 daemon で実際の digest pass を起こす**: CoS の run が要り、CoS を有効にした試験 daemon は fake の harness では作れない。
  canary の間の `explained` の数え方は p1d の dispatcher の e2e（`cos_digest_behavior_change_canary_*`）が受け持つ。
