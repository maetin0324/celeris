---
title: "ADR: 最小 campaign 評価基盤（manifest・hash・split・dry-run）"
status: accepted
date: 2026-10-15
tasks: [01M4Z2989XX5XK159CCDNV1HYH]
---
# 最小 campaign 評価基盤

## 文脈
MLSys 向けに A（既存 baseline）/ B（Celeris+fixed）/ C（Celeris+Jev）を比べる。別の巨大オーケストレータは作らず、routing-eval の manifest/hash/split を再利用する。

## 決定
- D1: 新 crate `crates/campaign`（`celeris-campaign`）。campaign manifest は routing-eval の入力 manifest を path+sha256 で固定参照し、problem id・split（dev↔`dev`、final↔`held-out`）を照合する。routing-eval 本体は変えず、同じ方式の hash を campaign 側 `manifest_hash` に持つ（scope 制約）。
- D2: hash は正規化（problems・subset・configs 整列、キー順固定の canonical JSON）後の sha256。`sha256` 欄は空なら未封印、入っていれば内容と一致を要求。
- D3: dev と final は 1 campaign 内で混ぜない。manifest 検査（problem の split 不一致を拒否）と集計（`campaign_sha256`・split 不一致の trial を拒否）の両方で強制する。
- D4: 主比較は A/B 固定、B/C は manifest の `secondary.subset`（事前登録）だけ。budget・termination の欠落／0 は拒否。
- D5: campaign 側の集計は error/timeout/cancelled/missing を分母（宣言した問題数）から除かず、各内訳を出す。routing-eval の根拠表は従来どおり error/cancelled を分母から外して除外数を出す（意図した別物）。
- D6: routing-eval `aggregate::build` は問題の所属を見ない。呼び出し側が `campaign::aggregate::dev_trials` で manifest の dev 問題だけに絞ってから渡す。routing-eval 本体は変更しない。回帰試験は `crates/campaign/tests/routing_eval_identity.rs`。
- D7: dry-run は実 LLM を呼ばない。token/wall/disk/並列の見積りは全て `estimated` ラベルで、仮定値を出力の `assumptions` に載せる。実測値が無い欠測の仮置きで、実性能・実費用の主張に使わない。

## 帰結
runner・checker・export・fixture は後続 unit。
