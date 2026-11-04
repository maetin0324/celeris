---
title: P2-G — 実 LLM の worker による TS 方針の自己改善を非本番で小規模に試す
tasks: [01M6HGFR1TVSBS63STM8NH3XCS]
status: done
completed: 2026-11-04
updated: 2026-11-04
---
# P2-G — 実 LLM の worker による TS 方針の自己改善の非本番試験

完了日: 2026-11-04

親 task 01M6HG7T6ZKTX239KGH0QY4HFQ（段階移行 2）の子 p2g（p1d の後）。設計は
[ADR 2026-11-03-llm-self-improve-trial](../../adr/2026-11-03-llm-self-improve-trial.md)。

## 結論
- 実 LLM の worker（Celeris の子 task、claude-code・claude-opus-5-5）は、課題 2 件 × 経路 2 つの 4 試行すべてで、採点に通る候補 patch を作った（成功 4/4、人の介入 0 件）。
- TS の経路は 2 課題とも trial 1 回で ok。edit→activate は 7.90〜8.16 秒（再採点 8.23・13.88 秒）、cargo 0 回。旧 Rust の経路は 496〜575 秒（再採点 347〜481 秒）。
- 旧 Rust の T1 は、課題と関係のない時間依存の試験（task-dispatch の an_interrupted_integration_is_redone_idempotently_after_restart）が負荷で 2 回落ち、同じ patch のやり直し 3 回目で ok になった。
- 親（この run）は 4 件の candidate.patch を同じ trial.sh で採点し直し、4 件とも ok（rescore/trial.jsonl）。
- 本番の承認は自律化していない。試験 daemon でも、人でない主体の canary と改善仕様の承認は 403 で拒否される（pilot の段 canary_denied_non_human・behavior_change_denied_non_human）。

## 状況
- 1 回目の run: 試験の道具・課題 2 件・採点を作り、LLM を使わない参照の候補で T1 を両経路で通した。実 LLM の試行 4 件は子 task に委譲して yield した。
- 子 4 件（下の「試行」）は 2026-11-03 23:34Z〜2026-11-04 00:52Z に完了し、celeris が main に取り込んだ。
- 2 回目の run（continuation、この節を書いた run）: main を取り込み、候補 patch 4 件の再採点、collect.py による利用量の集計（reviewer と worker の run を分ける `by_role` を足した）、結果の表と比較をこの file に書いた。

## 変更の要点（file・関数）
| file | 要点 |
|---|---|
| `crates/task-core/examples/behavior-change-cases.rs` | `gen`: 課題の入力から期待差分 fixture（Rust の参照実装の出力と、仕様の編集を当てた期待出力）を作り、仕様の `cases_sha256` を書き換える。`score`: この build の `cos_digest_decide`（旧 Rust の経路の候補）を旧 fixture と期待差分 fixture で数える（`ts-behavior-eval.sh` と同じ欄・同じ合否） |
| `scripts/dev/llm-improve/tasks/T1-cap-background/`・`T2-cap-item-title/` | 課題文（`task.md`）・改善仕様（`spec.json`）・入力（`inputs.jsonl`、`make-inputs.py` が旧 fixture から作る）・期待差分 fixture（`spec.cases.jsonl`）。T1 は 16 件（explained 9・equal 7）、T2 は 12 件（explained 8・equal 4） |
| `scripts/dev/llm-improve/trial.sh` | 候補 patch を TS の経路（scope→eval→pilot）か旧 Rust の経路（scope→check・clippy・test-build・test-run・build・release-build→score）で検証し、1 行の raw record（`celeris.llm-improve-trial/1`）を書く。`--validate` で記録を検査 |
| `scripts/dev/llm-improve/collect.py` | 試行の子 task の event を DB（読み取り専用）から読み、source・model・token・cost・walltime・run の数・check の失敗・review の差し戻し・人の介入を 1 子 1 行の JSONL（`celeris.llm-improve-usage/1`）に書く。2 回目の run で `by_role`（worker_started の role で worker と reviewer の run を分けた run 数・token・cost・wall）を足した |
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

## 結果（実 LLM の試行 4 件）
raw records: 子の trial は各 dir の `trial.jsonl`、親の再採点は [rescore/trial.jsonl](p2g-llm-self-improve/rescore/trial.jsonl)（段の記録は `rescore/<label>/`）、
利用量は [usage.jsonl](p2g-llm-self-improve/usage.jsonl)（collect.py の出力）。

