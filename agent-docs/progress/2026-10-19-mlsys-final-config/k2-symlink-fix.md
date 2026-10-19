---
tasks: [01M58VFRBA6HG0SSF3CG5BKM91]
unit: k2-symlink-fix
status: done
completed: 2026-10-19
---
# K2 初期 tree の symlink 3 件で preflight が exit 1 になる問題（方針 a）

人の決定 k2-symlink: a（snapshot/isolation が tree 内に閉じた相対 symlink を許す）。判断は `agent-docs/adr/2026-10-19-campaign-inner-symlink.md`。

## 変更
- `crates/campaign/src/snapshot.rs`: `check_inner_symlink`（絶対・root 外・root 外経由の連鎖・dangling・循環を拒否）、`hash_tree` は symlink を link 先文字列ごと数える、`copy_tree` は symlink のまま複製。
- `crates/campaign/src/isolation.rs`: `copy_filtered` が同じ規則で symlink を複製（`prepare_trial`）。
- 試験: `tests/inner_symlink.rs`（6 件）、`tests/isolation.rs` に 1 件追加。
- `docs/ops/measure-run-final.md` §4 の「既知の阻害」を解消済みの記述へ更新。asset の hash（lock の content_sha256）は不変。

## 証拠
- `cargo test -p celeris-campaign --test inner_symlink --test isolation` → 6 passed + 6 passed（exit 0）。
- 実 K2 tree を `pinned/build-srcs.sh` で組立（symlink 3 件: `lib/locusta/CLAUDE.md -> AGENTS.md`、`…/ucx/README -> README.md`、`…/centos6/run-test.sh -> ../centos7/run-test.sh`）→ 一時試験で `hash_tree` / `copy_tree` が成功、複製後の hash 一致（`bc94862f…7d254cec`）。試験は削除済み。
- 全体検査（test-parallel / clippy / fmt）の結果は最終報告に記載。

## 未解決事項（operator 入力待ち）
- 実際の `measure-launcher.sh preflight` の通しは、operator 入力（`REPO_SRC` 等の実配置・pin 済み値・有料前の host 条件）が要るため未実行。symlink 起因の exit 1 は解消したが、他の不足一覧は pin-values / preflight の進捗ファイルを参照。
- この修正を含む release を測定専用 daemon / launcher が使う必要がある（campaign バイナリは release 経由。本番操作は人が実行）。

## 提案
- なし。
