---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
status: done
updated: 2026-10-05
---

# RouteLLM sidecar の shadow 評価（Phase 5）

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §10 Phase 5 の記録。外部 estimator を
sidecar adapter（`[model_routing.estimator.sidecar]`、既定 off・`shadow_only = true` 必須）で Phase 4 の
shadow 評価に差し込めるかを確かめる。手順は [docs/ops/model-routing-migration.md](../ops/model-routing-migration.md) §10。

**実 sidecar の start-stop と上限付き shadow は人（Fable）が本番 host で実行した（2026-10-05、内部 shadow 評価に限る）。**
原票は [model-routing-routellm-shadow/](model-routing-routellm-shadow/)（入口 `run-manifest.txt`）、転記は §3。
これは sidecar 単体の評価で、Celeris 本体の estimator shadow（coverage・`estimator_version_mismatch`・hard constraint
違反・primary 変更数）は本番 config の opt-in（手順書 §10.7、人の別判断）が要るため**未計測**。§1 の偽 sidecar の結果は
配線と protocol の検証で、その未計測の欄を埋めるものではない。

## 1. 偽 sidecar の結果（外部ネットワークなし）

### 1.1 結合試験: adapter → estimator shadow → export → evaluate

`cargo nextest run -p celeris --no-fail-fast -E 'test(routing_estimator_sidecar_plugs_into_shadow_eval)'`
（`crates/celeris/tests/routing_estimator_shadow.rs`）→ 1 test run: 1 passed。

構成: 一時 DB、in-process の llm-proxy（127.0.0.1:0）、偽の上流 2 本、loopback の偽 sidecar、
固定時計（`FixedClock`）。`daily_max_requests = 3`、allowlist は全対象。run を 4 本流した。
回避策は使わない: `RoutingDecided` は run の前に dispatch 側の decision id（`dec-run-<i>`）で残し、pin は
正直な descriptor（`route-test` / `1`）で行う。proxy の要求記録（`routing_request_decided`）は daemon の
`TaskProxyEventSink` と同じ手順で試験内に鏡写しした sink が追記する。

| run | sidecar の応答 | estimator shadow の記録 |
|---|---|---|
| 0 | 完了（heuristic primary と別の候補に高い index） | completed |
| 1 | `request_id` の違う応答（protocol 違反） | failed（`upstream_error`） |
| 2 | 完了 | completed |
| 3 | 日次上限（3）に達した後 | dropped（`cap_exceeded`）、sidecar へ送信しない |

確かめたこと:

- primary の決定は変わらない: 4 本とも応答は primary の上流から 200、dataset の `primary_model` は
  `RoutingDecided` のまま。
- 上限超過 0: sidecar への送信は 3 回（= 上限）、4 本目は送る前に落ちる。
- hard constraint 違反 0: `legacy` を含む全 policy の `constraint_violations = 0`。
- DB に `routing_shadow_recorded`（kind = estimator、`policy_version = estimator:route-test/1`）が 4 件。
- `routing_replay::export` → `evaluate_with_estimator`（`celerisctl routing export` / `evaluate --policy estimator`
  と同じ関数）の `estimator_comparison`:

| 欄 | 値 |
|---|---|
| target_decisions | 4 |
| evaluated / coverage | 2 / 0.5 |
| completed / failed / timeout / dropped / prompt_required | 2 / 1 / 0 / 1 / 0 |
| observed_estimator_versions | `estimator:route-test/1`（`estimator_version_mismatch` 0 件） |
| differs_from_heuristic / same_as_heuristic / same_as_primary | 1 / 1 / 1 |
| quality_observed / quality_unknown | 0 / 2 |
| incomparable_reasons | `upstream_error`: 1、`cap_exceeded`: 1、`unselected_model_outcome_unknown`: 1、`outcome_missing`: 1 |
| unknown_reason | estimator は選ばないので、primary 以外の選択の結果は観測されない（品質は未知） |

### 1.2 実プロセスの wrapper（偽 classifier）と task-core 検証

`sh scripts/model-routing/fake-shadow-check.sh` → exit 0。

`routellm_sidecar.py --fake-classifier` を 127.0.0.1 の ephemeral port で起動し、`/healthz` から descriptor を
組み、共有 fixture `crates/task-core/tests/fixtures/estimator_sidecar_v1/request_valid.json`（prompt なし）と
prompt 付きの 2 要求を送った。応答 2 件とも task-core の `EstimateResponseV1::validate` を通過
（`crates/celeris/tests/routing_estimator_sidecar_validate.rs`、`request_id` を変えた応答は拒否されることも確認）。
prompt なしは `prompt_required`、prompt 付きは `uncalibrated_pair_score`・`raw_pair_win_rate=0.75`（偽 classifier の
固定値）。SIGTERM 後に exit 0、PID 消滅、port 閉鎖を確認。

