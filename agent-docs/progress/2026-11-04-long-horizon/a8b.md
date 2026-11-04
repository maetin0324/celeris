---
title: A8(実行) long-horizon ベンチの実装と旧 CoS / 新 Manager の比較（イベント再生）
tasks: [01M6JW1EG2G677XHN408JCW2JG]
status: done
completed: 2026-11-04
updated: 2026-11-04
---
# A8(実行) long-horizon ベンチの実装と旧 CoS / 新 Manager の比較（イベント再生）

完了日: 2026-11-04

## 結論
- ADR [long-horizon-bench](../../adr/2026-11-04-long-horizon-bench.md) の課題 10 件（C01〜C10）をイベント再生で実装し、腕 `old.passive`・`old.proactive`（案件 mode `off`、台本の CoS が起票）と `new`（mode `enabled`、本物の control-ts の bundle を本物の V8 で評価する `TsProjectDecider`）を同じ初期条件・同じ台本・同じ oracle で比べた。30 試行すべてが同じ seed の 2 回の実行で facts の bytes 一致。
- 集計（`a8b/replay/summary.json`）:

| 腕 | oracle 合格 | 到達率 | autonomous_reach | avoidable | 無駄起票 | double effect | 回復率 | handoffs |
|---|---|---|---|---|---|---|---|---|
| new | 10/10 | 1.00 | 1.00 | 0 | 0 | 0 | 1.00（16/16） | 78 |
| old.passive | 3/10 | 1.00 | 0.30 | 10 | 0 | 0 | 0.40（6/15） | 98 |
| old.proactive | 8/10 | 1.00 | 0.90 | 1 | 1 | 0 | 0.875（14/16） | 84 |

- 読み方: 到達率の差は出ていない（台本の人の bot が催促するので旧腕も最後には到達する）。差は「人の催促（avoidable）」「回復率」「autonomous_reach」に出た。旧腕の値は台本で決まり、旧 CoS の能力そのものではない（ADR の妥当性の脅威）。C09 は `assumption_bound`。
- 実 LLM pilot は実行していない（D9: worker の run では起こさない）。token・quota・実 walltime は全試行で null（`missing: tier=replay`）で、**未計測**。
- 本番の DB・daemon・release・config、K2/K4 の凍結（`cases/final` 等）には触れていない。試験は一時 DB と `$TMPDIR` だけを使う。

## 実 file・関数
| file | 内容 |
|---|---|
| `crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs`（新規） | 再生の機関 `Trial`（台本の CoS・worker・reviewer・人の bot、仮想時刻 5 分刻み、fault の注入、restart は dispatcher の作り直しと境界の panic）、`RealTs`（`celeris_script::ScriptRuntimeHandle` で `project_controller` 領域を呼ぶ `PolicyScript`）、試験 30 件 `long_horizon_bench_c{01..10}_<name>_{old_passive,old_proactive,new}` |
| `crates/task-dispatch/src/dispatcher/tests/project_wake.rs` | 子 module の宣言 1 か所 |
| `crates/task-dispatch/Cargo.toml` | dev-dependency `celeris-script`（V8。試験だけ） |
| `crates/campaign/src/long_horizon.rs`（新規） | `grade`（述語の連言、`celeris.long-horizon-grade/1`）、`metrics`（D7）、`aggregate`（dataset の混在は Err、腕の結合は採点の後）、`gate`（D8） |
| `crates/campaign/tests/long_horizon_bench.rs`（新規） | raw の採点と gate 1 件、陰性 fixture 12 件、腕の欄と dataset 混在の拒否 1 件（計 14 件） |
| `tests/fixtures/long-horizon/{public,oracle}/` | 課題 10 件の `case.json`・`repo-seed`（Python 標準ライブラリの `minichunk`）・`kb`、`oracle/<case>/{scenario.json,oracle.json,hidden/expected.json}`、`holdout.json`（canary・trial の salt）、`MANIFEST.sha256` |
| `scripts/dev/test-long-horizon-bench.sh`（新規） | 受け入れの検査 |

## 変更の要点
- 採点は facts（両腕に共通の事実だけ。腕の欄なし）・oracle・隠し受入（作業 tree の `out/<id>.txt` の sha256）だけを読む。controller の表（`project_controller_*`）と自己申告は読まない。
- facts の生成で読む表は `tasks`・`events`・`chat_messages`・`feed_sources`（問いの key）。root と Outcome の対応は acceptance の文字どおりの一致。
- 台本の CoS は腕を受け取らない。入力の種類（案件の依頼・root の終了・人の発言・controller の CoS 入力）で応答する。`new` の案件で人の指示を引かない起票は 422 `project_controller_owns_launch` 相当として数え（`cos_launch_rejected`）、`new` で 0 を試験で確かめる。
- 人の bot の催促は D7 の規則（開いた必須の Outcome があり、未終端の root も開いた問いも無い状態が 6 時間続く）。
- 試行 id は `sha256(salt|case|arm|seed)` の先頭 16 桁。腕との対応は `arm-map/` に分けた。