### 試行の結果
| 試行（子 task） | 変えた file・関数 | patch | trial の回数 | 最後の trial | edit→activate | 採点（旧 fixture 差分 / 期待差分 食い違い） | 再採点（親） |
|---|---|---|---|---|---|---|---|
| T1 TS（01M6HW7NFHTFSG78DHWQNHBTHS） | `control-ts/src/domains/cos_digest.ts` の `capBackground` を `digestMessage` の背景行に当てる＋試験 3 件 | +43/-1 | 1（ok） | 93.9 秒 | 8.16 秒 | 0/2066・0/16（explained 9・equal 7） | ok、85.1 秒、edit→activate 8.23 秒 |
| T2 TS（01M6HW7NFH36N2AGX1NR5G9RVS） | `cos_digest.ts` の `title` が `capTitle`（`Array.from` で code point を数える）を通す＋試験 5 件 | +51/-2 | 1（ok） | 94.2 秒 | 7.90 秒 | 0/2066・0/12（explained 8・equal 4） | ok、25.1 秒、edit→activate 13.88 秒 |
| T1 旧 Rust（01M6HW7NFHGCREMDBFR1S20PH5） | `crates/task-core/src/chat/digest.rs` に `clip_background`＋単体試験 1 件 | +29/-1 | 3（1・2 回目は test-run で flaky、3 回目 ok） | 627.6 秒 | 575.3 秒 | 0/2066・0/16 | ok、517.2 秒、6 段の和 481.2 秒 |
| T2 旧 Rust（01M6HW7NFH56896K8PBRDXW934） | `crates/task-core/src/chat/triage.rs` に `CosTriageItemReport::digest_title`、`digest.rs` の `cos_digest_message` の見出しと card で使う＋試験 1 件 | +47/-1 | 1（ok） | 533.0 秒 | 496.2 秒 | 0/2066・0/12 | ok、379.9 秒、6 段の和 347.3 秒 |

- TS の pilot（4 回とも）: 18 段すべて通過。人でない主体の canary 403・改善仕様の承認 403、人の token の承認 200、canary の再起動の復元 0.30〜0.60 秒、rust への rollback 1.02 秒、cargo 0 回。
- 旧 Rust の 2 試行で、LLM は `title()` を変えず digest 用の関数を別に足した（T2）。fallback・launch・review の出力を変えないためと notes.md に書いている。TS の T2 は `title` を直接変えた（TS の domain の `title` は digest の中だけで使う）。
- 再採点の T2 TS の pilot（19.4 秒）は T1 TS（78.0 秒）より短い。T1 の直後で、一時 daemon の起動前の準備（bundle・node_modules）が温まっていたためと推定する（未確認）。

### LLM の利用・時間・やり直し・人の介入（usage.jsonl）
source は 4 件とも adapter claude-code・model claude-opus-5-5・provider claude_oauth（account は claude_max_lab、TS の 2 件は claude_max_personal も使った）。
cost は harness が報告する換算値（USD）で、OAuth の定額枠の実際の請求額ではない。

| 試行 | 実装の run（数・wall・出力 token・cache read・cost） | review の run（数・wall・cost） | check の失敗 | review の差し戻し | 人の介入 |
|---|---|---|---|---|---|
| T1 TS | 1・220.5 秒・6,812・468,171・0.949 | 2・1,339.7 秒・0.911 | 0 | 0 | 0 |
| T2 TS | 1・217.3 秒・6,787・577,624・1.038 | 1・1,126.9 秒・0.246 | 0 | 0 | 0 |
| T1 旧 Rust | 1・2,579.7 秒・10,037・1,705,690・0.934 | 1・530.7 秒・0.178 | 0 | 0 | 0 |
| T2 旧 Rust | 1・1,041.6 秒・6,585・888,848・0.661 | 1・485.8 秒・0.210 | 0 | 0 | 0 |

- 実装の run の wall は TS 約 220 秒、旧 Rust 1,042〜2,580 秒。差のほとんどは trial（cargo の 6 段）と再採点の待ちで、LLM の出力 token は同じ程度（6.6k〜10.0k）。
- T1 TS の review は 2 run あった。両方 done で、差し戻しではない。2 回目が走った理由は未確認。
- 成功率: 4/4（初回 trial で ok は 3/4）。検証の失敗: 2 回（旧 Rust T1 の test-run、課題と無関係な flaky）。再試行: 2 回（同じ patch）。

### 異常時の復帰
- TS: pilot の段で、canary の再起動の復元（canary のまま戻る）と rust への rollback と rollback 後の再起動の復元を 4 回とも通した。
- 旧 Rust: activate・rollback は release の作成・promote・daemon の再起動という本番の操作なので、試験しない（採点まで）。
- 試験の失敗（旧 Rust T1）: worker は別 worktree で失敗した試験を単独 3 回・2 crate 全体 2378 件で通し、flaky と判断して patch を変えずにやり直した。人の介入は無い。

