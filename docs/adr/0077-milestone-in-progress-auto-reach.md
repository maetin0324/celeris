# ADR-0077: 案件計画の途中目標を dispatch で `in_progress`、`auto_advance` なら Task の `done` で `reached` にする

- 日付: 2026-09-28
- 状態: Accepted（Phase F5-1 dogfood 4 回目の「途中目標の in_progress」）
- 関連: ADR-0074 D3.1 / D3.2 / D3.5 / D3.6（案件計画・途中目標の Go・DAG 表示・`ok` の意味）、ADR-0038（途中目標の判定）、
  ADR-0044 D6（一時停止・中止）、`docs/progress/phase-F.md` の F4b 申し送り（dispatch 時の `in_progress` 未実装）

## 文脈

ADR-0074 D3.1 は途中目標（`milestones`）を「人の判定の台帳」とし、状態に `in_progress` を持つが、案件計画（D3.3）から作った途中目標は
承認で `approved` になったあと、人の `ok` で `reached` になるまで状態が変わらなかった。案件ページの DAG（D3.5）では、マイルストーン Task が
走っていても途中目標は「承認済み」のまま見える。

また D3.2 は `projects.auto_advance = true` のとき「依存先の Task が `done` で後続を進め、`reached` の判定は後から行う」と定めている。
人は今回、`auto_advance = true` の案件では Task の `done` で途中目標も `reached` にすることを明示的に求めた。本 ADR はその意味を定める。
ADR-0074 の本文は書き換えず、D3.2 の最後の文を本 ADR で**上書きする**（`auto_advance = false` の意味は変えない）。

## 決定

### D1. dispatch で `approved` → `in_progress`

- マイルストーン Task（`task_core::is_milestone_task`: 案件直下・`milestone_id` あり）が dispatch され、`Event::WorkerStarted` を
  永続化した直後に、dispatcher が `task_ops::project_plan::mark_milestone_dispatched` を呼ぶ。
- 対象は案件計画の途中目標（`plan_key` あり）で、状態が `approved` のものだけ。`approved` 以外（既に `in_progress`・`reached`・
  `paused`・`cancelled`・`redesigned`）なら何もしない。2 本目の WU・再試行・再 dispatch でも状態は変わらない（冪等）。
- 決定的な store の更新だけで、LLM は呼ばない。失敗しても dispatch は止めない（`warn` を残す）。

### D2. `auto_advance = true` の案件では、Task の `done` で一回だけ `reached`

- dispatcher の tick ごとに `task_ops::project_plan::auto_reach_done_milestones` を呼ぶ。store が 1 本の SQL で候補（案件計画の途中目標で
  状態が `approved` / `in_progress`、案件が `auto_advance = 1`、その途中目標のマイルストーン Task が `done`）を返し、該当を `reached` にする。
  Task が `done` になる経路（レビュー通過・人の承認・子の完了など）が複数あるので、遷移の各所ではなく tick の 1 か所で拾う。
- 遷移は `reached` への**一回限り**。後で Task がやり直し（rereview など）で `done` でなくなっても `reached` から戻さない。
  `paused` / `cancelled` / `redesigned` の途中目標は候補にしない（人の操作を上書きしない）。
- `auto_advance = false` の案件は従来どおり: Task が `done` でも途中目標は `in_progress` のまま「Go 待ち」で、人の `ok`（D3.6）が `reached` にする。

### D3. 後続の Go

- 後続のマイルストーン Task の Go は D3.2 の規則のまま（`auto_advance = true` なら依存先の `done` で開く）。本 ADR の `reached` は Go の条件を
  変えない。`reached` は判定の台帳の状態を揃えるためのもの。

### D4. 人のレビューとの関係

- 自動の `reached` は人の `ok` と同じ状態として扱う。`reached` 済みの途中目標への判定（`POST .../milestones/{id}/decide`）は従来どおり
  409 になる。やり直したいときは人が案件の replan（ADR-0074 D3.4）を起こす。
- `auto_advance` を立てるのは「判定を人に委ねない」という人の選択なので、後から判定を仰ぐ通知は出さない。

### D5. GUI の表示

- 案件ページの DAG の節点の途中目標バッジは、途中目標一覧と同じ色（`in_progress` = warning「進行中」、`reached` = success「達成」など）で出す。
- 節点を選んだときの `reached` の表示は従来どおり「判定済みです。」（自動か人の `ok` かは区別しない。区別が要るなら後で event を足す）。

## 結果

- DAG で「承認済み」→「進行中」→「達成」が見える。`auto_advance = true` の案件では人の操作なしで `達成` まで進む。
- tick ごとに SQL が 1 本増える（候補が無ければ空の結果）。

## ADR-0079 による廃止（2026-09-28）

案件計画（ADR-0074 D3）の廃止に伴い、本 ADR の途中目標の dispatch での `in_progress` と `auto_advance` による自動の `reached` は**廃止**する
（ADR-0079 D13、R5a で停止）。本番に `plan_key` のある途中目標は 0 行。既存の途中目標の行は状態のまま凍結する。
