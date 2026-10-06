---
title: R5 launcher attach policy と session dir 回収
tasks: [01M4896B1Q1617Q95NT2BNDNAF]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# R5 launcher attach policy と session dir 回収

WorkUnit `launcher-launch`。

## 修正

- launcher が session に書く agent-browser の `policy.json` へ、CDP attach 専用の `launch` を常に追加した。`SessionPolicy.allowed_actions` は変更せず、利用者に許す action は従来の verb からだけ生成する。
- `SessionDir` の所有権で、起動失敗と通常停止の両方から session dir を回収する。launcher UID で削除できない subuid 所有の入れ子は、runtime 停止後に新しい userns の mapped subuid で空にしてから launcher UID で削除する。失敗時は launcher の stderr に path と原因を記録する。
- supervisor の失敗を `observe` で確認した場合、lease を待たずに session を停止して記録と dir を回収する。

## 検証

- `launcher_policy_allows_attach_and_only_authorized_navigation`: 書き出した policy は `launch` と `navigate` を含み、許可 origin の `open` は通り、別 port は拒否する。action が空なら `launch` だけになる。
- `session_dir_guard_removes_session_after_stop_or_failed_setup`: 回収 guard の解放後に session dir がない。
- `failed_supervisor_observation_reaps_session_immediately`: 失敗観測後に process、session 記録、server の session が消える。
- `cargo test -p task-worker --lib browser_launcher` → 33 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。

subuid を用いる実 launcher/Chrome 試験は `CELERIS_USERNS_TESTS=1` の opt-in のまま。worker sandbox では実機確認を行わず、4 回目の確認は Fable に委ねる。launcher binary の本番 host への配置・再起動は人の手順で行う。
