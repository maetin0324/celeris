---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
status: draft
updated: 2026-10-05
---

# RouteLLM sidecar の shadow 評価（Phase 5・草稿）

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §10 Phase 5 の記録。外部 estimator を
sidecar adapter（`[model_routing.estimator.sidecar]`、既定 off・`shadow_only = true` 必須）で Phase 4 の
shadow 評価に差し込めるかを確かめる。手順は [docs/ops/model-routing-migration.md](../ops/model-routing-migration.md) §10。

**実 sidecar（RouteLLM の実 weights）の評価は未実行: 人の手順待ち。** 下の「偽 sidecar の結果」は配線と
protocol の検証であり、RouteLLM の品質・費用・overhead の実測ではない。偽の合格で実測の欄を置き換えない。

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

## 3. 実 sidecar の結果

**未実行: 人の手順待ち。** 決定 `routellm-weights-use`（weights の利用承認）と実行環境の用意が要る。
手順は [docs/ops/model-routing-migration.md](../ops/model-routing-migration.md) §10.4〜§10.7、検査は
`scripts/model-routing/real-sidecar-check.sh`（設定と weights が無ければ exit 2 = 未実行。合格扱いにしない）。

| 欄 | 値 |
|---|---|
| RouteLLM の commit・weights の版 | 未実行: 人の手順待ち |
| 機材（CPU/GPU・RAM/VRAM）と peak 使用量 | 未実行: 人の手順待ち |
| coverage・heuristic との差・失敗理由 | 未実行: 人の手順待ち |
| overhead（mean / p50 / p95） | 未実行: 人の手順待ち |
| 品質（acceptance success）・費用への効果 | 未実行: 人の手順待ち |

実測後にこの節を埋める（§2 の 3 点は実測の前に直してある）。opt-in（本番判断への反映）はこの Phase では行わない。
