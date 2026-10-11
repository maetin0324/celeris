---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: prune-select: task-dispatch の旧 provider 選択器を消す
updated: 2026-10-11
---
# prune-select: task-dispatch の旧 provider 選択器を消す

失敗した dispatch-prune の前半（ADR 2026-10-11-llm-source-harness-selection、破壊的変更の方針）。

## 変更

- dispatch-prune の WIP patch（artifacts/dispatch-prune-wip.patch）を `git apply --3way` で当てた。
  `crates/task-dispatch/tests/cos_chat_triage.rs` だけは edges-fix の修正と衝突したので、patch と同じ削除を手で行った
  （StaticPolicy・ProviderSpec を組む旧 provider 経路の結合試験。prune-policy が型を消すと組めなくなる）。
  patch 由来の削除: `coding_default.rs`（model family から harness を逆算する既定）、旧 provider 試験
  `cheap_local_first`・`provider_and_retry`・`routing_and_quota`・`knowledge_fallback`・`browser_fallback`・
  `policy/tests.rs`・`tests/unified_kill.rs`。patch の進捗 `dispatch-prune.md` はこのファイルに移した。
- `dispatcher/provider_select.rs` を削除した。旧選択（provider の順位付け・sticky・account の採点と選択・
  lane/Tier からの model 再束縛・割り当てによる provider 除外）はすべて消した。
- まだ使う小 helper は `dispatcher/route_failure.rs` に移した: 供給側失敗の分類（`provider_failure_reason`・
  `provider_failure_outcome`・`cooldown_reason_name`）、API 語彙（`excluded_reason_name`・
  `account_cooldown_reason_name`）、`record_account_failure`、`adapter_for_account`、`provider_throttled_event`
  （最後のものは `Dispatcher.policy` を読むので prune-policy が扱う）。
- 削除で使われなくなった物を消した（`#[allow]` は使っていない）: `Dispatcher` の `account_pool_adapters` 欄と
  setter、ローカル行の health 判定、暗黙アカウント満杯判定、`sessions` の sticky 判定と tier 順位（とその試験）、
  dispatcher 試験の未使用 adapter 5 個と helper 2 個。
- celeris: coding 既定の材料を作る `Config::coding_harness_default` とその試験、daemon の起動・reload での
  `set_coding_harness_default`・`set_account_pool_adapters` 呼び出しを消した。
- 消した識別子を参照していた comment（routing_shadow・routing_members・sessions・model_routing）を直し、
  `docs/architecture-map.md` の行を route_select / route_failure に更新。`LaneResolution` の doc comment 変更に
  合わせ `UPDATE_SCHEMA=1` で `docs/api/v1/{api-v1,event}.schema.json` を更新した。

## 検証

- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `TMPDIR=/tmp cargo nextest run --no-fail-fast -p task-dispatch -p celeris`: 1325 passed, 1 skipped。
  （既定の長い TMPDIR では browser_doctor の unix socket path が SUN_LEN を超えて 1 件落ちる。既知の環境要因）
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`（schema 更新前）: passed 4975 / failed 2（schema drift 2 件
  `committed_schema_matches_generated`・`event_row_schema_matches_committed`）。`UPDATE_SCHEMA=1` で更新後、
  その 2 件を含む 8 件が pass。
- `cargo fmt --all -- --check`: 変更したファイルは差分なし（llm-proxy・routing_sidecar.rs の既存の fmt 差分は範囲外で未変更）。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は task-dispatch・celeris config/daemon・task-core の comment・docs/api/v1・architecture-map・この進捗のみ。

## 未解決・提案

- `web/api/generated/schema.json` は base の時点で既に `docs/api/v1/api-v1.schema.json` と食い違っている（`in_use_cos` が残る）。api-remove の再生成で合わせること。
- `routing_shadow.rs`・`routing_members.rs`・`routing_enforce.rs` は旧選択の shadow/enforce の殻（setter と型だけ）が残っている。
  config の `[model_routing]` と一緒に消す葉を提案する。task-core の `coding_harness`（model family → adapter）と harness の
  `adapter_policy = "model_family"` 欄も、読み手（dispatcher）を失ったので config-remove か後続で消す。
- `Config::account_pool_adapters`（celeris config/providers.rs）は読み手を失った。config-remove が providers と一緒に消す。

## 本番への移行

なし（config 欄は変えていない。dispatcher の内部選択の削除のみ）。
