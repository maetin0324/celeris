---
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
---
# ADR 2026-10-30: A raw は transcript と測定者のメタデータから生成する

状態: 採用。

既存の ops-r1-pipeline-manifest の D4 を具体化する。`ops-r1-a-raw --transcript <jsonl> --metadata <json> --out <json>` を追加する。
メタデータは trial_id・condition_id・started_at・ended_at・stop・exit_code・claude_code・workspace_sha256_initial・session_id・cost_usd。
時刻は RFC3339。stop は測定者が exited/cutoff/token_cap/operator_stop を指定し、transcript の中断から推測しない。
報告額はメタデータの cost_usd（無ければ null）で受け、単価から推計しない。

usage は operator_usage::convert の session/message ID による重複除去を再利用する。
指定 session の行だけを集計する。4 欄は各欄について欠測を null とし、空 transcript も観測済みの 0 にしない。
models は指定 session の assistant message の model の集合をソートする。別 session の記録は混ぜない。
不正な時刻・逆転した区間・空の識別子・不正な SHA-256・負の金額・exited で exit_code 欠測は入力エラーにする。

attribution は既存の読み取り専用 open_copy を使い、runs.model/adapter を model/harness として出す。
chat_runs の model/harness は null（assemble の固定票照合は別の担当）。schema は /2 に置き換える。
