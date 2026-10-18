# 親の continuation で同じ範囲の子を再起票しない

- 日付: 2026-10-18
- 状態: 採用・実装済み

## 背景
Pairs PRA-31（親 01M56TM9339FNM5QTE2R8VA0BN）で、continuation の各 run が子を新たに起票し、31a×3・31b×3 が並走した。
done の子の成果が main にあるのに同じ範囲の子が再び作られ、親の branch にも同じ endpoint が実装された。
continuation の進捗は files_changed・head・completed/remaining で判定するため、重複起票は検出されなかった。

## 決定
- D1: `task_ops::delegate::plan_delegation`（delegate.json と execution-plan の `children` の両経路）は、親の既存の子
  （failed・cancelled 以外: done・running・ready・draft・blocked）と title の正規化一致、または objective の token Jaccard >= 0.9 の提案を
  拒否する（理由に既存の子の id・状態を書く）。execution-plan 経路は拒否 1 件で計画の採用が失敗する。
- D2: 子を起票できる run の prompt には、既存の子の一覧（status・outcome・branch）を「Existing child tasks (do not duplicate)」節で示す。
  集約 run の節は従来どおり。
- D3: continuation の進捗判定（`checkpoint_shows_progress`）は子の起票を入力に含めない。重複起票は D1 で作られないので、進捗にも数えない。
  旧い「子を起票すれば何度でも作れる」前提の試験は無い（新規に重複拒否の試験を置く）。
