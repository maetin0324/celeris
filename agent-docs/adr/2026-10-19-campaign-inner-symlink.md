# campaign の snapshot / isolation は「tree 内に閉じた相対 symlink」だけ許す

- 日付: 2026-10-19
- 状態: 実装済み
- tasks: [01M58VFRBA6HG0SSF3CG5BKM91]

## 背景
K2 の初期 tree（BenchFS 086c4618 + submodule 5 件）には symlink が 3 件ある（`lib/locusta/CLAUDE.md -> AGENTS.md`、`…/ucx1-sys/ucx/README -> README.md`、`ior_integration/ior/testing/docker/centos6/run-test.sh -> ../centos7/run-test.sh`）。
campaign lock（`lock_tree`）は symlink を mode 120000・link 先文字列として数えるが、`snapshot::{hash_tree,copy_tree}` と `isolation::copy_filtered` は symlink を一律拒否したため、preflight の measure-env が exit 1 になった。人の決定（k2-symlink: a）。

## 決定
- D1: 許すのは **tree 内に閉じた相対 symlink だけ**。`snapshot::check_inner_symlink` が (1) 絶対 path を拒否、(2) link 先の字面が置かれた dir から tree root の外へ出る（`..` 超過）ものを拒否、(3) 連鎖を最後まで辿った実体（`canonicalize`）が root 外のもの・dangling・循環を拒否する。これ以外の symlink は従来どおり拒否する。
- D2: `hash_tree` は symlink を `L` + 相対 path + link 先文字列として数える（中身は辿らない＝二重に数えない）。symlink を含まない tree の hash は変わらない。lock の content_sha256（asset の hash）は変更しない。
- D3: `copy_tree` / `copy_filtered`（試行の隔離展開）は symlink を symlink のまま、link 先文字列を変えずに置く。検査は source root 基準なので、展開先でも root の外を指せない。
- 緩めたのは「tree 内に閉じた相対 symlink」だけ。special file、root 外を指す・絶対・dangling・循環は拒否のまま。grader の成果物 dir の symlink 規則（`grader.rs`）は変えない。

## 試験
`crates/campaign/tests/inner_symlink.rs`（合格: 相対・`..` を含む tree 内・dir 向き・tree 内連鎖、hash が link 先文字列に依存、symlink のまま複製。拒否: 絶対・root 外・root 外経由の連鎖・dangling・循環）、`tests/isolation.rs`（prepare_trial が symlink を保ち、root 外を拒否）。実 K2 の 3 件は組立済み tree で hash_tree / copy_tree が通り、複製後の hash が一致することを確認した。
