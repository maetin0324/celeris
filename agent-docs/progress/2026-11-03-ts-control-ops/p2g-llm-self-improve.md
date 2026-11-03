---
title: P2-G — 実 LLM の worker による TS 方針の自己改善を非本番で小規模に試す
tasks: [01M6HGFR1TVSBS63STM8NH3XCS]
status: running
updated: 2026-11-03
---
# P2-G — 実 LLM の worker による TS 方針の自己改善の非本番試験

親 task 01M6HG7T6ZKTX239KGH0QY4HFQ（段階移行 2）の子 p2g（p1d の後）。設計は
[ADR 2026-11-03-llm-self-improve-trial](../../adr/2026-11-03-llm-self-improve-trial.md)。

## 状況
- 1 回目の run（この節を書いた run）: 試験の道具・課題 2 件・採点を作り、LLM を使わない参照の候補（私が書いた正解例）で T1 の TS の経路を通した。
  実 LLM の試行 4 件は子 task として委譲した（下の「試行」）。試行の結果はまだ無い。
- 次の run（continuation）が子 4 件の終わりを待ち、子の branch を取り込み、同じ trial.sh で採点し直して、結果の表をこの file に書く。

## 変更の要点（file・関数）
| file | 要点 |
|---|---|
| `crates/task-core/examples/behavior-change-cases.rs` | `gen`: 課題の入力から期待差分 fixture（Rust の参照実装の出力と、仕様の編集を当てた期待出力）を作り、仕様の `cases_sha256` を書き換える。`score`: この build の `cos_digest_decide`（旧 Rust の経路の候補）を旧 fixture と期待差分 fixture で数える（`ts-behavior-eval.sh` と同じ欄・同じ合否） |
| `scripts/dev/llm-improve/tasks/T1-cap-background/`・`T2-cap-item-title/` | 課題文（`task.md`）・改善仕様（`spec.json`）・入力（`inputs.jsonl`、`make-inputs.py` が旧 fixture から作る）・期待差分 fixture（`spec.cases.jsonl`）。T1 は 16 件（explained 9・equal 7）、T2 は 12 件（explained 8・equal 4） |
| `scripts/dev/llm-improve/trial.sh` | 候補 patch を TS の経路（scope→eval→pilot）か旧 Rust の経路（scope→check・clippy・test-build・test-run・build・release-build→score）で検証し、1 行の raw record（`celeris.llm-improve-trial/1`）を書く。`--validate` で記録を検査 |
| `scripts/dev/ts-behavior-eval.sh` | `--changes <dir>` を足した（課題の期待差分 fixture を渡す。既定は今までどおり `control-ts/behavior-changes/cos_digest`） |
| `scripts/dev/ts-pilot.sh` | 環境変数 `TS_PILOT_PATCH`（固定の改善の代わりに候補 patch を当てる。`control-ts/src/` の外を変える patch は失敗）・`TS_PILOT_SPEC`（試験 daemon で人でない主体の承認が 403・人の token の承認が 200 になる段 `behavior_change_denied_non_human`・`behavior_change_approve` を足し、段 diff の probe を期待差分 fixture の入力にする）・`TS_PILOT_CHANGE`。無ければ S6 pilot のまま |

課題の入力の長さ（背景 150〜1000 文字、見出し 80〜300 文字）は、旧 fixture の文字列の最長 11 文字より長いので、旧 fixture の出力は変わらない
（`ts-verify.sh` の golden の段は改善仕様を読まないので、旧 fixture に当たる改善は今は試せない。p1d の未解決 1）。

## 試行（実 LLM、子 task）
| 子 | 課題 | 経路 | 記録の dir |
|---|---|---|---|
| 1 | T1（背景を 300 文字で切る） | TS | `p2g-llm-self-improve/T1-cap-background-ts/` |
| 2 | T2（見出しを 80 文字で切る、本文と card） | TS | `p2g-llm-self-improve/T2-cap-item-title-ts/` |
| 3 | T1 | 旧 Rust | `p2g-llm-self-improve/T1-cap-background-rust/` |
| 4 | T2 | 旧 Rust | `p2g-llm-self-improve/T2-cap-item-title-rust/`（3 の後。build が重いので直列） |