## 2. 偽 sidecar で見つかった食い違い（解決済み）

shadow-check で見つけた 3 点は shadow-join unit で直した。結合試験（§1.1）は回避策なしで通る。

1. **decision id の結合（export 側で対応表から引く）**: estimator shadow の `primary_decision_id` は今も
   proxy の要求ごとの decision id（`pdec_…`）のまま記録する（sidecar が採点した候補列は proxy の決定のもの）。
   `routing_replay::export` は task ごとに先に対応表（`routing_request_decided` の `decision_id` →
   `parent_decision_id`、stage = proxy の `RoutingDecided` も同様）を作り、shadow を記録どおりの id か、
   対応表で引いた dispatch の decision id の行に付ける。proxy が親を知らなかった記録は run id などで推定して
   結ばない（行に付かない）。対応表は本走査の前に集めるので、shadow が要求記録より先に追記されても結ぶ。
2. **estimator version の照合**: `crates/task-ops/src/routing_replay/estimator.rs` の `version_matches` が
   daemon の記録形式 `estimator:<id>/<version>` を受けるようにした。正直な pin（`route-test` / `1`）で一致し、
   別 version（`2`）の pin では `estimator_version_mismatch` のまま（試験
   `routing_estimator_shadow_joins_daemon_records`）。
3. **model の名前（profile id へ正規化）**: llm-proxy の estimator shadow は、shadow kernel が catalog の
   deployment から引いた model profile id（sidecar が採点した名前と同じ）を `candidate_model` に記録する
   （`KernelChoice.chosen_model_profile_id`）。上流の wire model 名は記録しない。evaluate は shadow の model が
   その行の候補の profile id に無ければ `estimator_model_unknown` と数え、heuristic・primary との一致にも
   食い違いにも数えない（旧形式の記録が wire 名で残っていても一致扱いにならない）。

残る前提: dispatch の候補・`primary_model` と proxy の catalog が同じ profile id 体系（daemon は両方とも
legacy 設定の正規化 `legacy:<family>:<wire>` から作る）であること。食い違えば `estimator_model_unknown` に出る。

## 3. 実 sidecar の結果（人の実行、2026-10-05）

人の決定 `routellm-weights-use`（2026-10-05）: `routellm/bert_gpt4_augmented` は**内部の shadow 評価に限り**使ってよい
（再配布・公開なし、外部発表前に人が license を再判断）。実行は人（Fable、承認者 rmaeda）が本番 host で
[docs/ops/model-routing-migration.md](../ops/model-routing-migration.md) §10.4〜§10.6 と `scripts/model-routing/real-sidecar-check.sh`
に沿って行った。本番の daemon・DB・`~/.config/celeris` には触れていない。以下は原票
[model-routing-routellm-shadow/](model-routing-routellm-shadow/) からの転記。

### 3.1 実行コマンドと exit

共通 env: `CELERIS_ROUTELLM_REAL=1`・`CELERIS_ROUTELLM_WEIGHTS_DIR=<weights-dir>`・`OMP_NUM_THREADS=4`・
`CELERIS_ROUTELLM_STRONG=legacy:claude:opus`・`CELERIS_ROUTELLM_WEAK=legacy:qwen:qwen3.8-27b`（worktree HEAD `5c9930b0`）。

| 検査 | コマンド | exit | 結果 | 原票 |
|---|---|---|---|---|
| start-stop | `/usr/bin/time -v sh scripts/model-routing/real-sidecar-check.sh start-stop` | 0 | ready・finite pair score 1・0 failed、SIGTERM で sidecar exit 0、stop 後 port 閉鎖。wall 15.01 s、peak RSS 2643756 kB | `start-stop.log` |
| 上限付き shadow | `/usr/bin/time -v sh scripts/model-routing/real-sidecar-check.sh shadow --dataset <scratch>/dataset --max-requests 50 --out docs/reports/model-routing-routellm-shadow` | 0 | 30 finite pair score・0 failed、停止後 port 閉鎖。wall 5.94 s、peak RSS 2623488 kB | `shadow.log`・`real-sidecar-results.json` |

### 3.2 件数・上限消費・失敗

