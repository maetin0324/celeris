---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
title: routing-eval 統計関数
---
# routing-eval 統計関数

`crates/routing-eval/src/stats.rs` に、問題単位の `pass^1` を扱う純関数を実装した。Wilson 95% 区間、seed 固定 bootstrap、対応のある exact McNemar と paired 差 bootstrap 区間、pass 最大化・費用/時間/quota 最小化の Pareto 前線を提供する。

D1 の根拠成立条件（30 問×2 は有効問題 24 以上、k=1 は全問題）、区間が重なる同等集合、k=1 の優越主張禁止、層別の開発/held-out で選択差の符号が反転した場合の不成立判定も用意した。推定と bootstrap は問題を独立単位とし、同じ問題の反復を別標本として扱わない。

## 検証

`cargo test -p routing-eval stats` は成功（5 tests passed）。試験は Wilson の既知値、seed の再現性、exact McNemar の既知表、paired 区間、Pareto、D1 の成立・同等集合・k=1 と held-out の規則を確認する。

CLI `stats` は集計器から呼ぶライブラリ処理とし、単独の入力形式はまだ設けていない。
