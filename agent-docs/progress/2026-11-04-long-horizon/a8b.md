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
- ADR [long-horizon-bench](../../adr/2026-11-04-long-horizon-bench.md) の課題 10 件（C01〜C10）をイベント再生で実装し、腕 `old.passive`・`old.proactive`（案件 mode `off`）・`new`（mode `enabled`、本物の control-ts の bundle を本物の V8 で評価する `TsProjectDecider`）と、対照 `ctl.null`（mode `enabled`、常に待つ decider、人の bot は催促しない）を流した。40 試行すべてで、同じ seed の 2 回の実行の facts が bytes 一致した。
- 同一初期条件は digest で検査する。案件・request・`project_repos`・thread・brief・`authority_grants` の行（ULID・時刻・案件 id を伏せる）と作業 tree の hash を、mode を置く前に取る。課題ごとに 4 腕の digest が一致し、`invalid_init` は 0 件だった。`launch_root` の許可は全腕に同じ行を作る（ADR の未解決の判断を「揃える」で決めた）。
- 集計（`a8b/replay/summary.json`）:

| 腕 | oracle 合格 | 到達率 | autonomous_reach | avoidable | 無駄起票 | double effect | 回復率 | handoffs | 指示なしの到達 | controller の root |
|---|---|---|---|---|---|---|---|---|---|---|
| new | 10/10 | 1.00 | 1.00 | 0 | 0 | 0 | 1.00（16/16） | 96 | 15 | 20 |
| old.passive | 3/10 | 1.00 | 0.30 | 10 | 0 | 0 | 0.40（6/15） | 98 | 0 | 0 |
| old.proactive | 8/10 | 1.00 | 0.90 | 1 | 1 | 0 | 0.875（14/16） | 84 | 10 | 0 |
| ctl.null（対照） | 2/10 | 0.18（3/17） | 0.20 | 0 | 0 | 0 | 0.00（0/3） | 14 | 0 | 0 |

- 読み方: `new` と旧腕の到達率に差は出ていない。台本の人の bot が催促するので、旧腕も最後には到達する。差が出たのは avoidable（人の催促）、回復率、autonomous_reach。旧腕の値は台本で決まり、旧 CoS の能力そのものではない（ADR の妥当性の脅威）。C09 は `assumption_bound`。
- 対照 `ctl.null` の到達は 3 件で、すべて人の指示（C09 の発言、C10 の回答）を引いた root による。指示なしの到達と controller の root はどちらも 0 で、`new` の到達のうち 15 件は指示なしの root による。到達は台本の足場ではなく controller の働きで起きている。
- 実 LLM pilot は実行していない（D9: worker の run では起こさない）。token・quota・実 walltime は全試行で null（`missing: tier=replay`）で、**未計測**。pilot の raw を記録・採点する経路（`--tier llm_pilot`）は実装し、stub の facts で確かめた（下の「実 LLM pilot の記録の経路」）。
- 本番の DB・daemon・release・config と、K2/K4 の凍結（`cases/final` 等）には触れていない。試験が使うのは一時 DB と `$TMPDIR` だけ。

## 実 file・関数
| file | 内容 |
|---|---|
| `crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs`（新規） | 再生の機関 `Trial`（台本の CoS・worker・reviewer・人の bot、5 分刻みの仮想時刻、fault の注入、restart は dispatcher の作り直しと境界での panic）、`Trial::init_digest`・`tree_digest`（同一初期条件）、`Trial::controller_enabled`（DB の mode を読む）、`NullDecider`（対照）、`RealTs`（`celeris_script::ScriptRuntimeHandle` で `project_controller` 領域を呼ぶ `PolicyScript`）。試験 40 件 `long_horizon_bench_c{01..10}_<name>_{old_passive,old_proactive,new,ctl_null}` |
| `crates/task-dispatch/src/dispatcher/tests/project_wake.rs` | 子 module の宣言 1 か所 |
| `crates/task-dispatch/Cargo.toml` | dev-dependency `celeris-script`（V8。試験だけ） |
| `crates/campaign/src/long_horizon.rs`（新規） | `grade`（述語の連言、`celeris.long-horizon-grade/1`）、`metrics`（D7。`reached_without_instruction`・`daemon_roots` を含む）、`aggregate`（dataset の混在は Err、課題ごとの `init_digest` の不一致は `invalid_init`、腕の結合は採点の後）、`aggregate_tier`・`dataset_of_tier`・`cli`（tier 別の採点）、`gate`（D8 と対照の検査） |
| `crates/campaign/src/main.rs` | subcommand `long-horizon-grade --tier <replay\|llm_pilot> --raw --fixtures --out` |
| `crates/campaign/tests/long_horizon_bench.rs`（新規） | raw の採点と gate 1 件、陰性 fixture 12 件、腕の欄と dataset 混在の拒否 1 件、`invalid_init` 1 件、対照 1 件、pilot の tier 1 件（計 17 件） |
| `tests/fixtures/long-horizon/{public,oracle}/` | 課題 10 件の `case.json`・`repo-seed`（Python 標準ライブラリの `minichunk`）・`kb`、`oracle/<case>/{scenario.json,oracle.json,hidden/expected.json}`、`holdout.json`（canary・trial の salt）、`MANIFEST.sha256` |
| `scripts/dev/test-long-horizon-bench.sh`（新規） | 受け入れの検査 |