| 欄 | 値 |
|---|---|
| dataset | 合成の `/estimate` v1 要求 30 件（easy 10・medium 10・hard 10、人が作った試験用 prompt。本番の prompt・利用者の入力は含まない）。各 file の hash は `dataset.sha256` |
| 上限と実消費 | `--max-requests 50` に対し送信 30（≤ 50、上限超過 0） |
| completed / failed / timeout / dropped | 30 / 0 / 0 / 0（script は timeout を failed に数える。failures は空） |
| 外部呼び出し | 0（sidecar は `HF_HUB_OFFLINE=1`、loopback のみ） |
| `/estimate` 応答時間（client 側、30 件） | mean 44.62 ms・p50 31.57 ms・p95 42.93 ms・max 426.33 ms（初回の要求） |
| raw_pair_win_rate の平均（未校正） | easy 0.4003（0.1581–0.5551）・medium 0.4543（0.2632–0.6671）・hard 0.4731（0.2907–0.6525） |
| hard constraint 違反数・primary 変更数 | **未計測**（Celeris 本体の evaluate が要る。§3.4） |
| coverage・heuristic との差・`estimator_version_mismatch` | **未計測**（同上） |
| 品質（acceptance success）・費用への効果 | 未計測（estimator は選ばないので本番 outcome は観測されない） |

raw score は難しさの順（easy < medium < hard の平均）に並ぶが、範囲は重なり、校正していない。品質向上の根拠にはしない。

### 3.3 依存・weights・機材

| 欄 | 値 |
|---|---|
| RouteLLM | 0.2.0 @ `0b64fdafe049e596a3f5657c219329f24af24198`（`requirements.lock` と一致） |
| 依存 | Python 3.11.15・torch 2.3.1+cpu（CPU wheel、sha256 を index の値と照合）・transformers 4.41.2・litellm 1.60.0・numpy 1.26.4。freeze は `venv-freeze.txt`（71 行） |
| weights | `routellm/bert_gpt4_augmented` revision `86237e3df400762178ea98379477b8296e66d5e4`、weights-sha256 `6ec0b06c8af3c1b11aaefccb55f51f01ab531f80e276a7ad283607eec279cae5`（手順書 §10.1 の式）、tokenizer-sha256 `06112d98f5dd4e57a3aa9ee546d938a7c671b99ae5e25eaa9ef6b411ce15b492`。各 file は `weights-files.sha256` |
| 取得範囲 | 推論に要る file だけ（学習状態 optimizer.pt 等は取得せず）。weights・venv は原票に含めない（再配布しない） |
| 模型 | XLM-RoBERTa の sequence classification（`model.safetensors` 1112208084 bytes）。手順書 §10.5 の旧目安（BERT-base 相当・約 0.45 GB）と違うため §10.5 を実測値で直した |
| license の観測 | model card に license 宣言なし（cardData・tag なし）。repo に Apache License 2.0 本文の `LICENSE` file あり。人の決定（内部評価に限る）は変えず、外部発表前の再判断の材料とする |
| 機材 | AMD Ryzen Threadripper PRO 3945WX（12 core / 24 thread）、RAM 110 GiB、GPU 不使用、`OMP_NUM_THREADS=4`。peak RSS 約 2.6 GB |

手順書 §10.1 の block は `approved`（full revision・checksum 記入済み）で、`check-runbook.sh --require-approved` は exit 0。

### 3.4 未計測のもの（偽 sidecar で埋めない）

Celeris の routing 監査と `celerisctl routing evaluate --policy estimator`（coverage・`estimator_version_mismatch`・
hard constraint 違反・primary 変更数）は、本番 config で `[model_routing.estimator.sidecar]` を opt-in しないと出ない
（手順書 §10.7）。opt-in は人の別判断で、この Phase では行っていない。§2 の結合 3 点は実測の前に直してあり、
偽 sidecar の結合試験（§1.1）では回避策なしで coverage が出る。本番切り替え（estimator の primary 化）はしない。

## 4. close 時点の確認（2026-10-05）

- 原票: 人の commit `3c1a0f5c` が `docs/reports/model-routing-routellm-shadow/` に置いた → §3 に転記。
- `sh scripts/model-routing/check-runbook.sh --require-approved` → ok（`routellm-weights-use=approved`）。
  `sh scripts/model-routing/fake-shadow-check.sh` → exit 0。
- worker 内の `sh scripts/model-routing/real-sidecar-check.sh` は weights が無いので exit 2（未実行）。実行の証拠は人の原票。
- §2 の食い違い 3 点は shadow-join unit で解決済み（`integrate wu/shadow-join` `3687abbb` を close branch に取り込み済み）。
