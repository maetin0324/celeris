# ADR 2026-10-23: 必須検査は置き換えず、結果を基底 SHA・候補 SHA に束縛して main 取り込み前に照合する

## 背景
main への取り込み（delivery の ff-only）は、ADR-0118 の `target_sha` / `reviewed_sha` / `merge_candidate_sha` の照合で「review が見た
候補」だけを通す。だが検査の結果そのものは SHA に結び付いていなかった。
- review の `workspace.toml` `[commands] check`（test-affected・clippy・fmt）は、**task の `acceptance` に `Check::Command` が 1 つでも
  あれば実行されなかった**（ADR-0043 D4「acceptance の明示が勝つ」）。task 固有の acceptance で必須検査を消せた。
- 検査結果は `ReviewVerdict` に合否が残るだけで、どの基底・候補で走ったかは delivery から引けなかった。reviewer の延期で review を
  やり直すと、同じ SHA でも重い検査を走らせ直した。
release gate（全体試験）は main 更新・push の後なので、統合前の最小検査と候補の対応は release gate では担保できない。

## 決定
- D1: リポジトリの `[commands] check` は必須。task の `acceptance` に `Check::Command` があっても置き換えず、足す
  （`review::review_task`）。acceptance に `expect_exit = 0` で同じ cmd があれば、その判定を必須検査の結果とし、二重には走らせない。
  `mode = prototype` は従来どおり必須検査を持たない（ADR-0046 D4）。ADR-0043 D4 のこの部分を置き換える（旧い分岐は消した）。
- D2: review は必須検査の結果を `Event::RequiredChecksEvaluated {repo_id, target_sha, reviewed_sha, definition_sha256,
  results[]}` に残す。`target_sha` / `reviewed_sha` は同じ review の `ReviewTargetSynced`（ADR-0118 D3）の値で、
  新しい SHA の仕組みは作らない。検査中に snapshot が動けば従来どおり stale で、証跡は書かない。
- D3: 同じ `(repo, target_sha, reviewed_sha, definition_sha256)` に束縛された最新の証跡で合格した cmd は、次の review で再実行せず再利用する
  （`required_checks::reusable_passes`。不合格は再利用しない）。候補・基底・定義 digest のどれかが違えば再利用されない（失効）。
- D4: delivery は `MergeQueued` から ff-only を試す前に、`check_candidate`（ADR-0118 D4）に続けて
  `required_checks::verify_required_checks` を呼ぶ。証跡が無い・束縛が現在の `target_sha`/`reviewed_sha` と違う・1 件でも不合格・
  候補の HEAD の `workspace.toml` が必須とする cmd が結果に無い（acceptance 等で置換された）は、ff せず再 review に戻す
  （`request_rereview`。stale と同じ上限 `MAX_TARGET_RESYNCS`、超えたら人に見える Blocked）。ff-only と `validate_candidate` は変えない。
  各 repo で必須 cmd の網羅・定義指紋を照合する。
- D5: 統合経路ごとの検査と証跡の照合箇所は進捗ファイルの棚卸しに書く。全体試験・e2e・browser/userns は足さない
  （ADR 2026-10-20-full-tests-at-release-only）。

## 影響
- acceptance に command を書く task の review でも test-affected・clippy・fmt が走る（同じ cmd の acceptance は重複しない）。
  変更の無い crate は test-affected が空で終わる。
- 本番への移行: この版の昇格時に MergeQueued で止まっている配送は証跡が無いので 1 回だけ再 review になる（config・DB の変更は無い。
  event の追加のみ。schema は `docs/api/v1`・`web/api/generated` を再生成済み）。
- 軽量ゲート（別 task）を `workspace.toml` の `check` に足せば、そのまま必須検査になり証跡に入る（接続は不要）。

## 検証
`task-core::required_checks::tests`（束縛・失効・再利用・最新優先）、`review::tests`（acceptance が必須検査を置換しない・
同一 cmd の重複なし・再利用）、`delivery::tests::merge_requires_passing_evidence_*`・
`merge_rejects_evidence_missing_a_required_check_of_the_candidate`、`target_sync` の証跡の記録、
`scripts/dev/tests/test-affected-list.sh` (d)（統合時に基準が空 diff にならない）。

## 付記: evidence-gaps（2026-10-23）

- 候補の定義は共通の `task_core::required_checks::candidate_required_checks`（git の実行は呼び出し側が渡す）で読む。git show が失敗した場合、cat-file と
  ls-tree で候補のファイル不在を確認したときだけ空の定義とする。git 実行・取得・parse の失敗は review と delivery を拒否する。
- 各 repo の候補の定義を各 worktree で実行し、その repo 自身の ReviewTargetSynced と結果・定義指紋を束縛する。
  他 repo への結果の複製・網羅検査の免除は廃止した。
- verify と reusable_passes は定義 digest の一致も必須とする。同じ候補・基底でも定義指紋が異なれば失効する。
- 必須検査を先に実行し、acceptance の同じ cmd（expect_exit=0）は同じ workspace の結果を使う。
  同じ束縛・定義での再 review は acceptance も合格を再利用する。別 repo の同一 cmd は共有しない。
- event の不要な定義元 repo 欄を削除し、API schema と web 生成型を更新する。

定義 digest は v2 domain と長さ付き cmd 列に変更する。これにより旧実装が別 repo へ複製した成功も失効し、
同一コマンド列を持つ複数 repo でも旧証跡を再利用しない。改行入りの単一 cmd と複数 cmd の衝突も避ける。
旧イベントは削除せず、次の review が各 repo の証跡を追記する。
