---
title: "B1/C1 variant の hash 固定と子/孫/retry 継承の検査"
tasks: [01M59KY5FPEWVNW0DT07HZRA3Q]
unit: variant-pin
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# variant-pin

`crates/campaign/tests/variant_pin_b1c1.rs`（7 試験）。B1/C1 は manifest の B/C。新ルータなし。
既存の `check_resolved`・`routing_override`・task 祖先継承（ADR 2026-10-16-mlsys-routing-inheritance-reconciliation）を再利用。本体コードは変更していない。

- (a) `variant_hash_is_pinned`（B/C の共通部分・方策差・予算・初期情報の hash を固定）、`changing_anything_but_the_policy_breaks_hash_and_check`（候補集合・下段・harness・harness 版・model・予算・input・snapshot を変えると hash が変わり、arm 欄の変更は検査も落ちる。policy_version だけの変更は検査を通る）、`b1_and_c1_differ_only_in_the_policy`（routing_override は mode だけが違う）
- (b) `evidence_table_is_never_used`（両 arm の evidence_mode=none、根拠表の追加・暗黙 latest は失敗）
- (c) `c1_pinned_to_a_single_model_or_lane_fails`（候補集合が Sonnet+Opus の 2 以上でなければ失敗。tier/assignee/routing_override の template 経路は拒否）
- (d) `variant_is_inherited_by_child_grandchild_and_retry`（root の pin が子・孫・retry に及び、config の mode が逆でも勝つ。pin の無い task は config に落ちる負例）、`pin_on_a_sibling_branch_does_not_cover_a_retry_elsewhere`

## 不備・未解決（本体は直していない）
- 「候補集合が 2 モデル以上」の検査は試験内の helper で、本番の preflight/`check_resolved` には入っていない（final.json の B/C は `models` 未宣言で Sonnet 単独）。提案: Jev 副比較用 manifest を作る時に `check_resolved` 側へ入れる。
- B1/C1 を A なしの別 manifest として宣言する形は未整備（inventory 不足 4）。本試験は A/B/C manifest の B/C 部分を variant として hash する。
- 継承は in-memory store の単体試験で、実 daemon の子 run の実ログ確認は live-runbook の人手順に属する。
