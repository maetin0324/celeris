# Phase browser-4: isolated runtime・trusted injection・backend routing（P4-A〜C）

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG]
---

- 状態: **P4-A/B/C の受け入れは未完**。runtime 方式・H7 は人の回答を採用済み（ADR-0085）。旧 broker IPC の秘密返却を廃止。実 runtime・CDP sink・backend 適合は継続実装が必要。本番未昇格。
- 更新: 2026-09-29
- ADR: [ADR-0084](../adr/0084-browser-phase4-isolation-injection-routing.md) D6、[ADR-0085](../adr/0085-browser-phase4-runtime-selection.md)

## 行ごとの判定

| 行 | 判定 | 証拠・限界 |
|---|---|---|
| P4-A isolated runtime | 未達。純関数検査だけ | `cargo test -p task-core browser_isolation` の前回結果は14 passed。worker から `bwrap_argv`・`verify_isolation`・`check_egress`・`orphan_groups` は未呼出。実 namespace・egress・killpg は検証できていない。 |
| P3-A identity 復元（隔離下のみ） | 未達。API の契約テストだけ | `cargo test -p task-api restore_is` の前回結果は2 passed。`restore_isolated` は稼働中 runtime に未結合。`--restore` / `--state` / `--profile` は利用しない。 |
| P4-B stronger injection | 未達。攻撃の純関数テストだけ | `cargo test -p celeris-credentiald injection` の前回結果は6 passed。実 CDP sink / IPC peer UID role は未接続。旧 plugin bridge と resolve.sock 自体を固定拒否に変更。有効 lease を持つ別 worker process の実 IPC 拒否を検証。実注入は未達。 |
| H3 観測停止の維持 | 実装維持。機密起動は拒否 | `cargo test -p task-worker browser --lib` → 31 passed。`browser_auth_section_forward_events_drops_progress_artifact_and_live` と LiveEmitter の抑止試験を含む。API 結合テストの store auth_section / takeover 拒否も成功。実注入中の end-to-end 検証はP4-A/B待ち。 |
| P4-C backend routing | 一部接続。公開操作の既存 loop 再利用と未適合機密要求の拒否 | worker browser 関連31件と `cargo test -p task-api --test browser_e2e` → 4 passed。`CredentialUse` を `CredentialInjection` 必須能力へ写像し、wait操作・承認消費・起動より前に拒否する。`IdentityRestore` にも `InjectionAttackSuite` を要求。静的 fixture 登録は実 backend の適合証拠ではなく、specialist・実 fixture runner・同一task実評価は未。 |

## attempt 3 の挙動と検査

- 公開操作だけの policy は ACP / 明示 Claude の既存 loop で実行する。`CredentialUse` を含む effective policy は、承認状態を問わず `browser backend lacks required conformance` で拒否する。task が公開操作を行うには管理者がその policy を明示する。要求能力を黙って落とさない。
- API 結合テストは Phase 2 から残った durable wait を store に作り、実API・brokerによる手動登録と一回承認後も worker が起動しないことを確認。承認は未消費、broker の grant/use は0件、run directoryも作らない。DB/WAL・vault・event・worker出力の sentinel 非露出も確認する。
- 旧 trusted-local の認証成功・redirect 中の注入・認証区間の注入成功を期待するテストは、現在拒否する挙動に合わせて更新した。新しい拒否テストを P4-B の TOCTOU/redirect/iframe/DOM 実攻撃試験や隔離実行の証拠として数えない。H3 の production `auth_section` と `forward_events` の配線は変更していない。
- `cargo test -p task-worker browser --lib` → exit 0、31 passed。
- `cargo test -p task-api --test browser_e2e` → exit 0、4 passed。
- `cargo test --workspace` → exit 0（2957 passed、0 failed、既存のignored 7件）。`cargo clippy --workspace -- -D warnings` → exit 0。workspace 検査に isolation 14件、backend 7件、injection 6件、auth_section 名の7件を含む。
- 初回の sandbox 内実行は sccache 接続が EPERM。Celeris の環境変数は変更せず、sandbox 外で同じ検査を実行する。結合テストの `owner_id` 型エラーを修正し再実行済み。

## 人の回答反映と旧 IPC 拒否（run 01M3QCTV524JJ41X9MSFS0756V）

- runtime は bubblewrap + subuid/subgid、host trusted controller/proxy の分離を採用。H7 は固定 agent-browser 0.38.1 + 既存 harness の browser-specialist を採用。方式・候補の回答待ちは解消し、再決定要求は出していない。
- 旧 `resolve.sock` は `trusted_injection_required` を返す。control 許可 PID、有効 binding/lease、JSON の `role: injector` 自己申告、同じ UID の別 worker process からも秘密を返さない。grant 2件に対し use 0件で lease 未消費を確認する。旧 bridge は socket に接続せず固定失敗を返す。
- API の実手動登録・承認・grant 後の旧秘密取得成功テストも拒否へ更新した。`CredentialProvider` と broker 内部の単回 lease の契約・試験、H3/auth_section 実装は変更していない。これらの拒否は実 trusted injection 成功や実CDP攻撃試験を意味しない。
- 初回の限定テストは sandbox 内 sccache が EPERM。環境を変更せず sandbox 外で実行。control 拒否前に request を読まない変更では Unix socket の未読データによる切断が `Io` となったため、既存の bounded read を維持して固定応答を返すよう修正した。
- 最終検査: `cargo test --workspace` → exit 0、2957 passed / 0 failed / 既存 ignored 7件。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all --check` → exit 0。
- workspace 実行に `retired_plugin_bridge_never_connects_or_echoes_input`、`daemon_rejects_worker_secret_retrieval_even_with_valid_lease`、`invalid_control_process_id_is_rejected`、API の `real_broker_registration_keeps_sentinel_out_of_db_events_artifacts_and_worker_output` を含み、すべて成功。P4-A の実 runtime と P4-B の実 CDP 攻撃、P4-C の実 backend 適合は未実施なので、workspace 成功をそれらの達成証拠とはしない。

## 未解決

- P4-A: 実 bwrap/subuid 起動・事実採取・broker/CDP/IPC 分離・filtering proxy・orphan 回収・稼働中隔離 session への identity 復元。純関数判定のみで受け入れない。
- P4-B: P4-A の後、別 injection-only IPC の SO_PEERCRED role/session 認可・実 CDP sink・実攻撃負例・H3 の端から端の検証。旧 endpoint は再開しない。
- P4-C: 選択済み specialist と既存 loop を実 fixture runner/同一 task 評価へ接続し、能力を失わない fallback を実行経路で検査する。機密機能は P4-A/B の実適合まで拒否する。
- 3件の継続小タスクを run の `delegate.json` に提案した。P4-B は P4-A に依存し、公開能力の P4-C は独立。採用・実行・完了はこの run では確認できていない。提案を実装済みとして数えず、親の受け入れ条件0/1/2は未達のままとする。
- 内部 origin の追加、本番昇格、リモート実行は行っていない。
