---
title: 葉 adr-script — ADR 2026-10-14-eval-trial-scratch-target と worktree-target-dir.sh の env 優先・ルート fs 既定廃止
tasks: [01M4WHYHVZTYNBRGM5C3PWHMZE]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# 葉 adr-script

## やったこと
- ADR `agent-docs/adr/2026-10-14-eval-trial-scratch-target.md` を新規（D1 評価試行の試行ごと scratch target、D2 script、D3 残り経路の撤去）。
- `scripts/dev/worktree-target-dir.sh`:
  - `CARGO_TARGET_DIR` が env にあれば書かない（前にこの script が書いた config は消す）。
  - 置き場は `CELERIS_BUILD_CACHE`、無ければ `<CELERIS_SCRATCH_DIR（既定 /local/celeris/data/scratch）>/dev-targets`。
    `/var/tmp/agent-platform-build` の既定を撤去。
  - 置き場が無い・scratch が無い・ルート fs 上（`stat -c %d` が `/` と同じ）なら書かずに止める。hook は常に exit 0。
  - 名前は `<basename>-<絶対 path の sha256 先頭 12 桁>`（`-main` 印は廃止）。
- 試験 `scripts/tests/worktree-target-dir.sh`（新規、POSIX sh、一時 git repo、12 項目）。
- `agent-docs/ops/dev-builds-local-target-dir.md` の規則・掃除・経緯を新しい置き場に書き直し。

## 証拠
- `sh scripts/tests/worktree-target-dir.sh` → exit 0、12 項目 ok（"all passed"）。
- 旧 script（HEAD 版）に差し替えて同じ試験 → FAILED: 11（試験が旧挙動を検出することの確認。試験中に旧 script が作った空 dir
  `/var/tmp/agent-platform-build/repo-main` は rmdir 済み）。
- `grep -n /var/tmp scripts/dev/worktree-target-dir.sh` → 該当なし。

## 未解決事項
- `/var/tmp/agent-platform-build` を既定に持つ Rust 側の経路（ADR 2026-10-07-build-tmp-hygiene D1.1 の `target_sweep` 既定 roots など）は
  葉 var-tmp-audit の担当で、この葉では触っていない。
- 評価試行の `CARGO_TARGET_DIR`（D1）は葉 eval-target。

## 本番への移行
- 人の手元の worktree は次の cargo 実行で hook が `/local/celeris/data/scratch/dev-targets/<name>-<hash>` を指す config に書き換える（初回は全 build）。
- `/var/tmp/agent-platform-build/` の残りは人が `du -sh` で確かめて消す。`~/.cargo/config.toml` の家訓のコメントが旧い置き場を指していれば直す。

## 提案
- `dev-targets` は scratch pool の lease を持たないので `scratch_gc` は消さない。worktree が消えた target を消す保守を
  target_sweep の roots に入れるかは var-tmp-audit で判断する。
