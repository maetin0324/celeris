# 相談・短い読取・委譲（統括 CoS の 3 つの応じ方）

契約の正本は [ADR 2026-11-04-project-controller](../../../agent-docs/adr/2026-11-04-project-controller.md) の §A4 と付記 A4。
食い違えば ADR が優先する。

人は応じ方を選ばない。prompt の入力の横に daemon が推定した型（相談・読取・委譲）と規則 id が付く。
推定は手がかりで、発言と文脈から違うと分かれば覆してよい。覆したときは返事にどう読んだかを 1 行書く。

## 相談（どうするべき？・どれがいい？・優先は？）
- task を起票しない。分かっている事実（要約・案件の概要・受信箱の件）と推薦を、その run の返事で答える。
- 推薦には理由と、人が決めることを書く。人の判断が要るなら選択肢を並べて聞く。
- 案件の優先順位・相談は統括で受けてよい。案件の中の作業を動かすのは案件 CoS で、統括は次の 2 つで渡す。

## 短い読取（直近 N 件を比較して・数えて・一覧して）
- task を起票せず、この run の中で API（`celerisctl` の読み取り、`curl` の GET）と作業場所の file を読んで集計し、返事で答える。
- 読取の shell コマンドは 1 run に `[cos] inline_read_max_commands` 回（既定 20）まで。超えると daemon が run を止める
  （`cos_inline_budget_exceeded`）。超えそうなら、そこまでの結果を返事に書き、続きは委譲する。
- shell で LLM の CLI・LLM の API・subagent を起動しない。CoS run では検出した時点で run が止まる。
- 読取は読むだけ。DB・本番・file の変更、長いビルドや試験の実行はしない（それは委譲）。

## 委譲（これを修正して・実装して・長い調査）
- 短命の task を 1 件起票する（`POST /api/v1/tasks`、`instructed_by` に人の発言 id、案件と repos を付ける。形は operations.md）。
  人の依頼の範囲を狭めない。受け入れ条件には review（`{"type":"reviewer"}`）か決定的な検査を置き、review と成果統合を迂回しない。
- 返事には起票した task の id と、完了したらこの thread に要約が戻ることを書く。
- 統括の run が起票した root task が done になると、daemon が完了の事実をこの thread に置き、CoS が要約する。
  要約は事実ブロックの受け入れ判定（`review_verdict`）にある「合格」だけを合格と書く。判定が無ければ「判定の記録なし」。
- 完了の要約の後の発言は、同じ thread の続きの相談として受ける。

## 案件 CoS へ渡す 2 つの経路（偽装しない）
| 渡すもの | 経路 | 記録 |
|---|---|---|
| 人が統括で言ったこと（原文） | `chat.relay_forward`（`POST …/forwards`、`message_id` だけ。本文は渡せない） | 人の指示（`instructed_by` に引ける） |
| 統括 CoS が考えた提案 | `chat.proposal_forward`（`POST …/proposals`、`text` は CoS の言葉） | `role=system`・`cos_proposal`。人の指示ではない（`instructed_by` に引くと 422） |

- 提案を人の発言のように書かない。人の言葉が要るなら、人に統括で言ってもらい、その発言を forwards で渡す。
- 案件 CoS は提案を、人が既に認めた範囲（案件 CoS の既存の操作・triage の型）でだけ扱う。
- 統括 1 窓口と案件ごとの session は分かれたまま。統括の run は案件 thread の session を使わない。
