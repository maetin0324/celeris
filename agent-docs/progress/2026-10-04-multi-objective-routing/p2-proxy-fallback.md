---
task: multi-objective-routing
unit: p2-proxy-fallback
phase: 2
status: done-in-branch
date: 2026-10-05
completed: 2026-10-05
---

# p2-proxy-fallback: proxy の同一要求内 fallback（stream 前だけ）と deployment の half-open

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §5 の proxy 側。変更は `crates/llm-proxy/` の中だけ。

## 実装内容

- `src/fallback.rs`（新規、`pub mod fallback`）: 純粋な部品。時刻は全部引数で受ける。
  - `FailureClass::of(&SourceError)`: 401=unauthorized / 429=rate_limited / 5xx=server / network / local（資格情報・source 無し）/ client（その他 4xx）。breaker に数えるのは server と network だけ（401/429 は既存の account cooldown）。
  - `RetryLimits`（分類別の「倒してよい回数」と `total_attempts`）・`FallbackBudget`（分類別上限 → 総上限 → deadline の順に判定）。既定は unauthorized/rate_limited/local 3、server/network 2、client 0、総 6、deadline 120 秒。
  - `RequestConstraints::from_request`: `claude/…`・`gpt/…`・`qwen/…` は自分の source だけ、`celeris/…` は全部。候補が制約の外なら送らずに飛ばす。
  - `Breakers`（deployment = `<source label>/<upstream model>`）: closed →（連続 `failure_threshold` 回）open →（`open_secs` 後の最初の 1 件）half_open の試し打ち → 成功で closed / 失敗で再 open / 401・429 は枠を返して half_open のまま。試し打ちは `Permit` が持ち、同時 1 件。結果を返さずに drop（要求の取り消し）したら枠だけ返す。`failure_threshold=0` で無効。
- `src/server.rs` の `chat_completions`: 候補ごとに 制約 → breaker の admit → `budget.begin_attempt()` → 送信。stream は最初の item を待ち、`Err` なら「送る前の失敗」と同じに扱って次の候補へ、`Ok` なら先頭へ戻して caller へ流す（以後の失敗は `finalize_stream` が切るだけで再送 0 回）。失敗は `permit.failed_with` と既存の `record_failure`（account cooldown）に渡し、`budget.after_failure` が `Stop` なら最後の失敗を返す（429 の Retry-After はそのまま）。
  - 1 回も送れなかった（制約の外・breaker が open/試し打ち中）ときは 503 `no_source_available`、Retry-After は最も早く開く breaker まで（無ければ 5）。
  - no-source 再走査（`NO_SOURCE_MAX_RESCANS`）と account cooldown の経路は変えていない。
  - `ProxyState::with_fallback(settings, clock)` で設定と時計（`reservation::Clock`）を差し替える。既定は `FallbackSettings::default()` と `SystemClock`。config への配線は config unit。
- 動作の変更点: 401/429 以外の 4xx（client）は次の候補へ倒さない（既定 `client: 0`。別の候補でも直らない要求の誤り）。

## 試験

- `src/fallback_tests.rs`（単体 5 件）: 分類、分類別・総上限・deadline（境界ちょうど）、breaker の遷移と試し打ち同時 1 件・drop で枠を返す、threshold 0、制約。
- `tests/proxy_fallback.rs::routing_proxy_fallback_preserves_constraints_and_stream_boundary`（偽 relay と、呼ばれた数を数える偽 Anthropic。時計は手動）:
  1. 401 → 429 → 503 → 成功で 4 番目の relay から返る。`qwen/cheap` の制約で Claude は 0 回。
  2. 全部 5xx: server 上限 2 で 3 件目の後に止まり 4 件目は 0 回、Claude 0 回。
  3. `total_attempts=2`: 401・429 の後は健全な 3 件目へ送らず、429 と `Retry-After: 7` を返す。
  4. `deadline_secs=0`: 最初の失敗で止まり、次へ倒さない。
  5. stream: 最初の byte の前に壊れたら次の relay（`[DONE]` まで届く）、最初の byte の後に壊れたら同じ relay のまま切れて再送 0 回。
  6. breaker: 1 回の 5xx で open（until=T0+30）、T0+29 は送らない、T0+30 で試し打ち。偽上流の中で試し打ちを止め（`arrived` を待つ。sleep なし）、その間の要求は次の relay へ（試し打ちは 1 件だけ）。試し打ち成功で closed。
  7. 候補が全部 open: 送らずに 503、`Retry-After: 20`（T0+10 で until=T0+30）。
- stream の先読みを外す変異を入れると 5 の前半で落ちることを確認した（変異は戻した）。

## 証拠

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p llm-proxy routing_proxy_fallback` | 1 passed |
| `cargo nextest run -p llm-proxy`（変更前の既存 80 件、server.rs 変更直後） | 80 passed |
| `bash scripts/dev/test-parallel.sh` | exit 0、Summary「3955 tests run: 3955 passed (1 slow), 12 skipped」 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo clippy -p llm-proxy --all-targets -- -D warnings` | exit 0 |

既存の `claude_429_falls_back_to_the_next_account_and_records_a_cooldown`・cheap の Qwen 優先/fallback 試験は変えずに通っている。

## 未解決事項

- `FallbackSettings` の config 配線（`[model_routing]` の retry・breaker）は config unit。
- breaker の状態は proxy の in-process だけ（再起動で closed に戻る）。sources API への表示は api unit の判断。
- proxy-select の `select_state`／予約表は server.rs にまだ配線していない（integrate-runtime 以降）。fallback の候補順は legacy の `attempts_for` のまま。

## 提案

- config unit で `client` 上限を 0 以外にしない（4xx の誤りを別候補に投げると quota を無駄にする）ことを検証に入れる。
