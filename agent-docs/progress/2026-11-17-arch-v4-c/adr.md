---
title: アーキ v4 C adr — 境界の意味を変える改修を高 risk にする規則・class の補完・final review の差分 risk・6 ケースの設計
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: running
updated: 2026-11-17
---
# adr（WorkUnit）

ADR: [2026-11-17-boundary-change-risk-review.md](../../adr/2026-11-17-boundary-change-risk-review.md)。基準は `$CELERIS_WU_BASE`。

## 変更
- `arch-v4 C adr: …` — ADR を足した（実装前 brief 7 項目、決定 D1〜D6、replay の数、後続 WorkUnit への引き継ぎ）。
  code・profile・scripts（`crates/`・`web/`・`scripts/`）は変えていない。

## 決めたことの要点
- D1: write path が高 risk の class に合えば `new_*` の宣言によらず High（理由 `path_class:<class>:<path>`）。`new_fallback` は今どおり High。
  brief の無い計画も class に合えば High（補足の理由 `brief_absent_human_plan`）。
- D2: 新しい class `authz`・`transaction`・`event-apply`・`browser`・`worker-finish`・`review-gate` と `selfdeploy` への `crates/celeris/src/instance.rs` の追加。
  class ごとに `invariants`（`[review.invariants]` の key）を持たせる。`store` は狭めない（replay の数）。
- D3: final review で実差分（rename の両側・削除）を base 側の方針に当て、採用した計画の設計 review が覆わない class が残れば、同じ reviewer run に既存の
  `design`（`DesignVerdictOut`）を書かせる。無い・読めない・不合格は不合格（fail-closed）。
- D4: reviewer に path・owner 地図の行・不変条件と `cycle-edge:<src>-><dst>` の鍵、worker・planner に規則と class ごとの不変条件。
- D5: 6 ケース `boundary_case_1_`〜`6_` と失敗注入。
- D6: profile の変更は leaf `profile` だけ。現行の daemon は `high_risk[]` の項目の新しい欄を読み飛ばす（`PathClass` に `deny_unknown_fields` が無い）。
  top level の欄を足すと `ReviewPolicyOut` の `deny_unknown_fields` で方針が読めなくなるので足さない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- replay: `python3 -I $TMPDIR/replay/replay.py . bc665168c9caf1898adc4c611404bbee056daf9a 2026-10-28T00:00Z bc665168c9caf1898adc4c611404bbee056daf9a`
  → 非 merge 492 件中 base 80・D2 94、web/・docs/ だけ 10 件中 0 件（道具と出力の写しは run の artifacts の `replay/`。道具は commit しない）。
- 範囲: `git diff --name-only "$CELERIS_WU_BASE" -- crates web scripts` → 出力なし。

## 未解決
- 進捗の task 側の「未解決」を参照（D1 の呼び出し元の確認、旧い規則の High の件数は未確認）。
