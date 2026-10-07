---
task: build-tmp-hygiene
wu: main-sync
status: done
completed: 2026-10-07
---
# main-sync

- merge commit: 18ff07ce（`git merge --no-ff main`、衝突なし）
- 取り込んだ main: 0fa576a8（account_pool_scenarios 修正）。HEAD の祖先であることを `git merge-base --is-ancestor` で確認
- `git diff main -- crates tests web`: 差分ゼロ
- `cargo build -p celeris -p celerisctl` の後に `cargo test -p e2e --test account_pool_scenarios`: 3 passed / 0 failed
  （binary 未 build だと 3 本とも "celeris not found" で落ちる。先に build が要る）
