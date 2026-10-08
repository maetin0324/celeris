# ADR 2026-10-08: WorkUnit の範囲検査を開始時の作業木 snapshot と比較する

---
tasks: [01M4E0E7SX4Q8BJ50N0K08174R]
---

- 状態: 実装済み
- 関連: [ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md)、[WU checks の指針](../guides/work-unit-checks.md)

## 背景

専用 worktree がない直列実行では、先行 unit の成果は commit されず同じ作業木に残る。
`WorkUnitRow.base_commit` が空のため worker・事後 check に `CELERIS_WU_BASE` が設定されず、
`HEAD` 比較と未追跡一覧が先行成果を後続 unit の範囲違反として検出した。
人は 2026-10-08 に unit 開始時の base を daemon が渡す方針を選んだ。

## 決定と実装

1. `dispatcher/wu_base.rs` が、初回 worker 起動前に先頭リポジトリの作業木を記録する。
   実 index を別 index にコピーし、`git add -u`・`write-tree`・`commit-tree` で追跡済み snapshot を作る。
   これを `CELERIS_WU_BASE` として渡す。`git diff --name-only "${CELERIS_WU_BASE:-HEAD}"` の既存形は
   先行の追跡済み未 commit 成果を除外できる。実 index・HEAD・作業 file は変更しない。
2. 同じ別 index の `git add -A` で未追跡 file も含む全 snapshot を作る。
   未追跡一覧 file を `CELERIS_WU_BASE_UNTRACKED`、全 snapshot と比較する shell 補助を
   `CELERIS_WU_SCOPE_PATHS` で渡す。全 snapshot を直接 `git diff` に渡すと Git は未追跡 file を削除として
   扱うため、補助が現在の tree を別 index で組み、`git diff --cached --name-only` で比較する。
   前からある未追跡 file の編集・削除、symlink・mode、後から commit された file も検出する。
3. 保存先は実行 cwd の git directory の `celeris-wu-bases/<wu-id>/`。`snapshot.json` の rename を保存完了の印とする。
   WU id 単位で初回にだけ作り、retry・daemon 再起動・事後検査引き継ぎでは読み直す。
   `refs/celeris/wu-base/<wu-id>` が全 snapshot とその親の追跡済み snapshot を GC から保護する。
   作業木とともに記録を保持し、自動で再取得・削除しない。
4. `dispatch_run.rs` は作業場所を用意してから、worker 起動より前に snapshot を保存する。
   保存失敗は WU prepare の失敗として扱い、基点無しで worker を起動しない。
   `work_units.rs::spawn_work_unit_checks` は保存した同じ変数を check 環境に追加する。
   統合用 `base_commit` は変更しない。新 snapshot がない旧 run の check は従来の `base_commit` に戻る。
5. planner の雛形は補助があればその出力を使用し、補助の失敗を非 0 として伝播する。
   補助無しの場合のみ従来式に戻る。旧計画の `git ls-files --others` を daemon が書き換えることはしない。
   先行未追跡 file がある計画は雛形を更新する必要がある。単なる一覧除外では編集・削除を見逃す。

## 適用範囲

ローカル git の共有・直列作業木、WU 専用 worktree、Task worktree を使う repair に適用する。
環境変数の対象は従来どおり worker の cwd（先頭リポジトリ）。複数 repo を横断する補助、remote git、
git でない作業場所には拡張しない。snapshot commit は範囲比較専用であり、HEAD の祖先であることは保証しない。
本番操作、元のアカウント上限 task の計画・成果の変更は含まない。

## 検証

`task-dispatch` の `wu_base` 試験で直列実行の先行追跡済み・未追跡成果の除外、範囲外編集の拒否、
worker と事後 check の変数一致、retry の一致、専用 worktree の分離、実 index・HEAD の保存、
未追跡の変更・削除・symlink・commit 後比較、git GC 後の再利用を固定する。
結果は [進捗](../progress/2026-10-08-work-unit-scope-snapshot.md) に記録する。
