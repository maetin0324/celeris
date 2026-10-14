# 開発ビルドの target-dir をローカルに固定する（2026-09-30）

## 事象
2026-09-30 03:00Z、委譲エージェント 2 本の worktree（`~/workspace/agent-platform/.claude/worktrees/agent-*`、NFS の home 上）で cargo が
`target/` を NFS に書き、I/O pressure "full" 71%・load 80（24 コア）になった。CPU pressure は 0。NFS の統計は mount 以来 WRITE 1,340 万回 / 7.98 TB。
Celeris 本番の task（build はローカルの scratch target）も巻き添えで遅くなった。

## 規則（2026-10-14 改定。ADR 2026-10-14-eval-trial-scratch-target D2）
- worktree 直下の `.cargo/config.toml`（`[build] target-dir`）で target を `/local` の scratch（ADR-0136）に固定する。
  `CARGO_TARGET_DIR` 環境変数があれば script は config を書かない（Celeris の run・評価試行は scratch の target を env で受ける）。
- 生成は `scripts/dev/worktree-target-dir.sh`（冪等）。置き場は `CELERIS_BUILD_CACHE`、無ければ
  `<CELERIS_SCRATCH_DIR（既定 /local/celeris/data/scratch）>/dev-targets`。名前は `<worktree の basename>-<絶対 path の sha256 先頭 12 桁>`
  （同じ basename の worktree で target を共有しない）。
- 置き場が存在しない・scratch が無い・ルート fs 上なら書かずに止め、この script が前に書いた config は消す。scratch の無い host では
  `CELERIS_BUILD_CACHE` にローカルの非ルート fs の dir を指す。
- Claude Code の `PreToolUse`(Bash) hook（`.claude/settings.json`）が、cargo / nextest / test-parallel を含むコマンドの直前に同じ生成を行う
  （常に exit 0。ブロックしない）。
- `/.cargo/config.toml` は `.gitignore`。本番リリースの build tree（`/var/lib/celeris/release-build`）は触らない。
- 試験: `sh scripts/tests/worktree-target-dir.sh`。

## 手で使う
```
bash scripts/dev/worktree-target-dir.sh            # cwd の worktree
bash scripts/dev/worktree-target-dir.sh <dir>      # 別の worktree
```
## 掃除
`/local/celeris/data/scratch/dev-targets/<name>-<hash>` は worktree を消したら不要（scratch pool の lease を持たないので `scratch_gc` は消さない）。
`du -sh /local/celeris/data/scratch/dev-targets/*` で確認して消す。旧い置き場 `/var/tmp/agent-platform-build/` はもう使わないので丸ごと消してよい。

## 置き場の経緯
- 2026-09-30: `/var/lib/celeris/build-cache/cargo` は委譲エージェントの権限分類で拒否されたため、ルート fs の `/var/tmp/agent-platform-build` にした。
- 2026-10-14: 評価試行の worktree（basename が全部 `repo`）が 1 つの target を共有し、ルート fs を埋めた。置き場を `/local` の scratch に移し、
  env 優先・ルート fs 拒否・名前に hash を足した（ADR 2026-10-14-eval-trial-scratch-target）。
