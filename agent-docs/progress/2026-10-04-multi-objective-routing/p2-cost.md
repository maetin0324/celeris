---
task: multi-objective-routing
unit: p2-cost
phase: 2
status: done-in-branch
date: 2026-10-05
---

# p2-cost: SourceState の動的状態・effective cost・score の純粋関数

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §4 の純粋関数を `crates/task-core/src/model_router/cost.rs`（新規）に実装した。
範囲は `model_router/` の中だけ（`mod.rs` に `pub mod cost;`、`policy.rs` に定数、`profiles.rs` に欄）。

## 実装内容

- `profiles.rs`（additive、serde 既定で旧 JSON は読める）
  - `QuotaWindow` に `window_duration_s`・`estimated_consumption`・`reserve_value_usd`（いずれも Option）。
  - `SourceState` に `cooldown_until`・`rate_limited_until`（RFC3339、Option）。
- `policy.rs`
  - `FreshnessPolicy { observation_ttl_seconds }`（既定 300 秒、validate で有限・正を検証）。
  - 窓長の定数 5h / weekly / monthly と `default_window_duration_seconds(window_id)`（five_hour/5h、seven_day/weekly、monthly）。
- `cost.rs`
  - `estimate_cost(&CostInputs) -> Result<CostEstimate, &str>`: cash（API のみ。価格未登録は None）、subscription shadow（`max_j(reserve×u×h)`、一窓でも unknown なら全体 unknown）、self-host resource（率が無ければ unknown）、effective（必要成分の欠測は None）、pressure（既知成分の max、各成分 clamp）。
  - `exclusion_reasons(state, now, freshness)`: cooldown・rate_limit の終了前、新鮮な観測の残量 0 窓（`quota_exhausted`）。
  - `score(policy, quality, estimate, latency_ms) -> Result<ScoreBreakdown, &str>`: `wq·Q − wc·C − wl·L − wp·P`。C/L は参照値（policy.normalization）で割って 1 で頭打ち。未知の C/L/P は 1 として `unknown` flag を残す。品質の未知・不合格は Err。
  - 時計は引数（`now`）で注入。`Instant`/`SystemTime` は読まない。
  - 観測の鮮度: 観測時刻が無い・TTL 超過・未来・reset 境界超過の窓は「既知」でない（満タン扱いしない、古い 0 も枯渇とみなさない）。
  - 数値は非負・有限を検証（NaN・負は `invalid_estimate`、分母 0 は `zero_denominator`）。

## 試験

- `model_router::cost::tests::routing_effective_cost_distinguishes_cash_shadow_and_resource`（PASS）
  - 残量 0.5→0.1 で shadow 5 倍、reset までの時間が半分で shadow 半分、同入力は同出力。
  - API の cash は窓・queue の状態に依らず不変、単価未登録は None（0 ではない）。
  - self-host は queue 待ちの増加で resource と pressure が増える。率が無ければ resource・effective は None。
- `model_router::cost::tests::routing_source_state_stale_quota_is_unknown`（PASS、偽時計）
  - TTL を 1 秒超えた観測は shadow None、除外なし（満タン扱いしない）。
  - reset 境界を越えた窓は shadow None。古い観測の 0 は除外しない。
  - 新鮮な 0 窓が 1 つでもあれば `quota_exhausted` で除外。
  - 観測時刻なし・未来の観測は既知にしない。NaN・分母 0 は拒否。
- `model_router::cost::tests::routing_score_unknown_terms_rank_as_worst_and_keep_flags`（PASS）

## 検査の結果

- `cargo test -p task-core model_router`: 7 passed, 0 failed（既存 5 件の kernel・legacy 試験も通過）。
- `cargo test -p task-core`: 725 passed, 0 failed。
- `cargo clippy -p task-core --all-targets -- -D warnings`: exit 0。
- 変更は `crates/task-core/src/model_router/` の 3 ファイル変更と 1 ファイル新規のみ。

## 未解決・後続の段への申し送り

- 本 unit は純粋関数だけ。dispatch・proxy への接続、state の組み立て、config 配線（reserve_value・resource 係数・観測 TTL）は後続の段（dispatch-enforce・proxy-select・config）。
- `SourceState` の JSON schema（API 公開）と gui/web の生成型は schema の段で更新する。本 unit の欄追加は API に出ていないため生成物は未更新。
- `optimizer.rs` の既存 score（`optimize`）は未変更。新 score への切替は後続段の判断。
- `estimated_consumption` は窓ごとの 1 呼出し見積り。呼出し単位の推定は後続の段で入れる。

## 範囲 check の不合格（前回 run の後）

- 前回 run の後に celeris が走らせた check `test -z "$(git diff --name-only $(git merge-base HEAD main) -- crates | grep -v '^crates/task-core/src/model_router/')"` は exit 1。
- 原因: `git merge-base HEAD main` は 33774b6a。HEAD（305c64b6）は Phase 1 の統合済み commit を含み、main にはまだ無い。そのため基点から見た差分に Phase 1 の crates（celeris・llm-proxy・task-api・task-dispatch ほか 40 ファイル）が入る。この unit の変更は model_router の 3 ファイルと新規 1 ファイルだけ。
- 確認: `git diff --name-only 305c64b6 -- crates | grep -v model_router/` は 0 件。この unit の変更は model_router/ の中だけ。
- check の直し方（replan 側）: 基点を worktree の base（305c64b6）にする。例: `git diff --name-only 305c64b6 -- crates`。または worktree.json の base を使う。merge-base with main は Phase 1 の統合後は使えない。
