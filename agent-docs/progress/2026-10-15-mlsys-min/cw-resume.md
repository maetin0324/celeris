---
task: 01M4Z2B2HA2FEK1ZRWKS8MXSRR
unit: cw-resume
status: done
completed: 2026-10-15
---
# cw-resume: 秘密保存 event → 最小費用検証 → 一度だけ再開・誤キー処理・reconcile

ADR 2026-10-15-mlsys-min-decision-assisted D4 の名前（`waiting_for_auth`・`secret_stored`・集約キー `credential:<secret_id>`）をそのまま使い、新しい名前は足していない。

## 実装
- `PUT /api/v1/secrets/{id}` 成功時に `credential_record_secret_saved`（cw-core）で `secret_stored`（id と fingerprint のみ）を追記。重複 fingerprint は増えない。記録失敗でも PUT は成功のまま（reconcile が拾う）。
- 既知の秘密 id `jev_api_key`（provider `jev`）を `GET /secrets` の一覧に常に出す（`KNOWN_SECRETS`。schema は変えていない）。
- `crates/task-dispatch/src/credential_resume.rs`:
  - `CredentialVerifier`（provider adapter 境界の trait）、`JevVerifier`（OpenRouter Decisions API へ最小 1 呼び出し: 200→有効、401→誤キー、403→権限不足、402/429/5xx/通信失敗→一時障害）、`classify_http_status`。試験は `Scripted` mock（実ネットワーク・LLM なし）。
  - `CredentialResumer::process`: reconcile（`SecretSource` の fingerprint と記録の突き合わせ）→ 秘密・fingerprint ごとに 1 回検証 → 有効なら `mark_verified` → `claim_resume`（一度だけ）→ 同じ WU を `blocked` から `ready`/`needs_continuation`（checkpoint 有無）へ。誤キー/権限不足は `rejected`（同じ fingerprint は再検証しない。受信箱 item は `credential_waits` から導出されるので同じ item の文面が変わる）。一時障害は待ちを保ち、30s 倍々・上限 15 分の backoff（注入時計）。cancel 済み task は `claim_resume` が `cancelled` にして再開しない。
  - 再キューに失敗したら `credential_wait_release_claim`（cw-core の store に追加）で `verified` に戻し次の tick で再試行。
  - Dispatcher: `set_credential_resumer` と `tick_credential_waits`（開いた待ちが DB にあるときだけ別 thread で 1 つずつ実行。tick 自身は DB 一覧を読むだけ）。`celeris` の bootstrap で `[secrets]` があれば `jev` 検証器とともに結線。

## 証拠
- `cargo test -p task-dispatch credential_resume`: 11 件 pass（有効→一度だけ再開、誤キー→再試行なし→更新で再開、権限不足、429 等の backoff（時計注入）、重複 event と 2 resumer で二重再開なし、cancel 済みは再開しない、取り逃し reconcile、再起動復元、DB/event に値が出ない、HTTP 分類）。
- `cargo test -p task-api --test secrets_admin`: 12 件 pass（`put_records_a_secret_stored_event_without_the_value` を追加、既知 id の一覧を反映）。
- `bash scripts/dev/test-parallel.sh`: 5173 pass / 0 fail（exit 0）。`cargo clippy --workspace -- -D warnings`: exit 0。

## 未解決・提案
- **task 側の復帰**: 待ちで task 自体を `Blocked` にする規則（理由名・Trigger）は cw-enter の担当。ここは WU を戻すまで。task が `Blocked` で止まる設計なら、integrate-build-b で `credential_resumed` の WU 遷移に合わせた task の Ready 復帰（`ClusterJobResume` と同型）を足す必要がある。
- WU 遷移の blocked_reason は待ち入口側の値に依らず、`Blocked` なら戻す。
- claim 後・再キュー前の crash は、release_claim が走らないため WU が blocked のまま残りうる（極小の窓）。必要なら `resumed` で WU がまだ `blocked` の行を reconcile する。
- 判定済み fingerprint と backoff 状態はメモリ内（再起動後は最大 1 回再検証する）。
- Jev の header 名・body は ADR の「実装 unit が確認する」点で、実ネットワークでは未確認（Bearer 認証を使用。live 検証は鍵の登録後に人が行う）。登録は連携の有効化で、本測定や無制限課金の承認ではない（item 文言は cw-enter / cw-web で入れる）。
