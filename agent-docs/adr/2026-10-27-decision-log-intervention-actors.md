# ADR: decision log の介入に主体を記録し、写像の漏れを直す

- Date: 2026-10-27
- Status: Accepted
- Task: 01M5XXF7Q6YWNB9ZZY0NYK3154
- 依存: [2026-10-27-event-principals](2026-10-27-event-principals.md)（A2。event の `by`）、
  [2026-10-27-human-ops-actor-roles-request-ref](2026-10-27-human-ops-actor-roles-request-ref.md)（主体の種類の語彙）、
  [2026-10-15-decision-log-export](2026-10-15-decision-log-export.md)（D7 介入）

## 背景

`decision_log_interventions`（D7）は events から人の介入を投影するが、次の問題があった。

- 介入に主体の欄が無い。`ApprovalDecided` は `by` を見ずに介入に数え、CoS の承認も人の介入になった。
- `DecisionAnswered`・`IntegrationAnswered`・計画の承認待ちの判断（`PlanGateDecided`）を写像していない。
- 人の応答待ち（`record_type: "wait"`）に decision（`DecisionRequested` → `DecisionAnswered`）の区間が無い。
- `wall_ms`（人の所要時間）は events から取れないので常に欠測。

## 決定

1. **主体の欄（D1）**: 介入に `actor_kind` と `actor_id` を足す（migration 0098。表を作り直して CHECK を付ける）。
   `actor_kind` は次の 7 種の閉じた列挙（`task_core::decision_log::InterventionActorKind`）。

   | 値 | event の `by` |
   |---|---|
   | `human` | `human:<name>`（id は name） |
   | `cos_project` / `cos_general` | `cos:<thread>` で `chat_threads.kind` が `project` / `general`。`cos_project:<thread>`・`cos_general:<thread>` もそのまま |
   | `external_operator` | `operator:<name>` |
   | `mcp_client` | `mcp:<client>` |
   | `system` | `daemon`・`worker:<run>`・`project-plan`（daemon・run が書いた記録） |
   | `unknown` | `by` が無い（A2 より前の event）、`human`・`cos` だけの丸めた値、thread が読めない・legacy の `cos:<thread>`、上に無い値 |

   語彙は human-ops の `actor.kind`（ADR human-ops-actor-roles D2）に合わせ、評価者の script の `script` の代わりに
   daemon・worker を表す `system` を置く。A2 より前の event・丸めた `human` は `unknown` にし、`human` で埋めない
   （`actor_id` には読めた値をそのまま残す）。
2. **写像（D2）**: 次の event を介入にする。区分（`kind`）は従来の D7 の 4 区分。

   | event | kind |
   |---|---|
   | `Answered` | `research_judgment` |
   | `DecisionAnswered` | `research_judgment` |
   | `PlanGateDecided`（approve / replan / withdraw） | `research_judgment` |
   | `Retried` | `recovery` |
   | `IntegrationAnswered`（自動統合の失敗からの回復） | `recovery` |
   | `ApprovalDecided` | `unclassified` |
   | `Edited` | `unclassified` |

   人以外（CoS・operator・MCP・system・unknown）の処理も行として残し、`actor_kind` で区別する。
   `Edited` も主体を問わず残す（以前は `by` が `human` のものだけ）。
3. **export（D3）**: `record_type: "intervention"` の行に `actor_kind`・`actor_id` を出す。summary の
   `interventions_by_kind` は `human_interventions_by_kind` に改名し、`actor_kind = human` の介入だけを数える。
   全主体の件数は `interventions_by_actor_kind`（7 種を全て出す。無ければ 0 件）に出す。旧い名前は残さない。
4. **人の応答待ち（D4）**: `decision_log_human_waits` に `wait_kind: "decision"` を足す。開始は `DecisionRequested`、
   終了は同じ decision id の最初の `DecisionAnswered`（revise は終了にしない）か `DecisionWithdrawn`。
   decision id で対にする（出した節点と答えの event の task が違っても閉じる）。全ての wait 行に終了の主体
   `end_actor_kind`・`end_actor_id` を出す（終了が無い・`by` を持たない終了は `null`）。
5. **`wall_ms`（D5）**: 人の所要時間は events から観測できないので欠測のまま。応答の遅れは D4 の wait 区間、
   人の作業時間は human-ops の記録（ADR 2026-10-19-eval-raw-human-ops-record）で測る。

## 範囲外

- 投影は従来どおり campaign 識別を持つ task の木だけ（本番で表が 0 行なのは campaign が無いため）。
- `IntegrationRequested` → `IntegrationAnswered` の待ち区間は足さない。

## 結果

- 評価の集計は介入を主体別に数え、CoS・operator・MCP の処理を人の介入に入れない。
- A2 より前の event は `unknown` として別に数えられる。
