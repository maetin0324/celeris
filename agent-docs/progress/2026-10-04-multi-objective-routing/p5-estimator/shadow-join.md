---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: shadow-join
status: done
completed: 2026-10-05
---

# Phase 5 shadow-join: estimator shadow の結合 3 点（decision id・version 照合・model 名）

## 変更

- `crates/task-ops/src/routing_replay.rs`: export が task ごとに proxy decision → dispatch decision の対応表
  （`routing_request_decided` の `parent_decision_id`、stage = proxy の `RoutingDecided`）を先に作り、shadow を
  記録どおりの id か対応表の dispatch decision id の行に付ける。親不明は推定で結ばない。
- `crates/task-ops/src/routing_replay/estimator.rs`: `version_matches` が `estimator:<id>/<version>` を受ける。
  shadow の model が行の候補 profile id に無ければ `estimator_model_unknown`（一致・食い違いに数えない）。
- `crates/llm-proxy/src/estimator_shadow.rs`: `KernelChoice.chosen_model_profile_id` を足し、estimator shadow の
  `candidate_model` を wire model 名から model profile id に変えた。
- `crates/celeris/tests/routing_estimator_shadow.rs`: 回避策（shadow の `pdec_` で RoutingDecided を書く・
  `estimator:route-test` で pin し直す）を外した。RoutingDecided は run の前に `dec-run-<i>`、pin は正直な
  descriptor、`TaskProxyEventSink` の鏡写しの sink で要求記録を追記。
- 試験追加: `routing_estimator_shadow_joins_daemon_records`（task-ops）、llm-proxy の kernel_choice の assert。
- 文書: report §2 を「解決済み」に書き換え、ADR 末尾に付記（export 側の対応表を選んだ理由）。

## 証拠

- `cargo nextest run -p celeris --test routing_estimator_shadow --no-capture` → 1 passed。target 4・evaluated 2・
  observed `estimator:route-test/1`・`estimator_version_mismatch` なし・differs 1 / same 1 / same_as_primary 1
- `cargo nextest run -p task-ops -E 'test(routing_estimator) | test(routing_routellm) | test(routing_replay)'` → 5 passed
- `bash scripts/dev/test-parallel.sh` → exit 0、4035 tests run: 4035 passed, 13 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh` → ok

## 未解決事項

- 実 sidecar（RouteLLM weights）の評価は人の手順待ち（決定 routellm-weights-use、report §3）。
- dispatch の候補と proxy catalog の profile id 体系が食い違う環境では `estimator_model_unknown` に出る（実測で確認する）。
