---
title: "launcher runtime の credential-request → WaitingForAuth wait"
tasks: [01M4G7XB7HSDYBCD36G2TZ0S22]
status: done
updated: 2026-10-09
---

# launcher runtime の credential-request → WaitingForAuth wait

## 変更

final review 指摘 (1)（launcher runtime が `credential-request.json` を wait に変えない）を直した。

- `crates/task-worker/src/browser.rs`
  - `credential_wait(task_id, run_id, session_id, policy, &CredentialRequest) -> NewBrowserWait`（`pub(super)`）を追加。approval の `operation_wait` と同じ置き方で、`WaitingForAuth` wait をここでだけ組む。中身は intent（`origin` / `purpose` / `credential_policy_id`）と policy binding・`resume_key = auth:<task>:<run>` だけで、`credential: None`・`operation: None`・`trusted_login: None`。
  - `shim_request_wait(runtime, task_id, run_id, session_id, policy, sink, outcome) -> (outcome, Option<BrowserRunState>)`（`pub(super)`）を追加。run 後に shim が runtime dir に残した request file を durable wait に変える共有段で、**daemon 経路と launcher 経路の両方がこれを呼ぶ**。
    - 順（両経路で同じ。コードの comment にも書いた）: `credential-request.json` があれば**それを先に**処理して `WaitingForAuth` wait を開き、run は `Terminal::Question`（文面は `CREDENTIAL_REQUEST_QUESTION`、origin も秘密も含まない）。`credential-request.json` が無いときだけ `approval-request.json` を見て `WaitingForApproval` wait（ADR 2026-10-08 D2）。
    - fail closed: request が policy に合わなければ（許可外 origin・知らない `policy_id`・非 https origin・`CredentialUse` 未許可・未知の欄）wait を開かず `Err`。`sink.browser_wait_open` が失敗しても `Err`（`browser wait could not be opened`）で、resume できない Question を返さない。
    - 返り値の `Option<BrowserRunState>` が wait の state（`WaitingForAuth` / `WaitingForApproval`）。
  - `CredentialRequest` を `pub(super)` に（`credential_wait` の引数型。`private_interfaces` 警告の解消）。
  - daemon 経路（`run_with_executable_attempt`、旧 1876–1943 行）は inline の wait 組み立てをやめて `shim_request_wait` を呼ぶ形にした。挙動は不変（同じ順・同じ resume key・同じ state 決定。cleanup 失敗が outcome を上書きする位置も変えていない）。
- `crates/task-worker/src/browser_launcher_run.rs`
  - `run` の run 後段（旧 925–956 行の `approval-request.json` だけを見る段）を `super::shim_request_wait(&runtime_dir, …)` の呼び出しに置き換えた。これで launcher 経路でも `credential-request.json` が先に `WaitingForAuth` wait になる。
  - `waiting_for_approval` フラグをやめ、`wait_state: Option<BrowserRunState>` で最終 `browser.state` を決める（wait が開いていればその state、無ければ従来どおり Done→Completed / Question→WaitingForHuman / それ以外→Failed）。`sink.browser_updated` は従来どおり最後に 1 回。
- `crates/task-worker/src/browser_launcher_run_tests.rs`
  - `launcher_credential_request_` 接頭辞の試験 5 件を追加（偽 sink `RecordingWaitSink` と tempdir の request file だけ。userns・実 launcher・実 process・socket・ネットワーク不要）。
  - `CredentialUse` を含む prepared policy を作る `shim_policy` / `credential_policy` helper を追加。

## 試験

`browser_launcher_run_tests.rs` の `launcher_credential_request_*`（5 件）:

| 試験 | 見ていること |
| --- | --- |
| `…_opens_a_waiting_for_auth_wait_and_questions` | (a) `credential-request.json` があると wait がちょうど 1 件開き、`reason = WaitingForAuth`、`resume_key = auth:<task>:<run>`、outcome は `Terminal::Question`、state は `WaitingForAuth`。`credential` / `operation` / `trusted_login` は `None`、policy binding は wait に固定される |
| `…_outside_policy_is_refused_without_a_wait` | (b) 許可外 origin・知らない `policy_id`・非 https origin・`CredentialUse` 未許可の 4 通りで wait 0 件・state なし・`browser credential request denied` の `Err` |
| `…_takes_priority_over_approval_request` | 両方の request file があると credential が優先（wait 1 件・`WaitingForAuth`）。`credential-request.json` を消すと同じ dir で `approval-request.json` が `WaitingForApproval` になる（決めた順の固定） |
| `…_without_a_wait_store_is_an_error` | wait store が開けないと `browser wait could not be opened` の `Err`（fail closed、Question を返さない） |
| `…_keeps_the_secret_out_of_wait_events_and_state` | (c) request に未知の欄で試験用の秘密文字列を混ぜると拒否され、拒否の文言にも wait にも出ない。正しい request では runtime dir に秘密 file があっても、wait・browser 更新・progress の serialize と outcome に秘密が出ない（`wait.credential == None`） |

## コマンドと結果

- `cargo test -p task-worker --lib launcher_credential_request_` → exit 0、`test result: ok. 5 passed; 0 failed; 0 ignored; 993 filtered out`
- `cargo test -p task-worker --lib -- approval credential_request operation_wait` → exit 0、10 passed（daemon 経路の `click_approval_opens_a_wait_and_resumes_the_same_session_once` と `credential_request_origin_outside_effective_domain_is_denied` を含む。共有段への置き換え後も daemon 経路の挙動が変わっていないことの確認）
- `cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0（警告なし）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo test -p task-worker --lib`（既定 TMPDIR）→ 956 passed / 38 failed。失敗は全て `bind: Error { kind: InvalidInput, message: "path must be shorter than SUN_LEN" }`（launcher / browser_launcher / CDP / socket 系の既知の環境失敗。この WU の変更とは無関係）
- `TMPDIR=/local/celeris/data/scratch/cw cargo test -p task-worker --lib -- browser::launcher_run browser_launcher:: browser::tests::` → exit 0、`118 passed; 0 failed; 3 ignored`（短い TMPDIR では上記の失敗が消え、追加した 5 件も含めて全件成功）
- `cargo fmt -p task-worker` を実行した後、この WU の範囲外の file（`browser_ledger_tests.rs`・`browser_tests.rs` の既存の未整形行）は `git checkout --` で戻した。変更 file は `sh "$CELERIS_WU_SCOPE_PATHS"` で 3 file（`browser.rs`・`browser_launcher_run.rs`・`browser_launcher_run_tests.rs`）+ この進捗 + ADR 付記だけであることを確認した

## 未解決事項

- 共有段の試験は `shim_request_wait` を直接呼ぶ（launcher 経路の run 後段そのもの）。`run` 全体を端到端で回す試験は credentiald への登録（実 socket）と偽 launcher の process 起動が要り、既定 TMPDIR では `SUN_LEN` で落ちる既存試験と同じ環境依存を持つため追加していない。
- `integrate-impl`（統合）で main 追従後の同一検査を再度流す。

## 提案

- `shim_request_wait` は request file の検査・wait 組み立て・sink 呼び出しだけを持つ純粋な段になったので、request の種類が増える（例: 別の人間待ち）場合はこの 1 関数に case を足す形を維持したい（daemon / launcher の両経路に同じ順が自動的に効く）。