### 検証範囲の差
| 項目 | TS の経路 | 旧 Rust の経路 |
|---|---|---|
| 採点 | 旧 fixture 2066 件＋期待差分 fixture（同じ file） | 同じ |
| 静的検査 | tsc の型検査（ts-verify） | cargo check・clippy（2 crate） |
| 試験 | control-ts の単体・golden・安全・統合（ts-verify 10 段） | task-core・task-dispatch の全試験（約 2378 件） |
| build | 候補 bundle（esbuild） | celeris の debug・release build |
| 配備・昇格 | 一時 DB の daemon へ配備・登録→compare→canary（承認 403/200 を含む） | 行わない（本番の操作） |
| rollback・再起動の復元 | 一時 daemon で実施 | 行わない |
| patch の範囲 | `control-ts/src/` の中だけ | `crates/task-core/src/`・`crates/task-dispatch/src/` の中だけ |

TS は Rust 側の試験（task-core・task-dispatch）を走らせない。代わりに Rust が持つ安全検証（承認・guard・fail-closed）を一時 daemon で通す。Rust は全試験を走らせるが、昇格と rollback は確かめない。

## 証拠（1 回目の run）
- 期待差分 fixture の生成: `cargo run -q -p task-core --example behavior-change-cases -- gen --spec … --inputs … --out …` を 2 課題 → exit 0（T1 16 件・T2 12 件、上の内訳）。
- 参照の候補（LLM なし、[oracle/oracle-t1-ts.patch](p2g-llm-self-improve/oracle/oracle-t1-ts.patch)）:
  `bash scripts/dev/llm-improve/trial.sh --task T1-cap-background --path ts --patch …/oracle-t1-ts.patch --out …` → exit 0、80.7 秒。
  - eval: 旧 fixture 2066 件の差分 0、期待差分 16 件（explained 9・equal 7・食い違い 0）、6.8 秒、cargo 0 回。
  - pilot: 18 段すべて通過（`behavior_change_denied_non_human` 403・`behavior_change_approve` 200 を含む）。ts-verify 8.08 秒・cargo 0 回、
    edit→activate（ts-verify＋bundle＋配備＋登録）8.39 秒、rollback 1.02 秒、再起動の復元（canary）0.59 秒。
  - raw records: [oracle/trial.jsonl](p2g-llm-self-improve/oracle/trial.jsonl)、段の記録 [oracle/oracle-t1-ts-pilot.jsonl](p2g-llm-self-improve/oracle/oracle-t1-ts-pilot.jsonl)。
  - `bash scripts/dev/llm-improve/trial.sh --validate …/oracle/trial.jsonl` → exit 0。
- 旧 Rust の経路の参照の候補（[oracle/oracle-t1-rust.patch](p2g-llm-self-improve/oracle/oracle-t1-rust.patch)）:
  `bash scripts/dev/llm-improve/trial.sh --task T1-cap-background --path rust --patch …/oracle-t1-rust.patch --out …` → exit 0、452.9 秒。
  段の秒: check 34.6・clippy 34.8・test-build 81.9・test-run 59.1・build 56.1・release-build 151.2・score 28.3。edit→activate 相当（6 段の和）417.7 秒。
  score: 旧 fixture 2066 件の差分 0、期待差分 16 件（explained 9・equal 7・食い違い 0）。使い捨ての作業ツリー（$TMPDIR）なので workspace の crate は path が変わって作り直しになる（S6 の warm 344.5 秒と同じ条件ではない）。
- 負の対照（採点が落とすことの確認、LLM なし）:
  - T1 で 299 文字で切る patch（[oracle/neg-t1-ts-299.patch](p2g-llm-self-improve/oracle/neg-t1-ts-299.patch)）→ exit 1、段 eval で落ちる（explained 0・食い違い 12・旧 fixture の差分 0）、5.8 秒。
  - T2 に T1 の参照の patch を当てる → exit 1、段 eval で落ちる（explained 0・食い違い 8）、2.8 秒。
- 4 行の記録は [oracle/trial.jsonl](p2g-llm-self-improve/oracle/trial.jsonl)。`trial.sh --validate` → exit 0（4 行）。

## 性能
- TS の経路の edit→activate: 子の trial 8.16・7.90 秒、親の再採点 8.23・13.88 秒、参照の候補 8.39 秒（5 試行、p50 8.23 秒・最大 13.88 秒）。cargo は全部 0 回。
  S6 基準（warm p50 7.47〜7.52 秒）と同じ桁で、目標（p50 30 秒以下・p95 60 秒以下）の内。ts-verify.sh 自体は変えていない。
