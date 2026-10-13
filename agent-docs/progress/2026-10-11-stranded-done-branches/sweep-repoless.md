---
tasks: [01M4M8D4ANYGCWVZ0A0WSW53X7]
status: completed
completed: 2026-10-11
---
# sweep-repoless: repos 未設定の done root task を取り残しの走査に入れる

## 何をしたか

- `crates/celeris/src/daemon/stranded_sweep.rs`: done root task を `task.repos` の有無で分けた。`task.repos` が空の task は `repoless_records` が登録済みの Local repository（`project_list` → `repo_list`）ごとに `git for-each-ref --format='%(refname) %(objectname)' refs/heads/celeris/` を 1 回読み、`celeris/<task id>` があればその `repo_id` で `BranchRecord` を作る。どの repository にも branch が無ければ記録しない（repo を変えていない扱い）。`task.repos` を持つ task の挙動は変えていない。git は store lock の外で読む。
- `crates/task-api/src/integrations.rs`（`resolve_repo`）と `crates/task-api/src/handlers/tasks.rs`（`attach_task_integrations`）: project の無い task でも、task の integration 行が指す repository で名前解決する。これで merge/discard/hold/reopen の route と task 詳細・一覧の `integrations` が repos 空の task でも働く。
- ADR `agent-docs/adr/2026-10-11-stranded-done-branches.md`: D1 の「repo 未設定は未解決」を D3 の解決規則への参照に直し、D3 に repos 未設定 task の repo 解決規則を書いた。付記に「repos 未設定 task の解決（D3、sweep-repoless）」節を足し、配線の説明も直した。

## 証拠

- `cargo test -p celeris --test integration_api` → exit 0、5 passed（新規 3 件）:
  - `integration_api_repoless_done_task_detected_in_inbox_and_detail_then_discarded`: project_id なし・repos 空の done root task と、main に無い commit を持つ登録 repo の `celeris/<id>` → sweep で unintegrated（変化 1 件、再 sweep は 0）、受信箱に unintegrated item、task 詳細の `integrations[0]` が unintegrated・repo 名つき。discard で discarded、受信箱から消え、再 sweep も 0。
  - `integration_api_repoless_task_without_branch_records_nothing`: branch の無い repos 空 task は行も event も無い。
  - `integration_api_repoless_branch_in_main_is_integrated`: head が main の祖先なら integrated、受信箱に出ない。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0、passed 5163 / failed 0 / ignored 14。run の既定 TMPDIR（93 文字）のままだと Unix socket の path 上限で credentiald・browser 系の試験が落ちる（今回の変更と無関係、既知）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決

- 本番の初回 sweep では repos 空の done root 548 件について、登録 repo ごとに for-each-ref を 1 回読む（repo 数分の git 呼び出し）。branch が残っている task は全部 unintegrated か integrated として記録される。古い branch が多ければ受信箱に一度に多く出る。
- 進捗の根因節と「本番への移行」手順の更新は rootcause-verify が受け持つ。
