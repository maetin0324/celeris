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

## fake server 結合試験（WU fake-server, 2026-10-15）

`crates/jev-classifier/tests/jev_fake_server.rs`（11 試験）。127.0.0.1 の tokio `TcpListener` の fake server のみ。実 API・外部ネットワークなし。dev-dependency の tokio に `test-util` を追加（時計 pause 用）。実装側の変更なし（不具合は見つからなかった）。

- 成功: 型付き分類・要求 1 回・Bearer 付与。
- 不正 schema／未知分類: 3 要求（1+max_retries）後に `FallbackToFixed{InvalidResponse|UnknownClassification}`。途中で正常応答なら成功。
- timeout: server 側の遅延フック（`Reply::Delay`、3600s）＋ `start_paused = true`。`Timeout` → fixed。待ちは仮想時間で CPU 負荷なし。
- 401/403: `JevError::Auth{status, secret_id}`、retry なし・sleep なし、閾値超えの 4 回呼んでも breaker が開かず毎回 server へ届く。
- 429/500/503: server 受信数 = 1+max_retries（max_retries=3 で 4）、sleep 3 回、その後 fixed。
- breaker: 2 回連続失敗で開き、開いている間は受信数が増えない。cooldown 前（59s）は BreakerOpen、後（61s）は半開で 1 試行が届き、失敗なら直ちに再 open、成功なら閉じる。
- 秘密: 入力に混ぜた key が要求 body に出ない。応答本文に key が反射されても error / Debug / fallback 理由に出ない（secret id のみ）。genre 等の構造化欄が body に無い。長大入力は body < 40KB に切り詰め。

### 証拠

- `cargo test -p jev-classifier --test jev_fake_server`（3 回繰り返し）: 11 passed / 0 failed、各 0.18s。
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0。
- `bash scripts/dev/test-parallel.sh`: nextest 5184 passed / 0 failed / 13 ignored、doctest 正常、`test-parallel: ok`（exit 0）。

### 未解決事項

- 実 API の応答形・認証 header は未確認のまま（上記）。live smoke は未実施。

### 提案

- credential-wait 接続点: 分類段が `ApiKeyProvider`（secrets_dir 読み出し）を実装し、`JevError::Auth` / `KeyMissing` を `waiting_for_auth`（ADR D4）へ渡す。その間の通常作業は `FallbackToFixed` 相当の fixed 方策で進める。
- live smoke は別 unit で（有料測定のため人の承認の下で。1 件の最小要求で 401 時の header 形と応答形を確認）。

## mock 結合試験のまとめと最終証拠（WU record, 2026-10-15）

mock は 127.0.0.1 の fake server（実 API・外部ネットワークなし）。覆う場面: 成功／不正 schema／未知分類／timeout（遅延フック＋仮想時間）／auth error 401・403（型付き `Auth`、retry なし）／rate limit 429／5xx（bounded retry 後 fixed）／breaker（open・cooldown・half-open）。失敗時は `FallbackToFixed` か `BreakerOpen` を返し、通常作業は fixed 方策で止まらない。送信 payload に秘密・genre 等の構造化欄・全履歴が入らないことも同ファイルの試験で確認済み。

実行した証拠（すべて本 WU で実行）:

- `cargo test -p jev-classifier`: exit 0。unit 25 passed + `jev_fake_server` 11 passed、0 failed。
- `bash scripts/dev/test-parallel.sh`: exit 0。nextest 5184 passed / 0 failed / 13 ignored、doctest ok、`test-parallel: ok`。
- `cargo clippy --workspace -- -D warnings`: exit 0。

### 未解決事項（再掲）

- credential-wait 接続点: `JevError::Auth` / `KeyMissing` を `waiting_for_auth` へ渡す配線は未実装（分類段の unit で）。
- live smoke は別 unit（実 API の応答形・認証 header は未確認）。

### 提案（再掲）

- 分類段で `ApiKeyProvider` を実装し credential-wait に接続する。live smoke は人の承認の下で最小 1 件。