## 変更の要点
- 採点が読むのは facts（全腕に共通の事実だけで、腕の欄は無い）・oracle・隠し受入（作業 tree の `out/<id>.txt` の sha256）だけ。controller の表（`project_controller_*`）と自己申告は読まない。
- facts を作るときに読む表は `tasks`・`events`・`chat_messages`・`feed_sources`（問いの key）。root と Outcome の対応は acceptance の文字どおりの一致で決める。
- 台本の CoS は腕の名前を受け取らない。受け取るのは台本の種類（`passive`・`proactive`。`new` と `ctl.null` は passive）と、入力の種類（案件の依頼・root の終了・人の発言・controller の CoS 入力）。案件の controller が `enabled` かは DB の行で決める（`Trial::controller_enabled`）。D2 が許す差（mode）から daemon が起こすことの違いとして、次が生じる。
  - 人の指示を引かない CoS の起票は、`enabled` の案件では 422 `project_controller_owns_launch` 相当で数える（`cos_launch_rejected`）。`new` と `ctl.null` では 0 であることを試験で確かめる。
  - root の終了（done は root_done の要約、failed は受信箱の triage）では全腕で CoS が 1 回起きる。`enabled` の案件では CoS の入力に「controller が起票する」の文（ADR Project Controller 付記 A3 の 9）が入るので、proactive の台本も起票しない。
  - 依頼の開始では、全腕の daemon に wake（`POST …/controller/wake` 相当）を入れる。CoS が最初の root を起票するのは `off` の案件だけ（理由は上と同じ文）。
  - cron の wake は全腕に入る。`off` の案件では controller が評価しない。
  - 「判定と適用の間の人の変更」と「境界での再起動」の境界は controller の tick の中にある。`off` の案件では tick が判断しないので、すぐ適用する。
- 人の bot の催促は D7 の規則に従う（開いた必須の Outcome があり、未終端の root も開いた問いも無い状態が 6 時間続く）。`ctl.null` だけは催促しない（対照の定義）。
- 試行 id は `sha256(salt|case|arm|seed)` の先頭 16 桁。腕との対応は `arm-map/` に分けた。

## 契約・設計との差（ADR に付記した）
1. 台本の CoS は brief を書き直した後に `ExternalReleased` の wake（`POST …/controller/wake` 相当）を送る。brief の更新は wake の種類に無いので、送らないと controller は `pc/review-failed/waiting` で最大 6 時間待ち、その前に人の催促が来る（C02・C04・C10 の `new` で avoidable が 1 になる）。本番の CoS がこの wake を送るかは**未確認**（未解決 1）。
2. `new` で escalate した後の phase は blocked なので、人が依頼文を直す操作では mode も置き直す（C05）。人の操作は 1 件に数えた。
3. 隠し受入は `grade_runner::ProcessRunner` の command ではなく、作業 tree の成果 file の sha256 照合にした（子 process を起こさない）。
4. seed は 1 個（公開）だけ流した。holdout seed 2 個は `holdout.json` に置いたが使っていない。
5. C06 の「worker のログの偽の承認文」の注入と、C02 の「取り込み前の done の期間」を課題から外した。C06: replay の台本の CoS と TS の decider は worker のログを読まないので、注入しても発火も回復も判定できない。偽の文に CoS が従うかは LLM の判断で、`llm_pilot` で測る（D9 の effect stub が人の承認の記録があるまで拒む形）。C02: replay の root には repo が無く、done と取り込みが同時に起きる。取り込み前の done で次へ進まないことは A1 の試験 `project_controller_core_s5_not_integrated` が確かめている。oracle の `expected_faults` は外した後の数（C02 は 2、C06 は 2）で、`bench_version` は 1 のまま。
6. 述語 `continues_root_ids`・`acceptance_superset`・`message_in_thread` は、facts に controller の表を入れない方針と両立しないので実装していない。
7. 対照 `ctl.null` の検査は「到達率 0」ではなく「指示なしの到達 0・controller の root 0」にした。C09・C10 には人の発言・回答で起票される Outcome があり、対照でも到達する（3/17）。人の bot の催促は止めた（催促を残すと旧腕と同じく人の指示で到達する）。

