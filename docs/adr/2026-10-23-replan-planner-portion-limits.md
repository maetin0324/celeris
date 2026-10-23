# ADR 2026-10-23: replan の計画上限は done の持ち越しを除いて測る

---
tasks: [01M5MAPDJVDBJ1FXMDBJABD8E2]
---

## 背景

`execution-plan/3` の replan は planner の提出後に `carry_done_units_v3` で完了済みの unit・段を補う。
moshi-brewed の子 task 01M5HDSN3A では提出 4,819 bytes に履歴が加わり、68,384 > 65,536 bytes、
別の試行では 10 > 8 段で拒否された。planner に履歴の削減を求めても解消できない。

## 決定

`validate_v3` は採用用の全体 spec と、サイズ計測専用の planner 分を分ける。
planner 分は `done_work_units` と checks 以外が一致する unit、およびその unit だけを含む段を除く。
未完 unit と done が混在する段、および空の段は数える。人が done の本文を変更した場合も planner 分に数える。
採用・依存関係の検証・永続化には省略していない全体を使う。
段数は共通の `counted_plan_stages` を検証・採用時の `plan_limit_holds`・承認材料の `approval_facts` で使う。
承認材料の段あたり unit 数も done・adopt を除外する。旧い全体サイズ判定への切替は設けない。

| 上限 | 対象 |
| --- | --- |
| `max_plan_json_bytes_v3` | planner 分を compact JSON に直したバイト数。done の checks 変更は `(key, checks)` の配列を compact JSON にしたバイト数を加算する。元の objective は再課金しない |
| `max_stages` | planner 分の段数。done だけの段は除外 |
| `max_units_per_stage` | 従来どおり done・adopt 以外の unit 数 |
| `max_child_tasks_per_plan` | 従来どおり done・adopt 以外の子を作る unit 数 |
| `max_open_decisions_per_plan` | planner 分の決定。done に内包された決定と、done だけが必要とする root 決定は除外。未完の unit・段から参照される決定は数える |
| `max_rationale_chars` | 今回の rationale |
| title・objective・done_when の長さ・件数 | 従来どおり checks 以外が変わらない done を除外 |
| `max_checks` | 新しい unit および checks が変更された done。変更していない履歴は除外 |
| leaf の turns・wall budget | done 以外の実行対象 |
| `max_tree_leaves/runs/replans/tokens`、木の未回答決定・並列子・深さの上限 | 従来どおり木全体・現在の実行状態。履歴の免除なし |
| `/1`・`/2` の上限 | 変更なし（今回の持ち越し処理は `/3` 専用） |

JSON サイズは raw file の空白量ではなく従来同様のシリアライズ値で測る。
root 決定は done unit の `needs_decisions` に参照があり、planner 分の unit の参照と
`needed_before` の unit・段の対象が残らない場合のみ履歴と扱う。出所を判別できない決定は数える。
構造・key の一意性・依存・循環・done の不変条件などは全体に対して検証する。

拒否には planner / carried done / total の JSON bytes・段数・unit 数、および変更した done checks の bytes を添える。
内訳は単独で拒否しない補足情報であり、既存の「段数などの違反を limit 決定にする」分類を変えない。
上限に照合するのは planner 分であることと、木全体の上限は全体を数えることを明記する。
持ち越し bytes は全体 bytes から planner 分を引いた値で、JSON の区切りも含む。

拒否したファイルは `execution-plan.rejected.<run-id>.attempt-<n>.json` に移す。
試行番号は既存の planner 試行窓に従い、run ID により窓のリセット後も別名になる。
固定名への上書き保存は廃止し、planner への案内も新しい名前に統一する。

## 検証

30 done units / 10 done stages を持つ 64 KiB 超の履歴に小さい replan を加えた検証、
planner 分の境界値・超過・空段・変更 checks の超過・拒否文の内訳を回帰試験にする。
dispatcher の子失敗からの replan 試験でも、持ち越し後だけ JSON 上限を超える計画が実際に採用されることを確認する。
拒否ファイルは異なる試行と後の試行窓で内容が保たれることを確認する。
採用時の段数判定は done 段を免除する一方、木の leaf 上限は既存の累計値と新規 leaf の合計で止める回帰試験を置く。
