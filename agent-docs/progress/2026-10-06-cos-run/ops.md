---
tasks: [01M47997XM2GYH4E5J2QNAJZ7R]
completed: 2026-10-06
---
# CoS run credential と監査付き操作層

## 完了内容

- **credential / checkpoint (`crates/task-core/src/chat/credential.rs`, `credentials.rs`)**: run に結び付いた期限付き credential を発行・hash 検証し、run 終端で失効。checkpoint は現在の run と配送済み cursor を検証し、32 KiB 上限と expected 値の競合を扱う。dispatcher からの発行・失効配線は後続の chat-run 段が担当する。
- **operations store (`crates/task-core/src/chat/operations.rs`)**: thread 内 idempotency key と request hash を照合。同一 hash は同じ operation、異なる hash は conflict。適用・`cos_operations`・監査 envelope event・chat card event を transaction にまとめ、拒否も reason 付きで記録する。
- **認証と checkpoint API (`crates/task-api/src/cos/auth.rs`, `checkpoint.rs`)**: bearer credential から `actor=cos`・thread・run を確定し、body/header の人間 identity claim を採用しない。credential 付きの既存領域への直接変更は `cos_audit_context_required` で 422。
- **operations API (`crates/task-api/src/cos/operations.rs`)**: `POST /api/v1/cos/operations` と thread 範囲で読む `GET /api/v1/cos/operations/{o}`。最終 allowlist は task create、comment create、decision answer、approval decide、execution phase-gate、project title/request update、knowledge reject。任意 URL、未登録 path、再帰 `/cos` 等は拒否する。登録操作は既存 handler と共有する operation 関数へ監査 context を渡す。
- **CLI (`crates/celerisctl`)**: `CELERIS_COS_RUN_CREDENTIAL` が設定された変更要求を `/cos/operations` 経由で送信し、未対応操作は送信前に拒否する。
- **schema/docs**: `docs/api/v1/api-v1.schema.json`、web/gui の生成型、`docs/api/v1/gui-api.md` と同期文書に route・型を反映。

## 受け入れ条件と証拠

0. `bash scripts/dev/test-parallel.sh` → exit 0。最終統合 HEAD `0e9b4c85` で nextest **4,069 passed / 0 failed / 12 skipped**、doctest を含む script 集計は **4,069 passed / 0 failed / 13 ignored**（124 nextest binaries + 10 doctest binaries）。
1. 文書検査は全て exit 0: `sh scripts/dev/check-doc-links.sh` (`check-doc-links: ok`)、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` (`check-doc-layout: ok`)、`sh scripts/dev/check-adr-numbers.sh` (`ok (141 files)`)。
2. `cargo clippy --workspace -- -D warnings` → exit 0。

関連する `cos_chat_ops_` 試験は計 **39 件**（core 7、auth/checkpoint API 7、operations API 6、領域 API 7、celerisctl 9、CLI command 3）。主な試験名: `cos_chat_ops_cred_*`, `cos_chat_ops_checkpoint_*`, `cos_chat_ops_auth_*`, `cos_chat_ops_checkpoint_api_*`, `cos_chat_ops_api_*`, `cos_chat_ops_domain_*`, `cos_chat_ops_ctl_*`。証拠として各実装片に記録した `cargo test -p task-core --lib`、`cargo test -p task-api --test cos_auth --test cos_operations --test cos_operations_domains`、`cargo test -p celerisctl --test cos_ops` がある。最終 HEAD の全 workspace 実行も上記 gate で通過した。

## 未解決事項

- dispatcher による chat run 開始時の credential 発行・run 終了時失効の配線は chat-run 段の作業。
- 操作は登録 allowlist のみ。外部 URL/proxy、`/cos` 再帰、その他の task/decision/approval/execution/project/knowledge 操作は公開していない。
- `expected_revision` は operation に必須で記録するが、領域固有の revision 照合は各領域の版定義が必要。
- knowledge 候補は git 上にあり SQLite transaction の外部。git 更新後に SQLite commit が失敗した場合の完全な原子性は未解決。
- 外部操作の outbox は未実装。
- 生成型の GUI package install / generator は依存 store の SQLite error と offline tarball 不足で実行不能だったため、GUI 型は手反映。schema test、web generator `--check`、GUI docs sync `--check` は schema leaf の記録上 pass。

## 提案

- chat-run 段で dispatcher の run lifecycle に credential 発行と失効を接続し、credential 値が log・event・prompt に現れないことを確認する。
- 外部操作が必要になった時点で outbox の再試行・冪等性と監査 envelope を別途設計する。
