---
title: "健全性v2 R4 record-d7: D7 の反例と試験の対応を全行で締め、進捗の未解決事項を結果に置き換える"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R4 record-d7

完了日: 2026-11-15

コードは変えていない。計画 v10 の段 dep-fix（cred-cache・fix-promote）と段 pin-rest（cex-race・cex-tick・cex-late2）の結果を、
ADR [2026-11-11-dispatcher-state-ownership](../../adr/2026-11-11-dispatcher-state-ownership.md) と R4 の進捗
[2026-11-11-health-v2-r4.md](../2026-11-11-health-v2-r4.md) に書き写した。基準は `$CELERIS_WU_BASE`（task 全体は `git merge-base HEAD main`）。

## 変更

- ADR 2026-11-11-dispatcher-state-ownership:
  - 付記 2026-11-14 の表の試験の列に 9 本を足し、右の列（試験の無い部分）を全行「無し」にした。足した試験:
    `cex_lease_renew_after_expired_snapshot_keeps_the_holder`（lease）、`cex_restart_after_wu_done_promotes_dependents_once`・
    `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`（restart）、`cex_done_and_lease_expiry_in_one_tick_close_the_run_once`・
    `cex_human_cancel_between_wu_and_task_transitions_is_kept`・`cex_human_comment_between_wu_and_task_transitions_is_kept`（同時完了）、
    `cex_late_integration_after_stage_repair_is_dropped`・`cex_late_review_pass_after_replan_is_dropped`・`cex_late_review_pass_after_repair_is_dropped`
    （review/integration）。objective の 8 本に、cex-tick の comment の 1 本を足した（D7 の同時完了の行の「人のコメントか cancel」の comment の側）。
  - 付記の試験 file の一覧に `cex_same_tick.rs` を足し、初版の「試験の無い部分」の説明を record-d7 が締めたことに書き換えた。
  - 成り立たなかった反例の箇条を 6 件にした。3 件目が fix-promote（ADR 2026-11-14-dependency-promotion-every-plan-version へのリンク、修正の前後の結果）。
    4〜6 件目は段 pin-rest で見つけて直した反例（fix-store・fix-reclaim、fix-verdict、late-review。ADR へのリンクと修正の前後の結果）。
  - D1 の表の `credential_held` の行に、cred-cache が消したこと（store の `credential_waits` の写しだった、ADR 2026-11-14-atomic-task-credential-verified の
    付記 cred-cache）を書いた。owner の要約の表の `ExecutionCoordination` の「移し済み」を 30 から 29 にした（field 数は表の行の数なので 30 のまま）。
  - 冒頭の決定と unit の対応表の D1・D7 の行に cred-cache と、段 dep-fix・pin-rest の unit を足した。
- R4 の進捗 2026-11-11-health-v2-r4.md: 結論、決定と unit の対応、「D7 の反例と試験」（表の右の列を全行「無し」、足した試験の結果）、
  write-set の節（cred-cache・fix-promote・pin-rest が触れた path、slices-check の数、決定 store-scope の後の task-core の差分）、
  未解決事項 1・2 の結果、提案、受け入れ条件ごとの証拠（record-d7）。
- cex-lease.md: 未解決の反例の項に fix-promote への参照（修正と試験、前後の結果）を足した。
- この file。

## 実装前 brief

1. 根本原因: 最終 review の criterion 1 は、ADR の D7 付記の右の列に試験の無い順序が残り、cex-lease の反例が repair・replan に回っていないことで落ちた。
   試験と修正は段 dep-fix・pin-rest が済ませたが、ADR と進捗が古いままだった。
2. 責務 owner: ADR 2026-11-11 の付記 2026-11-14 と D1 の表、R4 の進捗（この unit の objective）。
3. 既存の再利用先: 各 unit の進捗（fix-promote.md・cred-cache.md・cex-race.md・cex-tick.md・cex-late2.md）の観測と証拠。試験名は試験 file の `fn` 宣言で確かめた。
4. 選択肢: (a) 付記の表をその場で書き換える（採用。objective の指示）。(b) 新しい付記を足す（不採用。同じ表が 2 つになる）。
5. 新しい状態・分岐・fallback・port: 無し（文書だけ）。
6. 撤去対象: 付記の表の右の列の「未確認」「試験は無い」「には無い」、進捗の未解決事項 1・2 の旧い記述。
7. 不変条件: 付記の表の 5 行の右の列が「無し」、8 本の試験名が ADR と進捗にあり、試験 file に `fn` がある（下の証拠）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

R4 の進捗の「受け入れ条件ごとの証拠（record-d7 の unit）」に同じものを書いた。

## 未解決

- 無し（D7 の表の外の窓は R4 の進捗の未解決事項に、各 unit の未解決として挙げた）。

## 提案

- R4 の進捗の「提案」のとおり。