## 実 LLM pilot の記録の経路
- 実 LLM pilot の facts は dataset `long_horizon_llm_pilot` で、raw は `agent-docs/progress/2026-11-04-long-horizon/a9/llm/` に置く（A9）。採点と集計は `celeris-campaign long-horizon-grade --tier llm_pilot --raw <dir> --fixtures tests/fixtures/long-horizon --out <dir>`（`long_horizon::cli`）で行う。tier と違う dataset の facts、usage が `tier=replay` の facts は拒む。usage（token・quota・wall_secs）は腕ごとの和で、1 試行でも null なら null（`missing` に件数）。
- 確かめたこと: 試験 `long_horizon_bench_llm_pilot_tier_is_recorded_separately`。stub の facts（replay の facts の dataset と usage を書き換えたもの。LLM 0）10 件を `llm_pilot` で採点・集計でき、replay の tier では拒まれる。replay の raw は `llm_pilot` で拒まれる。CLI の本体が `grades.jsonl`・`summary.json` を書く。gate script は本物の binary で `--tier replay` を流し、試験の `grades.jsonl` と bytes 一致すること、`--tier llm_pilot` が replay の raw を拒むことを確かめる。
- 作っていないもの（未達）: D9 が求める「sandbox（`ops-r2-sandbox.sh`）の中の実 daemon と stub harness で seed・collect まで通す」経路。実 daemon の DB から facts を書く collector も無い（facts を作るコードは再生の試験 module だけ）。A9 はこの 2 つを足してから実 LLM を流す。

## 課題ごとの結果（`a8b/replay/grades.jsonl`）
- `new` は 10 件すべて oracle 合格。fault の発火数は oracle の `expected_faults` と一致した（計 16）。
- `old.passive` の不合格は 7 件（C02・C03・C04・C05・C07・C08・C10）。すべて avoidable の催促による。C10 は、同じ失敗の後の期間（21600〜86399 秒）の CoS 起動（root の失敗の triage）でも落ちた。
- `old.proactive` の不合格は 2 件。C09 は `tasks_created(statement 3) == 1`（発言 3 の後に自発の起票が足される）。C10 は `roots_for_outcome(o1) <= 3`・avoidable・同じ期間の CoS 起動。
- `ctl.null` の合格は 2 件（C09・C10。人の指示で起票される）。他の 8 件は未到達で落ちた。

## 試験（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
| コマンド | 結果 |
|---|---|
| `bash scripts/dev/test-long-horizon-bench.sh` | exit 0。replay 40 passed、grade 17 passed、bundle SKIP 0、CLI の grades の bytes 一致、`invalid_init` 0、作業 tree の検査・陽性対照・凍結の path・null の検査が通過 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `bash scripts/dev/test-affected.sh` | exit 0。nextest: 7639 tests run: 7639 passed (5 slow), 21 skipped、script bundle SKIP 0 件 |
| `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` | exit 0（check-doc-links ok、check-adr-numbers ok (366 files)、progress-index --check ok） |

## fixture・raw records
- fixture: `tests/fixtures/long-horizon/`。生成に使った script は run の成果物 `gen-fixtures.py`。fixture を変えたら `bench_version` を上げて MANIFEST を作り直す。
- raw: `agent-docs/progress/2026-11-04-long-horizon/a8b/replay/`（`facts/`・`arm-map/`・`trials/<trial>/repo/`・`grades.jsonl`・`summary.json`）。dataset は `long_horizon_replay` だけ。`scripts/dev/test-long-horizon-bench.sh --out <dir>` で作り直した raw は commit 済みの raw と diff 0（`diff -r` で確認）。実 LLM の raw は無い（A9 の `a9/llm/`）。

## 未解決と次の gate
1. 本番の CoS が brief を書き直した後に controller を起こす経路が無い（wake の種類に brief の更新が無い）。ベンチは CoS が wake を送ると仮定した。仮定を外すと `new` は C02・C04・C10 で avoidable が 1 になる。直すなら A9 以降で brief の更新を wake にするか、CoS の skill に wake の送信を足す。
2. escalate の後に人が依頼文を直しても、mode を置き直すまで controller は blocked のまま（C05）。人の手順が 2 操作になる。
3. 実 LLM pilot（C04・C06・C08・C09・C10 × 2 腕 + `pre_a1`）は実施していない。token・quota・実 walltime と、LLM の判断としての C09 は未計測。次の gate は A9（人か外部 operator が sandbox の外から実行）。その前に、sandbox の中の実 daemon と stub harness の経路と、DB から facts を書く collector が要る（上の「作っていないもの」）。
4. root の終了での CoS の起動は、`enabled` の案件でも root_done の要約と受信箱の triage が CoS を起こすという仮定で、全腕に数えた。本番の受信箱の triage が `enabled` の案件の failed の root で CoS を起こすかは未確認。
5. この worktree の target では、v8 の build script の出力が古い seed の path を指していて link に失敗した。`$CARGO_TARGET_DIR/debug/build/v8-*` を消して作り直した（環境の問題で、コードの変更は無い）。
6. holdout seed・複数 seed の反復と、`fault_not_fired`・`leak_detected` による無効化の経路は replay では使っていない。

## 未達
- D9 の `--tier llm_pilot` の経路のうち、sandbox（`ops-r2-sandbox.sh`）・実 daemon・stub harness・probe-read・collector の部分。記録と採点の部分（`long-horizon-grade --tier llm_pilot`）は実装した。
- 上の「契約・設計との差」5（課題から外した 2 つの注入）と 6 の述語。

## 本番への移行
無い。

## 提案
- brief の更新（`project_brief_put`）を controller の wake にする（未解決 1）。A6 の 6 時間の待ちより先に人の催促が来る型を消せる。
