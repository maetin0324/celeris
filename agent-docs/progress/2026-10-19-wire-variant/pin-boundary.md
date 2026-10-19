---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 全役割・非対象 pin の検証証拠境界（pin-boundary）
work_unit: pin-boundary
status: done
updated: 2026-10-19
---
# pin-boundary

文書だけの葉。コードは変えていない。成果は `propagation.md` の §0（訂正）と §8（固定証拠の境界・決定後設計）、および §2・§5 D5/D7・§7・未解決事項・提案の更新。

## 内容
- 訂正: migration は 0087 → **0088**。新関数 `task_campaign_resolve_checked` は作らず、既存の `task_campaign_resolve` を強化した。core の変更は subtree ごとに別葉（core-wire / core-store / core-callsite-fix / core-api-schema / core-web-types）で、複数の subtree を同じ葉で直してはいない。
- 境界: daemon（L1）は `CampaignIdentity` と自分が観測できる値だけを照合し、観測値を記録する。campaign（L2）は封印証拠（selection・manifest・lock・VariantSpec・承認済み sha256 付きの B0 入力・env-lock・測定 config）から期待値を作って照合する。`effective_profile_sha256` は coding 課 1 node の証拠にすぎず、builtin 役割・他 node・役割モデル・情報・予算・認可・grader の不変性の証拠には使わない。
- 対応表（§8.3）: node 集合、coding 課、他の全 node（根 `cos` を含む）、全役割モデル（`runs.selected_*`）、起動時 config、情報、認可、review、予算、grader、識別の 11 行。それぞれについて POST・run 開始・campaign のどこで照合し、欠落したらどう明示失敗するかを書いた。
- 未解決: `variant-b0.json` は現状どこにも封印されていない（提案 P1）。reload は `identity` に出ない（提案 P2）。

## 再照合（変更前）
- `origin/main` = `254c885a`（base より 48 commit 先）。variant・task-core/api/ops には触れていない。merge-tree の衝突は progress 4 件の front matter だけ。
- 3 task: `01M58VFRBA6HG0SSF3CG5BKM91` は done（main に統合済み）。他の 2 件は cancelled。DB は `sqlite3 "file:$CELERIS_RUN_DB?mode=ro"` で読取専用で読んだ。

## 証拠
- `test -s agent-docs/progress/2026-10-19-wire-variant/propagation.md` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → ok / ok (254 files) / ok、exit 0
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` → 本 dir の 2 file だけ）→ 範囲外 0 件
- 正の grep: `0088` 6 件、`## 8. 固定証拠の境界` 1 件、§8.3 の表 11 行（§6 の表と合わせて `^| n |` 17 行）、`coding 課 1 node` 1 件。負の grep: §0 の訂正表を除く `resolve_checked` 0 件、`0087_task_campaign` 0 件
- cargo は回していない（文書だけの葉）