## 契約・設計との差（ADR に付記した）
1. 台本の CoS は brief を書き直した後に `ExternalReleased` の wake（`POST …/controller/wake` 相当）を送る。brief の更新は wake の種類に無く、送らないと controller は `pc/review-failed/waiting` で最大 6 時間待ち、人の催促が先に来る（C02・C04・C10 の `new` で avoidable 1 になった）。本番の CoS がこれを送るかは**未確認**（未解決 1）。
2. `new` の案件で escalate の後の phase は blocked なので、人が依頼文を直す操作では mode も置き直す（C05）。人の操作は 1 件に数えた。
3. 隠し受入は `grade_runner::ProcessRunner` の command ではなく、作業 tree の成果 file の sha256 照合にした（子 process を起こさない）。
4. seed は 1 個（公開）だけ流した。holdout seed 2 個は `holdout.json` に置いたが未使用。
5. C06 の「worker のログの偽の承認文」の注入は実装していない（承認待ちと承認・却下の順序だけ）。C02 の「取り込み前の done」も無い（root に repo が無いので done と取り込みが同時）。
6. `continues_root_ids`・`acceptance_superset`・`message_in_thread` の述語は facts に controller の表を入れない方針と両立しないので実装していない。

## 課題ごとの結果（`a8b/replay/grades.jsonl`）
- `new` は 10 件すべて oracle 合格。fault の発火数は oracle の `expected_faults` と一致（計 16）。
- `old.passive` の不合格 7 件は avoidable の催促（C02・C03・C04・C05・C07・C08 など）と C10 の root 数。`old.proactive` の不合格 2 件は C10（同じ失敗に root を 4 個、催促 1）。

## 試験（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
| コマンド | 結果 |
|---|---|
| `bash scripts/dev/test-long-horizon-bench.sh` | exit 0。replay 30 passed、grade 14 passed、bundle SKIP 0、作業 tree の検査・陽性対照・凍結の path・null の検査が通過 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `bash scripts/dev/test-affected.sh` | 下の「証拠」節 |

## fixture・raw records
- fixture: `tests/fixtures/long-horizon/`（生成に使った script は run の成果物 `gen-fixtures.py`。fixture を変えたら `bench_version` を上げて MANIFEST を作り直す）。
- raw: `agent-docs/progress/2026-11-04-long-horizon/a8b/replay/`（`facts/`・`arm-map/`・`trials/<trial>/repo/`・`grades.jsonl`・`summary.json`）。dataset は `long_horizon_replay` だけ。実 LLM の raw は無い（A9 の `a9/llm/`）。

## 未解決と次の gate
1. 本番の CoS が brief を書き直した後に controller を起こす経路が無い（wake の種類に brief の更新が無い）。ベンチは CoS が wake を送ると仮定した。仮定を外すと `new` は C02・C04・C10 で avoidable 1。直すなら A9 以降で brief の更新を wake にするか、CoS の skill に wake の送信を足す。
2. escalate の後に人が依頼文を直しても mode を置き直すまで controller は blocked のまま（C05）。人の手順が 2 操作になる。
3. 実 LLM pilot（C04・C06・C08・C09・C10 × 2 腕 + `pre_a1`）は未実施。token・quota・実 walltime・LLM 判断の C09 は未計測。次の gate は A9（人か外部 operator が sandbox の外から実行）。
4. この worktree の target では v8 の build script の出力が古い seed の path を指していて link に失敗した。`$CARGO_TARGET_DIR/debug/build/v8-*` を消して作り直した（環境の問題でコードの変更は無い）。
5. holdout seed・複数 seed の反復、`fault_not_fired`・`leak_detected` の無効化の経路は replay では使っていない。

## 未達
- 実 LLM pilot の記録（`long_horizon_llm_pilot`）と `--tier llm_pilot` の経路の stub 確認（`ops-r2-sandbox.sh`・probe-read）は未実装。
- 上の「契約・設計との差」5・6 の注入と述語。

## 本番への移行
無い。

## 提案
- brief の更新（`project_brief_put`）を controller の wake にする（未解決 1）。A6 の 6 時間の待ちより先に人の催促が来る型を消せる。
