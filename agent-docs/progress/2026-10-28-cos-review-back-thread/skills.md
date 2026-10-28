---
title: CoS skill に定期見直しの入力（review thread）と報告 operation の扱いを書く
tasks: [01M61HW6RGHHRVAGRMQASDF9YY]
status: done
updated: 2026-10-28
---
# CoS skill に定期見直しの入力（review thread）と報告 operation の扱いを書く

完了日: 2026-10-28

ADR [2026-10-28-cos-review-back-thread](../../adr/2026-10-28-cos-review-back-thread.md) の D3'〜D5' の skill 部分。

## 変更

- `config/skills/cos-operator/SKILL.md`: 節「定期見直しの入力（review thread）」を足した。人に伝えるべきこと（新しい停滞・
  人の判断が要る件・案件の要約の重要な変化）があるときだけ `chat.review_report`（`POST /api/v1/cos/review-report`、
  body `{review_message_id, text}`）で統括 CoS チャットに 1 件報告し、無ければ review thread に 1 行書いて終える。報告は
  人が数十秒で読める分量にする。統括 thread へ `chat.message_post` で直接書かない。入口の表の `cos-inbox-triage` の
  mount 先の説明を `launch.rs` の現行に合わせた。
- `config/skills/cos-inbox-triage/SKILL.md`: §11（定期見直しの run）を足した。review thread の run は受信箱の件を読むが
  resolve しない（ADR D3'）。冒頭の mount 先の説明を現行に合わせた。policy_version は 6 のまま（判定基準は変えていない）。
- `config/skills/cos-operator/operations.md`: `chat.review_report` の行は api-op の葉が既に足していた
  （review thread の run だけ、入力 1 件に 1 件、text 1〜4000 字）。この葉では変えていない。
- 各 `SOURCE.md` に 2026-10-28 の更新記録を足した。
- 統括 thread に定期見直しの返事を直接書かせる記述は skill に無かった（旧 ADR D2 の指示は dispatch の
  `COS_REVIEW_INSTRUCTION` にあり、dispatch の葉が置き換え済み）。

## 証拠（提出時の HEAD で実行、基準は `$CELERIS_WU_BASE`）

- `grep -n chat.review_report config/skills/cos-operator/operations.md` → 1 行（surface 表）。
- `grep -n "review thread" config/skills/cos-operator/SKILL.md config/skills/cos-inbox-triage/SKILL.md` → 両方に節がある。
- `bash scripts/dev/test-affected.sh --list` → 対象 crate なし（config/ と agent-docs/ だけの変更）。exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決事項

- 本番 KB への skill の取り込み（`celerisctl skills import config/skills --name cos-operator --name cos-inbox-triage`）は人が行う。

## 提案

なし。
