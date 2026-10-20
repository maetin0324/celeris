# replan で同じ段の依存先が変わった持ち越し WU を新しい起点から作り直す

- 日付: 2026-10-20
- 状態: 採用（実装中）
- 関連: ADR-0074（同じ段の依存は依存先 WU ブランチの HEAD から切る）、ADR-0079 D9 / D17、ADR 2026-10-11（破壊的変更を是とする）
- 実例: task 01M5AC7DNX の WU ctl-worker-callsite（計画 v7 で依存先を celeris-worker-callsite に変えたが、
  base_commit 5bc09a30 と旧ブランチが残り、再実行で同じ E0063）

## 背景

replan の持ち越し（`task-ops/src/execution.rs`）は `depends_on` を書き換えるが、kind task 以外の行の
`base_commit` / `head_commit` は残す。prepare（`task-dispatch/src/dispatcher/work_units.rs`）は既存の
WU ブランチがあれば `base_commit`（無ければブランチの HEAD）を起点に使い回し、`local_worktree` は `.git`
があれば作り直さない。範囲検査の snapshot（`wu_base.rs`）も WU id ごとに初回だけ作る。そのため依存先を
変えても新しい依存先の成果が worktree に入らない。

## 決定

- D1（判定）: replan の持ち越しで、kind task 以外の unit の「同じ段の依存先の集合」（`depends_on` のうち
  新しい版で同じ phase にある key の集合）が旧い行と新しい版で異なれば「起点の作り直しが要る」とする。
  依存の有無の変化（あり→なし、なし→あり）も含む。違う段の依存だけの変化は対象外（段の統合で入る）。
- D2（記録）: 作り直しが要る行は replan で `base_commit` / `head_commit` を消し、`rebase_pending` を立てる
  （`work_units` に列を足す migration）。旧い挙動を残す切り替え flag は作らない。
- D3（prepare）: `rebase_pending` の WU は、既存の WU ブランチを `<branch>-superseded-v<plan_version>` に
  `git branch -m` で改名して退避する（reflog ごと残る。消さない）。WU の worktree は `git worktree remove
  --force` で外してから、新しい起点で作り直す。新しい起点は同じ段の依存先の HEAD（`dependency_base`）、
  依存が無ければ Task ブランチの HEAD。`CELERIS_WU_BASE` と `base_commit` はこの起点になる。
- D4（範囲検査）: 範囲検査の snapshot はその WU について作り直す（古い起点の snapshot を消してから初回と
  同じ手順で作る）。古い起点との差分で範囲を測らない。
- D5（追跡）: 作り直しのたびに event `WorkUnitBaseReset { work_unit_id, key, old_head, archived_branch,
  new_base }` を追記する。古い試行の commit は退避ブランチの reflog と event から追える。
- D6（変えないもの）: 同じ段の依存先が変わらない持ち越しは、従来どおりブランチと `base_commit` を使い回す。
  kind task の unit の既存の扱い（子の結び付けを外し、branch / base を消す）は変えない。

## 結果

- 依存先を変える replan が実際に効く。旧い試行は消えず、退避ブランチと event に残る。
- 退避ブランチは replan のたびに増える。片付けは task の close と同じ時機に別途扱う（本 ADR の範囲外）。
