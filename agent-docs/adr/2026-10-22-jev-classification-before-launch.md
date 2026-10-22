---
tasks: [01M5H617AKZ119RE9EGH1JMPX5]
title: Jev 分類を WU の起動に間に合わせる（先読み分類＋上限付きの起動待ち）と分類の中身の記録
---
# ADR 2026-10-22: Jev 分類を WU の起動に間に合わせる

- 日付: 2026-10-22
- 状態: 採用・実装済み
- 関連: 2026-10-15-jev-wire（D3 tick 連携。dispatcher・store に LLM 呼び出しを入れない境界は守る）、
  2026-10-15-mlsys-min-routing-policy（lane_assist・ClassificationBoard）、ADR-0074 D5.3（planner run は lane assist を飛ばす。変えない）

## Context

本番（decision_assisted・jev.enabled）で `lane_assist.classifier_status = not_attempted` が多い。分類段は daemon の loop で
`Dispatcher::tick` の**前**に 1 回だけ走り、ready の WU だけを見る。tick の中で `drain_completions` が WU を
pending→ready に上げ、同じ tick の `dispatch_ready` が起動するので、その WU は一度も分類されない（not_attempted 158 件のうち
121 件は ready→routing_decided が 6 秒未満）。また計画が最初から ready で作る WU も、計画の採用が tick 内で起きて同じ tick で
起動するため同じ。もう 1 点、`jev_ok` で `jev_lane` が null の 42 件は、Specified なのか確信不足・欠測なのか記録から判別できない。

## Decision

### D1: 先読み分類（候補 a）
分類段（`classify_tick.rs`）は ready に加え、**終わっていない全 WU**（pending 含む。統合 WU と kind task の代理 unit は LLM run を
起こさないので除く）を ready になる前に分類する。WU の spec は変わらないので、ready になった tick で起動しても分類済み。
走っている WU があるだけの Running task も対象にする（計画の無い Running task 自身は対象外）。1 回の `run` で新しく分類する件数の
上限は 8（ready を先に、残りは次の loop）。分類は tick の前で await するため、dispatch を止める長さの上限になる。

### D2: dispatcher の上限付き起動待ち（候補 c。b は採らない）
D1 でも間に合わない WU（計画の採用直後に ready で作られた WU、上限 8 を超えた分、先読みの窓外）が残る。そこで dispatcher は、
分類段が動いている間（daemon が `set_classification_wait(Some(max))` を設定する。分類段が無ければ `None`）、decision_assisted の
subject（WU id、無ければ task id）の board に項目が無いとき、起動を**見送る**（task は ready のまま・attempts は消費しない）。
次の loop で分類段が先に走り、成功でも `fell_back_to_fixed`（失敗・timeout・鍵未登録）でも項目が載るので、その次の tick で起動する。
分類が失敗・timeout のときは従来どおり fixed へ戻る（項目が `fell_back_to_fixed`）。
`Dispatcher::tick` を完了処理と dispatch に分ける案（b）は、tick の中に挟まる処理が多く変更が大きいので採らない。
dispatcher は board を**読むだけ**で、分類器を呼ばない（境界は不変）。`routing.mode = fixed`（task の固定を含む）は待たない。

### D3: 待ちの上限
`DISPATCH_WAIT_MAX` = 20 秒（subject ごと、最初に見送った時刻からの単調時計。試験は `test_policy_clock` で進める）。上限を過ぎても
項目が無ければ分類なし（`not_attempted`）で従来どおり起動する。分類段が落ちている・standby の間は `None` で待たない。

### D4: 記録する欄（`LaneAssistRecord`、`routing_decided` の `lane_assist`）
- `jev_specificity`: Jev の判定（`specified` | `needs_implementation` | `needs_investigation_design` | `insufficient_info`）。`jev_ok` のときだけ。
- `jev_confidence`: 分類の確信（成功確率ではない）。返らなければ欄ごと省く。
- `jev_confidence_missing`: 分類は成功したが確信が欠測のとき true（false は省く）。確信 0 や閾値（`DEFAULT_MIN_CONFIDENCE` 0.6）未満とは別。
これで `jev_ok` かつ `jev_lane` なしが、Specified／確信不足／欠測のどれかを判別できる。分類ログ（`jev: classification result`）にも
subject・status・specificity・confidence（欠測は `missing`）を出す。鍵・prompt 本文・応答の生文は出さない。

## 範囲外
claude_code enforce の選び方（lane を順序に入れる・証拠の層別）、planner run が lane assist を飛ばす仕様は変えない。
旧い仕組みは置き換えていないので削除対象は無い（`only_the_runnable_work_unit_is_classified…` 試験は D1 に合わせて置き換えた）。

## 本番への移行
config の変更は無い。release 後に `routing_decided` の `lane_assist.classifier_status` の `not_attempted` 率と `jev_specificity` を確認する。
