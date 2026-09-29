# Phase browser-4: isolated runtime・trusted injection・backend routing（P4-A〜C）

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG]
---

- 状態: **P4-A/B/C の受け入れは未完、runtime 方式・H7 の決定待ち**。attempt 3 は未適合の機密要求を worker の起動経路で拒否する修正。本番未昇格。
- 更新: 2026-09-29
- ADR: [ADR-0084](../adr/0084-browser-phase4-isolation-injection-routing.md) D6

## 行ごとの判定

| 行 | 判定 | 証拠・限界 |
|---|---|---|
| P4-A isolated runtime | 未達。純関数検査だけ | `cargo test -p task-core browser_isolation` の前回結果は14 passed。worker から `bwrap_argv`・`verify_isolation`・`check_egress`・`orphan_groups` は未呼出。実 namespace・egress・killpg は検証できていない。 |
| P3-A identity 復元（隔離下のみ） | 未達。API の契約テストだけ | `cargo test -p task-api restore_is` の前回結果は2 passed。`restore_isolated` は稼働中 runtime に未結合。`--restore` / `--state` / `--profile` は利用しない。 |
| P4-B stronger injection | 未達。攻撃の純関数テストだけ | `cargo test -p celeris-credentiald injection` の前回結果は6 passed。実 CDP sink / IPC peer UID role は未接続。旧 plugin bridge を通る機密要求は起動前 routing で拒否。 |
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

## 未解決

- P4-A: runtime方式とUID運用の人の選択。選択後、workerへの実起動・事実採取・broker/CDP/IPC分離・filtering proxy・orphan回収・隔離sessionへのidentity復元を配線する。
- P4-B: P4-A の後、実 CDP trusted sink と SO_PEERCRED による role 判定を配線し、実経路の攻撃負例を検査する。CredentialProvider契約とH3は維持する。
- P4-C: H7 評価候補の人の選択。採用候補を実 fixture runner と同一task評価へ接続し、必要能力を失わないfallbackを実行経路で検査する。機密能力はP4-A/B相当の証拠取得後に有効化する。
- 上記2件の決定要求は run の `result.json` に記録。内部originの追加は行わず、既存のloopback/private IP拒否を維持する。前回の子タスク提案は採用・完了を確認できていないため、実装済みとは扱わない。