各子は親の branch を取り込み、改善案（`proposal.md`）→実装→`candidate.patch`→`trial.sh` を ok まで繰り返し、記録だけを commit する。
子の受け入れ条件は trial の記録の検査・最後の trial が ok・候補 patch の再採点・source を commit していないこと・改善案と記録があること。

## 証拠（1 回目の run）
- 期待差分 fixture の生成: `cargo run -q -p task-core --example behavior-change-cases -- gen --spec … --inputs … --out …` を 2 課題 → exit 0（T1 16 件・T2 12 件、上の内訳）。
- 参照の候補（LLM なし、[oracle/oracle-t1-ts.patch](p2g-llm-self-improve/oracle/oracle-t1-ts.patch)）:
  `bash scripts/dev/llm-improve/trial.sh --task T1-cap-background --path ts --patch …/oracle-t1-ts.patch --out …` → exit 0、80.7 秒。
  - eval: 旧 fixture 2066 件の差分 0、期待差分 16 件（explained 9・equal 7・食い違い 0）、6.8 秒、cargo 0 回。
  - pilot: 18 段すべて通過（`behavior_change_denied_non_human` 403・`behavior_change_approve` 200 を含む）。ts-verify 8.08 秒・cargo 0 回、
    edit→activate（ts-verify＋bundle＋配備＋登録）8.39 秒、rollback 1.02 秒、再起動の復元（canary）0.59 秒。
  - raw records: [oracle/trial.jsonl](p2g-llm-self-improve/oracle/trial.jsonl)、段の記録 [oracle/oracle-t1-ts-pilot.jsonl](p2g-llm-self-improve/oracle/oracle-t1-ts-pilot.jsonl)。
  - `bash scripts/dev/llm-improve/trial.sh --validate …/oracle/trial.jsonl` → exit 0。
- 旧 Rust の経路の参照の候補（[oracle/oracle-t1-rust.patch](p2g-llm-self-improve/oracle/oracle-t1-rust.patch)）はまだ走らせていない（次の run）。

## 性能
- TS の経路の edit→activate（参照の候補）8.39 秒、ts-verify 8.08 秒・cargo 0 回。S6 の warm p50 7.47〜7.52 秒と同じ桁で、目標（p50 30 秒・p95 60 秒）の内。
  1 試行だけの値で、p50・p95 ではない。ts-verify.sh 自体は変えていない。

## 未解決
1. 実 LLM の試行 4 件の結果（source・model・token・cost・walltime・成功率・再試行・人の介入）はまだ無い。次の run で集める。
2. 旧 Rust の経路の参照の候補の実行はまだ（次の run）。
3. 試験 daemon では実際の digest pass を起こせない（CoS を有効にした daemon が要る）。canary の間の `explained` の数え方は p1d の e2e が受け持ち、
   この試験の canary は guard の計数を一時 DB に synthetic に積んで通している（S6 pilot と同じ）。
4. 課題の採点の file は repo の中にあり、子の worker は読める。読まないよう課題文で求め、読んだら proposal.md に書かせる（守られたかは transcript でしか確かめられない）。
5. 改善仕様の `when` は item 0 だけを見る（p1d 未解決 3 の表せる範囲）。項目が 2 件以上で 2 件目も長い入力は本番の比較で `mismatch` になる（fail-closed の向き）。

## 次の gate
- 子 4 件の完了と、この file の結果の表。
- 自由課題（改善仕様も LLM が書く）の試行は、採点の正解を候補が決めることになるので、人が改善仕様を承認する手順を通す形で次の task にする。

## 本番への移行
- なし。本番の host・DB・mode・release・guard、IEEE Access・K2・K4 の環境には触れていない。config の欄も足していない。

## 提案
- なし（結果が出てから書く）。
