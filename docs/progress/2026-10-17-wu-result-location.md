# WU の result.json 誤配置修正

---
tasks: [01M54Y3MRRA1PAHWC128PQCTP1]
---

## 原因と修正

`wu/e2e/repos/agent-platform` から `../artifacts/result.json` に書くと、到達先は `wu/e2e/repos/artifacts/result.json` となる。正規の `wu/e2e/artifacts/result.json` とは異なる。

既存の worktree 用 RunRequest は絶対パスを出していた。一方、共通前置きの「残したい物は `artifacts/` に置く」「成果物ディレクトリ（`artifacts/`）」は cwd 相対の案内と読める。今回の修正ではその案内を取り除き、冒頭に result.json の絶対パスと「親ディレクトリを推測した相対パスも使わない」を追加した。共有 cwd の WU も絶対パスに統一した。過去 run のモデル内部の判断は断定せず、コード上で確認できた曖昧さを除去する。

旧実装は cwd 直下の `artifacts/result.json` のみ移動・採用していた。この処理と誤配置ファイルの事前削除を撤去した。正規ファイルがない場合は当該 WU root と cwd を調べ、期待パス・発見パスを含む再試行不可の失敗にする。JSON 本文を採用せず、誤配置ファイルを残す。`runs`、`.git`、`target`、`node_modules` と symlink は探索しない。

結果ファイルが任意の対話は探索対象外とし、home cwd の不要な探索を防ぐ。Claude/Codex の結果復元前、および dispatcher の共通 worker 実行経路で診断する。WU の失敗処理はこの診断を infra_requeue や自動 replan に変換せず、WorkerFinished にパス付き理由を残す。

設計: [ADR](../adr/2026-10-17-wu-result-location.md)。

## 検証

外部 LLM は使わず、shell stub と mock adapter を用いる。worker 試験は cwd 直下と親相対の誤配置、正規ファイル優先、探索範囲・symlink 除外を確認する。dispatcher 試験は mock が親相対で結果を書いて AdapterError::Other を返す障害を再現し、起動 1 回・パス付き WorkerFinished・WU failed・infra_requeue 無しを確認する。

初回の全体検査は 5,659 pass / 1 fail（doc-test 込み）。失敗した既存の `run_tmpdir_preamble_forbids_tmp_copies` は、撤去した相対 `artifacts/` 表記を必須としていた。期待値を「冒頭で指定された成果物ディレクトリ」に更新した。2 回目は既存の `the_repository_check_commands_are_the_reviewers_default` が後続レビューの verdict を得られず失敗したが、単独再試験は 0.87 秒で成功した。この試験のコードは変更していない。


ログ保存先: `/local/celeris/data/workspaces/01M54Y3MRRA1PAHWC128PQCTP1/artifacts/`。

- `test-misplaced.log`: 初回 worker 診断試験
- `test-wu-misplaced.log`: WU mock adapter 回帰試験
- `test-parallel.log`: 初回全体検査
- `test-parallel-final.log`: 2 回目全体検査
- `test-repository-check.log`: 既存 reviewer 試験の単独再検査
- `test-parallel-verified.log`, `clippy-verified.log`, `fmt-verified.log`: 最終差分の必須検査


最終検査（2026-10-17）:

| コマンド | 結果 |
| --- | --- |
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0、5,661 pass / 0 fail / 15 ignored（doc-test 込み） |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

`TMPDIR=/tmp` は Unix socket のパス長制約のため。Cargo の target・profile・wrapper 設定は変更していない。userns 強制の release gate は今回の検査対象ではない（summary の userns は null）。
