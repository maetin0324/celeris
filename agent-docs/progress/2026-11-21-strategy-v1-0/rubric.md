# 人が独立に採点する rubric（戦略層 v1 の評価）

子 8（対照 pilot）と子 9（最終検証）で、人が方式ごとの記録を読んで採点する。実装者・評価器（LLM）の自己評価は点に入れない。
対象の方式: (a) 旧い固定の Task tree、(b) 途中の親の評価器、(c) Evidence と複数の戦略の探索。入力は [replay](replay/replay.md) の 12 時点と
[研究ケース](research/research-case.md) の 7 時点。

## 採点の手順

1. 採点者は人 1 名以上か、人が採点者に決めた model。採点欄の採点者は `human:<名>` か `model:<id>` と書く。model の採点者には人の決定の記録（scorer・軸・決めた人 `human:<名>`・決めた日）が要り、`scripts/dev/parent-eval-blind/pilot_score.py check-graded --decision <記録の json>` が記録の無い model の採点者を通さない。実装者・CoS・operator は採点者にならない。方式の名を伏せた記録（各時点の判定・根拠・次の手・費用の記録）を受け取る。伏せ方: 記録の file 名を乱数の id に替え、方式の対応表は採点の後まで渡さない。
   子 8 の採点者は、人 rmaeda が 2026-11-27 に決めた `model:claude-fable-5-1`（記録 `agent-docs/progress/2026-11-24-strategy-v1-8/scorer-decision.json`）。子 9 で model の採点者を使うかは人の確認待ち。
2. 採点者は answer key（`replay/answer_key.json` と研究ケースの参照表）を**採点の後に**見る。先に自分の判定を書き、後で差を記録する。
3. 採点の分担（人の決定 2026-11-25）: 採点者（手順 1 の人か model）は 4 つの軸（execution success・epistemic progress・parent goal achievement・unsupported claims）を下の表の基準で 0〜2 点で付け、根拠（slice の file と行、event の (task, seq)）を 1 行書く。根拠を書けない点は 0 にする。採点者（`human:<名>` か `model:<id>`）とレビュー時間（分）も書く。model の採点者のレビュー時間は model の wall で、人の時間ではない（集計で区別する）。
   pivot の時点とコストの 2 軸は採点者が付けない。4 軸の採点の後に方式の対応表を開いてから、機械（`scripts/dev/parent-eval-blind/pilot_score.py machine-axes`）が下の表の 2/1/0 点の基準を answer key・研究ケースの参照表・`metrics.json` に当てて付け、根拠を「機械:」で始めて当てた規則と数値を書く。規則と数値は子 8 の進捗 file に書く。
   子 9 で同じ分担を使うかは人の確認待ち（子 8 の進捗の「提案」）。
4. 2 名で採点したら軸ごとの差を記録し、差が 1 点を超えた軸は話し合って 1 つにする（話し合いの前の点も残す）。
5. 結果は `agent-docs/progress/` の子 8 の進捗 file に表で残す（方式 × 軸 × 採点者）。

## 軸と基準

| 軸 | 測るもの | 2 点 | 1 点 | 0 点 | 決定的な補助の数値 |
|---|---|---|---|---|---|
| execution success | 子の仕事が受け入れ条件どおりに終わったか | 全子が reviewer と command check で合格し、差し戻しの後も同じ不具合が戻らない | 合格したが差し戻し 2 回以上か、受け入れ条件の一部を未達と明記 | 不合格のまま、または自己申告だけで done | `review_verdict` の pass の比、`reviewer_passed` の有無 |
| epistemic progress | 仮説・反証・未確定が増えて整理されたか | 各時点で仮説を明記し、反証の後に改め、未確定を未確定と書く | 仮説はあるが、反証の後の改めが遅い（2 時点以上） | 仮説が無いか、反証を無視して同じ主張を続ける | 改めた仮説の数、反証から改めまでの時点の数 |
| parent goal achievement | 親の目的（原文の受入条件）の実測の改善 | 親の指標が改善し、実測の根拠がある | 一部の指標が改善、残りは未達と明記 | 改善が無いのに達成と書く、または前後比較が無い | 親の指標の before/after（アーキv4 は循環 17・SCC・S1/S2・LoC） |
| unsupported claims | 根拠の無い主張の数 | 0 件 | 1〜2 件で、後の時点で自ら訂正 | 3 件以上か、訂正が無い | 主張ごとに slice の中の根拠の有無を数える（無い = 1 件） |
| pivot の時点 | 未達を検知して手を変えた時点 | 最初に検知できた時点（replay は E-mid）か 1 時点後までに discriminating_experiment・pivot・ask_human のどれか | 2〜3 時点後 | 4 時点以上後か、最後まで変えない（史実のアーキv4 はこれ） | 検知の時点の順位の差、誤った pivot（answer key で on_track の時点の pivot）の数 |
| コスト | 結果を得るまでの費用 | 検知の後の worker の run と費用が史実の 25 % 以下 | 25〜75 % | 75 % を超える | 史実: 検知の後 137 run・264.25 USD（list price）。人の採点・review の時間（分）も書く |

## 合格線

- 方式 (b)・(c) が (a) より良いと言える条件: **pivot の時点が 2 点**、**unsupported claims が 1 点以上**、**parent goal achievement が 0 点でない**（未達なら未達と書いた 1 点を含む）、の 3 つを replay と研究ケースの両方で満たし、
  合計点が (a) を 3 点以上上回ること。
- どの方式でも、execution success が 2 点で parent goal achievement が 0 点の記録（子の done だけで親の成果と扱った記録）は、合計点によらず不合格にする。
- 誤った pivot（answer key で on_track・unknown の時点で pivot した）が 2 回以上なら、pivot の時点の軸を 0 点にする。
- 採点者 2 名の合計点の差が 4 点を超えたら、採点をやり直す（基準の読み違いとみなす）。

## 史実（方式 (a)）の参照の点（子 0 の推定。人の採点で上書きする）

| 軸 | 推定の点 | 理由 |
|---|---|---|
| execution success | 2 | 子 A〜H は reviewer の合格で done |
| epistemic progress | 0 | 「狭い pilot で循環が減る」を D・E・F の反証の後も改めなかった |
| parent goal achievement | 1 | 循環 17 → 17・LoC +919 などの未達を、親の closeout が未達と明記した |
| unsupported claims | 未採点 | 主張ごとの数え上げは子 8 で行う |
| pivot の時点 | 0 | 親の最初の評価は 2026-11-20T14:12Z（12 時点のどれにも入らない） |
| コスト | 0 | 検知の後に 137 run・264.25 USD |
