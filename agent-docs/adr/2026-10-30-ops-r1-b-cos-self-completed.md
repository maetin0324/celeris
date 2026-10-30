---
tasks: [01M6617H9V0TZGQKFA9J82141A]
---
# ADR 2026-10-30: 案件 CoS が起票せず自分で済ませた B を通常運用として集計する（`--b-cos-self-completed`）

状態: 採用。

## 背景

ops-r1 の B（Celeris 通常運用）の再測定 B-t002（condition `mlsys-main-4-ops-r1-k4p1-B-celeris-ops-r2`）では、案件 CoS が task を起票せず、
CoS thread の作業場で分析を済ませて chat で報告した。trace の created・executed・outcome・report は null（`no_task_create_operation`）。
人は 2026-10-30 08:20Z ごろ「Bも測り直して、含めないで、CoSがやったのも通常運用で」と決めた（判断 3: CoS が自分で済ませた場合も
条件 B の結果として扱い、それ自体を drift にしない）。

従来の `ops-r1-assemble` は outcome stage が無い B を `timeout`（wall 1800 秒）にし、checker は task の run が 0 件の試行を
「set fixed declared but no run was observed」で拒否した。B-t001 の停止（`--b-stop`）の例外は停止の reason だけに効く。

## 決定

- D1 `ops-r1-assemble` に引数 `--b-cos-self-completed`（値なし）を足す。測定者が判断 3 の規則をこの試行に当てると明示したときだけ効き、
  渡さなければ出力は従来と byte 一致する。`--b-stop` と同時に渡すと error（停止が先に決まった試行に判断 3 は当たらない）。
- D2 前提の検査: trace の created stage の reason が `no_task_create_operation`、trace の intake が CoS run（`cos_run_id`）を指し、
  attribution にその run の `chat_runs` 行があり `finished_at` が読めること。外れたら error で out を作らない。
- D3 終わり方: 受付の CoS run の `finished_at` を試行の終わりとする。cutoff 以前なら outcome `passed`・reason `cos_self_completed`、
  cutoff 後なら `timeout`・reason `cos_self_completed`（wall は trial_timeout）。wall は受付からその時刻までの秒（切り捨て）。
  `passed` は executor の終了で、成功は従来どおり独立 grader の final_pass だけで数える。
- D4 消費: 試行の tokens は task の run（`source=runs`）の和のまま（task が無ければ 0）。受付の CoS run の行は停止と同じく
  共有の CoS 行（`role=cos`・`shared_overhead=true`）として records.jsonl に写し、final-accounting の `shared_unattributable.cos` に出る
  （判断 2: B の CoS の消費は試行の消費に含めず別の行で報告する）。CoS の model の照合も行う。
- D5 checker: reason が `cos_self_completed` の試行は停止の試行と同じく、observed が 0 件でも「no run was observed」「no worker run
  observed」にしない。observed に run があれば照合は従来どおり。
- D6 trace（`ops-r1-trace/1`）は変えない。CoS の報告は trace の report stage ではなく attribution の chat run の終了時刻で受ける。

## 採らなかった案

- created が null なら自動で D3 を当てる: 旧 condition（B-celeris-ops、起票→planner/worker/reviewer）では起票しないこと自体が
  条件外なので、測定者の明示なしに成功側へ倒さない。
- CoS run を observed に入れる: set の固定 model（worker の sonnet）と照合されて違反になり、CoS を試行の外の共有消費とする §4.4 と矛盾する。
