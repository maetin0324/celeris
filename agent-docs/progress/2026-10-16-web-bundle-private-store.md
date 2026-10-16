---
task: web-bundle-private-store
status: done
completed: 2026-10-16
---
# web 配布物の共有 store 丸写しをやめる

## 原因
ADR 2026-10-16-web-bundle-private-store の「原因（証拠）」を参照。共有 store `/local/.pnpm-store/v11`（344M・16450 file、
store の mtime が失敗時刻 05:42 と一致）を release.sh が `cp -a` していた。失敗時の log は回収されておらず、
競合の瞬間は事後に再現できない（負荷再現は禁止）。構造上の根拠で特定し、丸写しを無くした。

## 検証（一時 dir、worktree の web/ で release.sh を実行し、後で消した）
- `CELERIS_WEB_RELEASE=tmptest bash web/scripts/release.sh` → exit 0。`pnpm fetch --prod` → `install --prod --offline --frozen-lockfile` 成功。
- tarball 11,035,744 byte、entry 7571（旧 約 1.7 万）。共有 store の写しは無く、`.pnpm-store` は配布物専用の prod 分のみ。
- bundle_web 相当: `mktemp -d` に `tar -xzf --strip-components=1` → `pnpm install --prod --offline --frozen-lockfile` 成功 →
  `import.meta.resolve("express"); await import("./server/app.js")` が `import ok`。

## 未解決・提案
- 失敗した release の gate-logs を FAILED_DIR から回収する仕組みが無い（次に別の失敗が出たとき原因確認が難しい）。
