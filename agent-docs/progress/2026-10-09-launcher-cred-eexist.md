---
title: "launcher 経路の shim file 二重作成（EEXIST）の修正"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-09
---

# launcher 経路の shim file 二重作成（EEXIST）

- 完了日: 2026-10-09
- 症状: 本番 release 66e72820dd20 で task 01M4GYJ3XGJNWZQDF35F1MDE0H の run 01M4GYKC7KGHK0J73GS4YMB161 が
  launcher session 起動（session 69c435089aee6a02784a0c8987edabfe）の約 2 秒後に
  `infra_requeue: adapter: browser backend lacks required conformance or all capable backends failed: io error: File exists (os error 17)`。
- 原因: `crates/task-worker/src/browser_launcher_run.rs::run` が `write_shim_files`（`write_private` = `create_new(true)`）を
  2 回呼んでいた。1 回目は session を開く前の fail-closed 版（98f40d96 で追加）、2 回目は admission 後。2 回目の最初の
  `celeris-browser.py` 作成が EEXIST で失敗する。credential の有無に関係なく launcher 経路の全 run が harness に届かない。
  run 単位の path（`<workspace>/runs/<run_id>/browser`）なので過去 run の残骸ではない（本番で消すべき file は無い）。
- 修正: shim file の新規作成は run ごとに 1 回（1 回目の戻り値を使う）。admission が通った run だけ `admit_shim_config` で
  `config.json` を `replace_private`（`config.next` に書いて rename）で置き換える。置換に失敗したら session を stop して `Err`。
  fail-closed の順（gate と credentiald 登録の後にだけ credential_use を立てる）は変えていない。

## 証拠

- 再現: 修正前のコードで `launcher_run_creates_shim_files_once_and_reaches_the_harness` が
  `Io(Os { code: 17, kind: AlreadyExists, message: "File exists" })` で失敗。修正後は成功。
- `cargo nextest run -p task-worker launcher`: 102 passed
- `bash scripts/dev/test-parallel.sh`: 4988 passed, 13 skipped, exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require cargo nextest run -p task-api --test browser_h3_injection`: 3 passed

## 未解決事項

- admission が真になる経路（proof の launcher UID = 設定値 ≠ daemon UID）は同一 UID の試験 process では作れないので、
  `run` の通し試験は非 credential run で行い、置換段は単体試験で見ている。本番での credential run の確認は deploy 後に要る。

## 提案

- launcher 経路の `run` を偽 launcher で最後まで通す試験を release gate に含める（今回の不具合はどの試験も `run` を通していなかったため漏れた）。
