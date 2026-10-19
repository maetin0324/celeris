# Jev 副比較 B1/C1 の別測定 manifest（schema 分離）

tasks: [01M59W8DFMVT3M8WRM4XP8ZEWE]

## 決定
- B/C のみ（A なし）の manifest は別 schema `celeris.campaign-manifest-side-b1c1/1`（`SIDE_CAMPAIGN_SCHEMA`）で宣言する。`CampaignManifest` に欄は足さない（A0/B0 の hash・既存 literal・検査は不変）。別測定 ID は `campaign_id`。
- side の `validate` は configs=B,C、primary=(B,C)、`check_side_variant` を要求する: B/C の共通部分は mode（policy_version・根拠表の扱い）以外同一、根拠表なし、候補集合 `routing.models` に Sonnet と Opus の両方（未宣言・単一は拒否）。
- `side_variant_hash` が共通条件＋方策差を固定する。`planned_configs` / checker の期待集合は A を含まない。
- 明示 lane/tier/model は routing 欄に存在せず（deny_unknown_fields）、task template でも禁止（既存）。

## 影響
旧い仕組みの置換ではなく追加。累計予算の合算は別 WU（cum-budget）。