- 旧 Rust の経路（6 段の和）: 子の ok の trial 575.3・496.2 秒、再採点 481.2・347.3 秒、参照の候補 417.7 秒（p50 481.2 秒）。TS の約 58 倍（p50 の比）。
  使い捨ての作業ツリーで crate を作り直す条件なので、S6 の warm 344.5 秒と同じ条件ではない。

## 1 回目の run の検査（その時点の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/test-affected.sh`: 1 回目 exit 100（7237 件中 1 件、下の未解決 2 の flaky）、2 回目 exit 0（nextest 7237 passed・21 skipped、web 697 passed、test-ts-verify 18 passed）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p task-core --example behavior-change-cases -- -D warnings`: 警告 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-doc-links.sh`・`check-adr-numbers.sh`（362 files）・`sh scripts/dev/progress-index.sh --check`・`python3 scripts/dev/check-architecture-map.py`: すべて ok。
- 最終の検査は子の取り込みの後の run でやり直す。

## 提出時の検査（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/llm-improve/trial.sh --task <T> --path ts|rust --patch …/<T>-<path>/candidate.patch --out …/rescore --label <T>-<path>` を 4 件: 全部 exit 0（上の表）。
- `bash scripts/dev/llm-improve/trial.sh --validate …/rescore/trial.jsonl`: exit 0（4 行）。
- `python3 -I scripts/dev/llm-improve/collect.py --db "$CELERIS_RUN_DB" --parent 01M6HGFR1TVSBS63STM8NH3XCS --out …/usage.jsonl`: exit 0（4 行）。
- `bash scripts/dev/test-affected.sh`: exit 0（この run の変更は collect.py と記録だけで、試験対象の crate の変更なし。文書の自己試験 ok）。
  crates と control-ts の本体はこの task で変えていない（試行の候補 patch は commit していない）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`・`bash scripts/dev/check-doc-links.sh`・`python3 -I scripts/dev/check-architecture-map.py`: すべて exit 0。

## 未解決
1. flaky: `task-dispatch` の `dispatcher::tests::work_units::an_interrupted_integration_is_redone_idempotently_after_restart`（work_units.rs:2993、Blocked≠Done）が旧 Rust の T1 の trial で負荷時に 2 回落ちた。単独・全体の再実行では pass。決定化はこの task の範囲外。
2. flaky の疑い（1 回目の run）: `celeris::script_generation_e2e script_generation_e2e_switch_error_rollback_restart` が 1 回落ちた（:785、in_flight 2≠1）。単独 3 回 pass。原因は未確認。
3. 試験 daemon では実際の digest pass を起こせない。canary の間の `explained` の数え方は p1d の e2e が受け持ち、この試験の canary は guard の計数を一時 DB に synthetic に積んで通している（S6 pilot と同じ）。
4. 採点の file は repo の中にあり、子の worker は読める。4 件とも notes.md で「読んでいない」と書いたが、確かめる手段は transcript だけ。
5. 改善仕様の `when` は item 0 だけを見る（p1d 未解決 3）。項目が 2 件以上で 2 件目も長い入力は本番の比較で `mismatch` になる（fail-closed の向き）。
6. 試行は各 1 回・課題 2 件で、成功率と時間の分布を言える数ではない。TS の改善を本番で使うには、人の承認（改善仕様と canary・ts の昇格）が要る。この task では行っていない。
7. 旧 Rust の経路は activate・rollback を測っていない（本番の操作）。両経路の「edit→activate」は同じ範囲の操作ではない（上の「検証範囲の差」）。

## 次の gate
- 自由課題（改善仕様も LLM が書く）の試行。採点の正解を候補が決めることになるので、人が改善仕様を承認する手順（`/api/v1/script/behavior-changes` の人の承認）を通す形で別の task にする。
- 本番への適用は、p1e-shadow-measure（人の実行）で compare の記録を読み、人が canary へ上げると決めた後。
- 採点の file を worker から隠す仕組み（kit を worktree の外に置く）。今は課題文での禁止だけ。

## 本番への移行
- なし。本番の host・DB・mode・release・guard、IEEE Access・K2・K4 の環境には触れていない。config の欄も足していない。collect.py は本番 DB を `mode=ro` で読むだけ。

## 提案
- trial.sh の試験 daemon の記録を、TS の改善の候補 bundle ごとに残し、人が canary を承認する画面から参照できるようにする（今は進捗の dir にだけある）。
- 旧 Rust の経路の比較を続けるなら、task-dispatch の flaky 1 件（未解決 1）を先に決定化する。試行の失敗の半分がこれだった。
