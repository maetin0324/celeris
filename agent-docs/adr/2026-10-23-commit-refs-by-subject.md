# ADR 2026-10-23: 進捗ファイルと完了報告では commit を hash でなく件名で書く

## 背景
2026-10-23 に 3 task（研究評価再監査 3・健全性 5・研究評価再監査 1）が、進捗ファイルの commit 表・「検証した SHA」・merge-base が
rebase 前の hash のままという同じ理由で 2〜3 回ずつ差し戻された。worker が hash を書いた後に、celeris が review 前の main 同期で
task branch を rebase する（ADR-0118 D3、ADR-0120）。rebase は task branch の全 commit の hash を変えるので、worker が自分の
commit・HEAD・merge-base を hash で書く限り、記録は構造的に古くなる。worker が直しても次の同期でまた古くなる。

## 決定
- D1: 進捗ファイル（ADR-0128 D3）と完了報告では、task branch の commit を **件名（subject）で列挙**し、hash を書かない。
  reviewer は件名の commit が提出された履歴（`git log $(git merge-base HEAD main)..HEAD`）にあるかで判定する。
- D2: 差分・検査の基準は「`git merge-base HEAD main`」（WorkUnit は `$CELERIS_WU_BASE`）と**式で**書き、固定の hash を書かない。
- D3: 検査結果は「提出時の HEAD で実行」と書き、最後の main 同期・rebase・merge の後に再実行した結果だけを載せる。
  「検証した SHA」「tested_sha」の欄は置かない（どの SHA で検査が通ったかは celeris の `ReviewTargetSynced`・
  `RequiredChecksEvaluated` が記録する。ADR-0118、ADR 2026-10-23-required-checks-bound-to-candidate）。
- D4: 機械検査。`sh scripts/dev/progress-index.sh --check` は、この branch が変えた進捗ファイル
  （`git merge-base HEAD main` との差分と未追跡）の front matter・本文に出る commit hash らしい hex
  （英数字以外で区切った 7〜40 桁の `[0-9a-f]`、数字と a-f を両方含む）が **main 側の commit（merge-base の祖先）でなければ
  失敗**させる（警告ではない）。HEAD の祖先かどうかでなく main の祖先かで判定するのは、rebase 後の task branch の commit も
  次の同期でまた変わるため、提出時点で通る hash を将来も通る hash（main に入った commit）に限るため。hash の記載自体の全面禁止は
  しない: main に入った commit（他 task の修正・取り込んだ main の commit）は rebase で変わらず、参照として有用である。
  commit として解決できない hex も失敗（rebase 前の hash は object が消えると解決できない）。digest は全桁（64 桁）で書けば対象外。
  `main` が無い checkout（release bundle 等）では検査しない。この branch が変えていない進捗ファイル（過去の記録）は見ない。
  `scripts/dev/check-health.sh`（test-affected が先頭で走らせる）から `--check` と試験
  `scripts/dev/tests/progress_index_commit_hashes.sh` を呼ぶ。
- D5: 完了報告（result.json の summary・本文）で hash を挙げるなら、提出直前に `git merge-base --is-ancestor <hash> HEAD` が 0 のものだけ。
  task branch の commit は件名で書く。
- D6: 規則の正本は `CLAUDE.md`「各Phase完了時」。書式と例は `agent-docs/guides/progress-files.md`、reviewer への指示は
  `crates/task-worker/src/claude_code/prompt.rs` の `build_review_prompt`（hash が無いことを不備にしない、件名で履歴を照合する）。

## 影響
- ADR-0128 D3 の進捗ファイルの書式に D1〜D3 が加わる（同 ADR に付記）。
- 既存の進捗ファイルの hash は書き換えない（履歴）。ただしある task が既存ファイルを変えれば、そのファイルは D4 の対象になる。
- 進行中の branch で、自分の進捗ファイルに自分の commit の hash を書いているものは、main を取り込んだ後の test-affected で落ちる。
  件名に直す。
