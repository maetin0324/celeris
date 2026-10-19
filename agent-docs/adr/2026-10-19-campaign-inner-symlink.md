# campaign の snapshot / isolation は「tree 内に閉じた相対 symlink」だけ許す

- 日付: 2026-10-19
- 状態: 実装済み
- tasks: [01M58VFRBA6HG0SSF3CG5BKM91]

## 背景
K2 の初期 tree（BenchFS 086c4618 + submodule 5 件）には symlink が 3 件ある（`lib/locusta/CLAUDE.md -> AGENTS.md`、`…/ucx1-sys/ucx/README -> README.md`、`ior_integration/ior/testing/docker/centos6/run-test.sh -> ../centos7/run-test.sh`）。
campaign lock（`lock_tree`）は symlink を mode 120000・link 先文字列として数えるが、`snapshot::{hash_tree,copy_tree}` と `isolation::copy_filtered` は symlink を一律拒否したため、preflight の measure-env が exit 1 になった。人の決定（k2-symlink: a）。

## 決定
- D1: 許すのは **tree 内に閉じた相対 symlink だけ**。`snapshot::check_inner_symlink` は link 先を component ごとに辿り、出会った symlink（連鎖・入れ子を含む）をその場で展開する。`..` は展開後の実体に適用し、全ての中間段階で root の内側に留まることを確かめる。絶対 path、途中でも root 外へ出るもの（例: `alias -> .` と `link -> alias/../tree/a/real.txt`）、dangling、file を dir として通るもの、循環（展開 40 回超）を拒否する。字面の深さや最終解決先だけの検査は途中の root 外通過を見逃すため使わない。
- D2: `hash_tree` は symlink を `L` + 相対 path + link 先文字列として数える（中身は辿らない＝二重に数えない）。symlink を含まない tree の hash は変わらない。lock の content_sha256（asset の hash）は変更しない。
- D3: `copy_tree` / `copy_filtered`（試行の隔離展開）は symlink を symlink のまま、link 先文字列を変えずに置く。全段階が root 内に留まると検査済みで、相対文字列は変わらないので、複製後の全 link は複製 root 内に閉じる（試験で canonicalize して確認）。
- 緩めたのは「tree 内に閉じた相対 symlink」だけ。special file、root 外を指す・絶対・dangling・循環は拒否のまま。grader の成果物 dir の symlink 規則（`grader.rs`）は変えない。

## 試験
`crates/campaign/tests/inner_symlink.rs`（合格: 相対・`..` を含む tree 内・dir 向き・tree 内連鎖、hash が link 先文字列に依存、symlink のまま複製。拒否: 絶対・root 外・root 外経由の連鎖・dangling・循環・reviewer の `alias -> .` + `alias/../tree/a/real.txt`・入れ子 alias 経由の途中脱出・file 通過。合格: dir link 経由の `..`・入れ子の連鎖を含む安全なリンクで、複製後の全 link の canonicalize 先が複製 root 内）、`tests/isolation.rs`（prepare_trial が symlink を保ち、root 外を拒否）。実 K2 の 3 件は組立済み tree で hash_tree / copy_tree が通り、複製後の hash が一致することを確認した。
