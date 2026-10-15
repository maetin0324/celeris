---
title: "Jev 要求分類 adapter（crates/jev-classifier）"
tasks: [01M4ZD0W5XJ1KYP9MASNTX2NQ0]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# Jev adapter（unit jev-adapter / WU adapter）

ADR `2026-10-15-mlsys-min-decision-assisted.md` D1/D2/D4 の実装。実 API は呼んでいない（**実 API の統合試験は未実施**。fixed への fallback の試験と別の記録）。

## 認証 header（ADR D2 の未確認事項）

OpenRouter の一般 API 文書（https://openrouter.ai/docs/api-reference/authentication、2026-10-15 取得）: `Authorization: Bearer <OPENROUTER_API_KEY>`。
Decisions API 固有の別 header の記載は見つかっていない。実装は `bearer_auth` を使う。live-smoke で 401 が出たら再確認する。

## 実装

- `JevConfig`: endpoint・model_id（既定 `typesafe/jev-1.14`、`~` alias は `validate()` で拒否）・schema_version・prompt_version・timeout_ms・max_retries・backoff・cache_max_entries・breaker 閾値/cooldown・secret_id。未知欄は読み込みエラー。
- 型付き狭い質問: 手順確定度（specified / needs_implementation / needs_investigation_design / insufficient_info）、作業の性質、失敗種別（直近失敗があるときだけ）。成功率は聞かない。
- `ClassifyInput`: 指示・受入条件・必要成果物・依存成果の要約・直近失敗のみ。長さと件数の上限で切り詰め、秘密らしき値（sk-/ghp_ 等の prefix、Bearer、KEY=値、長い blob、メール）を `[redacted]` に。genre・check 有無等の構造化情報は入力に型として存在しない。
- `JevClassifier::classify(input, &dyn ApiKeyProvider) -> Result<ClassifyOutcome, JevError>`。
  - 401/403 → `JevError::Auth{status, secret_id}`（retry・breaker 計数なし）。鍵未登録 → `JevError::KeyMissing{secret_id}`。
  - timeout/429/5xx/無効応答/未知分類 → 指数 backoff の bounded retry（sleep は `Sleeper` 注入）→ `FallbackToFixed{reason}`。他の 4xx は retry せず fallback。
  - 連続失敗（classify 呼び出し単位）で開く `CircuitBreaker`（時計は `Clock` 注入、half-open 失敗で即再 open）。開いている間は呼ばず `BreakerOpen`。
  - 入力 hash（payload + 版の sha256）の bounded FIFO cache。成功だけ入れる。
- 秘密: `ApiKey` の Debug は伏せ、Display なし。エラーは secret id と status のみ。reqwest のエラー本文は捨てて種別だけ残す。
- dispatcher・store には接続していない（後続 unit の分類段が使う）。

## 証拠

- `cargo test -p jev-classifier`: 25 passed（config/breaker/cache/redact/parse の純粋部 + ローカル TCP の簡易 server で 成功・cache・401/403・鍵未登録・429/503/500 の backoff と breaker・不正 schema/未知分類→retry→成功・timeout）。
- `cargo clippy -p jev-classifier --all-targets -- -D warnings`: exit 0。
- 送信 payload に key・`token=…` の値が出ないことを試験（`success_sends_bearer_and_redacted_payload_then_caches`）。

## 未解決事項

- 応答 JSON の形（`answers.<name>.{choice,probabilities,confidence}`、`usage.cost`）と question の `criteria` の形は ADR の記述からの想定。live-smoke 前に公式ページで再確認する。
- `/api/alpha` の rate limit・error code は公式に記載なし。
- fake server の結合試験（別 WU `fake-server`）は tests/ に置かれる想定。ここの in-crate 試験と重複してよい。

## 提案

- 分類段が `ApiKeyProvider` を secrets_dir から実装し、`JevError::Auth/KeyMissing` を credential-wait（`waiting_for_auth`）へ接続する。
