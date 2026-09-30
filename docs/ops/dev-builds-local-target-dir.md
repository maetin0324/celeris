# 開発ビルドの target-dir をローカルに固定する（2026-09-30）

## 事象
2026-09-30 03:00Z、委譲エージェント 2 本の worktree（`~/workspace/agent-platform/.claude/worktrees/agent-*`、NFS の home 上）で cargo が
`target/` を NFS に書き、I/O pressure "full" 71%・load 80（24 コア）になった。CPU pressure は 0。NFS の統計は mount 以来 WRITE 1,340 万回 / 7.98 TB。
Celeris 本番の task（build はローカル `/var/lib/celeris/scratch/targets`）も巻き添えで遅くなった。

## 規則
- `~/.cargo/config.toml` の家訓「ビルドごとに CARGO_TARGET_DIR を明示（ローカル LVM `/var/tmp/agent-platform-build/<name>`）」を、
  worktree 直下の `.cargo/config.toml`（`[build] target-dir`）で機械的に守る。`CARGO_TARGET_DIR` 環境変数があればそれが優先。
- 生成は `scripts/dev/worktree-target-dir.sh`（冪等）。名前は worktree の basename（`agent-<id>`）、本体 checkout は `<repo>-main`。
- Claude Code の `PreToolUse`(Bash) hook（`.claude/settings.json`）が、cargo / nextest / test-parallel を含むコマンドの直前に同じ生成を行う
  （常に exit 0。ブロックしない）。委譲エージェント（worktree）にも main の session にも効く。
- `/.cargo/config.toml` は `.gitignore`。本番リリースの build tree（`/var/lib/celeris/release-build`）はローカルなので触らない。

## 手で使う
```
bash scripts/dev/worktree-target-dir.sh            # cwd の worktree
bash scripts/dev/worktree-target-dir.sh <dir>      # 別の worktree
```
## 掃除
`/var/tmp/agent-platform-build/agent-*` は worktree を消したら不要。`du -sh /var/tmp/agent-platform-build/*` で確認して消す。

## 置き場の変更（2026-09-30 04:3xZ）
`/var/lib/celeris/build-cache/cargo` は委譲エージェントの権限分類で「共有資源の変更」と判定され `ls` すら拒否されたため、同じローカル LVM 上の
`/var/tmp/agent-platform-build/<name>` に変更（`CELERIS_BUILD_CACHE` で上書き可）。`~/.cargo/config.toml` の家訓のコメントもこちらを指すよう更新すること（人）。
