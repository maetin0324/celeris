---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
status: awaiting-human
updated: 2026-10-05
---

# RouteLLM sidecar の shadow 評価（Phase 5・実評価は人の手順待ち）

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §10 Phase 5 の記録。外部 estimator を
sidecar adapter（`[model_routing.estimator.sidecar]`、既定 off・`shadow_only = true` 必須）で Phase 4 の
shadow 評価に差し込めるかを確かめる。手順は [docs/ops/model-routing-migration.md](../ops/model-routing-migration.md) §10。

**Phase 5 の実評価は未完了（人の手順待ち）。** 実 sidecar（RouteLLM の実 weights）の start-stop と上限付き shadow は
未実行で、原票（`docs/reports/model-routing-routellm-shadow/`）は close 時点（統合後 HEAD `18f60cef8b39`、2026-10-05）で無い。 下の「偽 sidecar の結果」は配線と
protocol の検証であり、RouteLLM の品質・費用・overhead の実測ではない。偽の合格で実測の欄を置き換えない。

## 1. 偽 sidecar の結果（外部ネットワークなし）

### 1.1 結合試験: adapter → estimator shadow → export → evaluate

`cargo nextest run -p celeris --no-fail-fast -E 'test(routing_estimator_sidecar_plugs_into_shadow_eval)'`
（`crates/celeris/tests/routing_estimator_shadow.rs`）→ 1 test run: 1 passed。

構成: 一時 DB、in-process の llm-proxy（127.0.0.1:0）、偽の上流 2 本、loopback の偽 sidecar、
固定時計（`FixedClock`）。`daily_max_requests = 3`、allowlist は全対象。run を 4 本流した。

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
| differs_from_heuristic / same_as_heuristic | 2 / 0 |
| quality_observed / quality_unknown | 0 / 2 |
| incomparable_reasons | `upstream_error`: 1、`cap_exceeded`: 1 |
| unknown_reason | estimator は選ばないので、primary 以外の選択の結果は観測されない（品質は未知） |

### 1.2 実プロセスの wrapper（偽 classifier）と task-core 検証

`sh scripts/model-routing/fake-shadow-check.sh` → exit 0。

`routellm_sidecar.py --fake-classifier` を 127.0.0.1 の ephemeral port で起動し、`/healthz` から descriptor を
組み、共有 fixture `crates/task-core/tests/fixtures/estimator_sidecar_v1/request_valid.json`（prompt なし）と
prompt 付きの 2 要求を送った。応答 2 件とも task-core の `EstimateResponseV1::validate` を通過
（`crates/celeris/tests/routing_estimator_sidecar_validate.rs`、`request_id` を変えた応答は拒否されることも確認）。
prompt なしは `prompt_required`、prompt 付きは `uncalibrated_pair_score`・`raw_pair_win_rate=0.75`（偽 classifier の
固定値）。SIGTERM 後に exit 0、PID 消滅、port 閉鎖を確認。

## 2. 偽 sidecar で見つかった食い違い（未解決）

結合試験は次の 3 点を回避して通している。実 sidecar の評価の前に直す必要がある。

1. **decision id の結合**: proxy が記録する estimator shadow の `primary_decision_id` は proxy の要求ごとの
   decision id（`pdec_…`）で、`routing_replay::export` は `decision_id` が一致する行にだけ shadow を付ける。
   dispatch の `RoutingDecided` の decision id では 4 行とも shadow 0 件になる。試験は shadow の
   `primary_decision_id` で `RoutingDecided` を書いて回避した。
2. **estimator version の照合**: daemon は `policy_version = estimator:<id>/<version>` で記録するが、
   `crates/task-ops/src/routing_replay/estimator.rs` の `version_matches` は `<v>`・`<id>@<v>`・`<id>:<v>`・`<id>/<v>`
   しか受けない。正直な descriptor（`route-test` / `1`）で pin すると 4 件とも `estimator_version_mismatch`。
   試験は照合が直るまで `estimator:route-test` で pin し直す（直れば正直な pin の経路を通る）。
3. **model の名前**: shadow の `model` は上流の wire model 名、dataset の候補と `primary_model` は model profile id。
   名前が一致しないため `same_as_heuristic`・`same_as_primary` は意味のある値にならない（上の `differs = 2` は
   この食い違いを含む）。

## 3. 実 sidecar の結果

**未実行: 人の手順待ち。** 人の決定 `routellm-weights-use`（2026-10-05）: `routellm/bert_gpt4_augmented`（観測 revision
`86237e3df400`、HF に license 宣言なし）は**内部の shadow 評価に限り**使ってよい（再配布・公開なし、外部発表前に人が
license を再判断）。worker は外部ネットワークと weights 取得ができないため、実行は人（Fable）が本番 host で行い、
原票を `docs/reports/model-routing-routellm-shadow/` に置く。手順書 §10.1 の block は weights の full revision と
checksum が記録されるまで `pending` のまま。
手順は [docs/ops/model-routing-migration.md](../ops/model-routing-migration.md) §10.4〜§10.7、検査は
`scripts/model-routing/real-sidecar-check.sh`（設定と weights が無ければ exit 2 = 未実行。合格扱いにしない）。

| 欄 | 値 |
|---|---|
| RouteLLM の commit・weights の版 | 未実行: 人の手順待ち |
| 機材（CPU/GPU・RAM/VRAM）と peak 使用量 | 未実行: 人の手順待ち |
| coverage・heuristic との差・失敗理由 | 未実行: 人の手順待ち |
| overhead（mean / p50 / p95） | 未実行: 人の手順待ち |
| 品質（acceptance success）・費用への効果 | 未実行: 人の手順待ち |

実測後にこの節を埋め、§2 の食い違いが直っていることを併記する。opt-in（本番判断への反映）はこの Phase では行わない。

## 4. close 時点の確認（2026-10-05、統合後 HEAD `18f60cef8b39`）

- 原票: `docs/reports/model-routing-routellm-shadow/` は無い → 転記なし。§3 は「未実行」のまま。
- `sh scripts/model-routing/real-sidecar-check.sh` → `not run: set CELERIS_ROUTELLM_REAL=1 and CELERIS_ROUTELLM_WEIGHTS_DIR to approved local weights`（未実行。合格扱いにしない）。
- `sh scripts/model-routing/check-runbook.sh` → ok（`routellm-weights-use=pending`）。`sh scripts/model-routing/fake-shadow-check.sh` → exit 0。
- §2 の食い違い 1・2 は未修正（`crates/task-ops/src/routing_replay/estimator.rs::version_matches` は `estimator:<id>/<v>` を受けない）。
  このまま実 sidecar の shadow を export/evaluate すると、正直な pin では全件 `estimator_version_mismatch`、dispatch の
  decision id の行には shadow が付かず coverage 0 になる。実評価の前に直す（または実行時に `--estimator` の pin を
  `estimator:<id>` にし、export の結合を確認する）。

実行に使ったコマンド・exit・件数（N）・上限と実消費・completed/failed/timeout/dropped・hard constraint 違反数・
primary 変更数・外部呼び出し数・起動停止検査（PID 終了・port 閉鎖）を、原票を置いた後に §3 の表へ転記する。
