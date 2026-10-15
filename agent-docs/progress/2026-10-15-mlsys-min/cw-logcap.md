---
unit: cw-logcap
task: credential-wait
status: done
completed: 2026-10-15
---

# cw-logcap: 秘密の値が通常ログに出ないことのログ捕捉試験

## 内容
- `crates/task-dispatch/src/dispatcher/tests/credential_logcap.rs`（新規）。`tracing::Subscriber` を自前実装（新 dependency なし）し、
  全 level の event と span の欄を捕捉する。`with_default` の下で 待機入口（`credential_gate_hold`）→ secret 保存 →
  `CredentialResumer::process`（mock verifier）→ 再開 を回す。
- 4 結果（Valid / InvalidKey / InsufficientPermission / Transient=429）それぞれ別のダミー値で実行し、
  捕捉した全ログに 値・先頭 8 文字・末尾 8 文字が含まれないことを断言。
- 空捕捉でないこと: 待機入口の info log（`credential missing; work unit waits…`）が捕捉されていることと行数 ≥ 2 を断言。
- 値が出る経路は見つからなかったので本体コードは変更なし（log に出るのは task_id / work_unit / secret_id / error のみ）。

## 証拠
- `cargo test -p task-dispatch credential_logcap` → 1 passed。
- 全体の `test-parallel.sh` と clippy は下記の実行結果を参照。

## 未解決事項
なし。
- test-parallel.sh: exit 0 (passed 5193, failed 0)。clippy workspace と task-dispatch --tests: exit 0。
