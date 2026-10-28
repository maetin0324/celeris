# 行為者と消費の追跡を評価出力まで照合する受け入れ試験

- 日付: 2026-10-28
- 状態: 実装済み（流れ 1〜5 の試験は `crates/celeris/tests/actor_trace_flow1.rs`〜`actor_trace_flow5.rs`。各流れの記録は `agent-docs/progress/2026-10-28-actor-trace-acceptance/`）
- Task: 01M60AKVZEB61PRBKX5AX44H3A

## 決定

外部 AI を内部 CoS へ移しただけでは計測上の解決とみなさない。人の介入、AI の判断・操作、共有消費を別に照合する。
`cos` の操作を人の介入として数えず、人の判断を `cos` の判断に吸収しない。欠測は null のまま残し、0 にしない。
消費の共有小計を試行へ割り振らず、直接消費と二重加算しない。

| 流れ | 既存の入口 | 照合する値と合格条件 |
|---|---|---|
| 1 認可内の要求を案件 CoS が処理 | decision の plan_answer / decision_resolve_apply_tx、CoS operations audit、受信箱 triage、decision log export、human-ops report、eval requests | 同じ task・decision・item・operation・run・thread を結ぶ。人の介入 0、cos_project の操作 1、理由・policy_version が保存される。要求は answered、解決者は cos_project、escalation と人待ちは 0。usage の thread_id・thread_kind=project・project_id と入力/出力/cache token・定価換算・壁時間を照合する。実支出の欠測は null |
| 2 人へ転送して判断者と実行者を分離（B3） | 受信箱 escalation、chat relays / instructed_by、CoS operations audit、decision log export、human-ops report、eval requests | request_ref と item/revision、転送開始と人の応答、relay の message_id・said_at・適用範囲・relayed_by を結ぶ。decided_by=human、executed_by=cos_project/cos_general または external_operator。人の判断 1 を実行者の件数へ置き換えない。人待ちと人の作業時間を区別する |
| 3 案件横断の停滞（C3） | task_ops::stagnation、human_inbox、統括 CoS の triage / operations、decision log export、human-ops report、eval requests | 複数案件の finding の論理キー・revision・根拠となる最終 event/待ち/verify 時刻を照合。統括 thread の cos_general が根拠・policy とともに対応するか、同じ要求を人へ escalate。消費は cos:general、案件を推測で付けない |
| 4 operator の配備・rollback（D2） | release の deployment_audit、CoS operations audit（CoS 経由の場合）、decision log export、human-ops report、eval requests | actor_kind=external_operator、actor_id、対象 release、from/to、operation、reason、policy_ref、task_refs/decision_refs、verify と status/exit_code を照合。判断者が人なら人の記録と参照で結ぶ。release 操作を task event と推測で同一視せず、未帰属・消費欠測はそのまま |
| 5 再配送・担当の再起動 | triage ingest/claim/requeue、CoS operations の冪等キー、decision log の投影と export、human-ops report、eval requests | 同じ source triple・operation key・usage dedup_key の再送後も、操作・消費・人の介入の件数と合計が不変。論理要求は source_kind/source_key の 1 行。再起動前後の run は履歴に残り、実際に別 run で発生した消費を消さない |

## 試験境界と共有部品

- `crates/celeris/tests/actor_trace/support.rs` を各 flow の `#[path]` で読む。共有関数は追加で拡張し、既存の意味を変えない。root の tests/support は変更しない。
- 一時 SQLite と固定 UTC を使う。既存の `tests/support/http_async.rs` の偽 HTTP を再利用し、決まった CoS 応答・usage を返す。接続先は loopback のみ。LLM、外部ネットワーク、sleep、負荷試験を使わない。
- 流れ1は API と同じ decision の計画・transaction 適用、CoS 監査、triage claim/resolve、chat run 終了、daemon と同じ decision log 投影を通す。HTTP 認証・LLM harness 起動自体は既存の task-api/task-worker 試験の責務で、この試験の対象外。
- export は `SqliteStore::decision_log_export_campaign`、report は `celeris_campaign::human_ops_report::build`、requests は `SqliteStore::request_ledger`。report の入力となる評価者の生記録（human-ops /2）は、試験の中で実際に行った操作（一時 DB の人の回答、隔離 fixture の配備結果）から作り、`Harness::outputs` で既存 builder へ渡す。期待する report JSON を手で作らない。未観測の人の総労働時間・消費・人待ちを 0 と主張せず null のまま残す。
- 流れ 4 は人の判断（human/manual の観測記録、判断者 human）と operator の実行（external_operator、promote・rollback 成功と start 失敗）を生記録にし、report の history・主体別件数・人の介入数と時間を、export・requests・deployment_audit の release・decision・task・request_ref・policy と照合する。report の history は targets・outcome を持たないので、logical_op_id で入力の生記録に結んで照合する。operator の所要時間は人の時間に足さない。
- 流れ 5 は既存の人の回答に結ぶ human/manual の記録（人待ちと人の能動作業）を観測ログに足し、人の介入 1・人の能動時間が既知の正値の report を基準にする。同じ record_id の再送（request・event 再配送の直後と、SQLite store とログを開き直した後の 2 回）はどちらも重複として捨てられ、report の件数・時間・主体別件数・history と全体が変わらない。
- 生記録は support.rs の既存 helper（流れ 4 は `human_record_base`）と、流れ 5 の補修で足した `record_human_answer` で書く。`test-affected.sh` が通常の celeris integration test として選ぶ。

## 付記: 固定時計の入口

既存の CoS 操作保存・event 追記・decision transaction は内部で壁時計を読んでいた。
各処理に時刻を受け取る `_at` 入口を追加し、既存の入口は壁時計を渡して同じ実装へ委譲する。
監査 event も注入した時刻で追記し、保存済み events の時刻を書き換えて試験を成立させない。
機能や保存形式は変えない。

## 付記（2026-10-28、流れ 2）: 人へ回した後の CoS の operation を要求に寄せる

[論理要求の台帳](2026-10-27-eval-request-ledger.md) の欠落を流れ 2 で見つけて直した（付記はこの ADR に置く。台帳 ADR の本文は変えていない）。
人へ回した item は `escalated` のまま `operation_id` を持たない。その後に CoS が取り次ぎ（`instructed_by`）を根拠に
答えた operation は item から辿れず、`operations` と台帳 ADR D8 の `instructed_by:`・`relay:` が台帳から落ちていた。

- 要求の item から辿れない operation のうち、対象が要求の decision（`target_kind=decision`、`target_id` が key の decision id）か
  受信箱の件（`target_kind=inbox`、`target_id` が key）で、作成が最初の revision の開始以後、state が
  `applied`・`superseded`・`needs_remediation` のものを同じ要求に寄せる（`rejected`・`pending` は寄せない）。
- 寄せた operation は `operations` と `policy_refs`（方針版・`instructed_by:`・`relay:`）に出す。解決の主体は従来どおり
  解決 event の `by` で決める。migration は無い。
- 試験: `request_ledger_tests::later_operation_on_an_escalated_request_is_linked_by_its_target`、
  `crates/celeris/tests/actor_trace_flow2.rs`。

## 付記（2026-10-28、流れ 2）: 取り次ぎの検査（D3）に時刻を渡す入口

流れ 2 は固定時計で [指示の取り次ぎ](2026-10-27-relayed-instructions.md) D3 の検査を通す。
`task_api::cos::operations::inbox_answer_instruction_at(store, ctx, message_id, item_id, task_id, now)` を足した。
route が `inbox.answer` で使う検査（`instructed_message`）と payload・reason の頭を、時刻を受け取って返す。
route は従来どおり壁時計を渡し、検査の内容と保存形式は変えていない。

## 関連

- [介入の主体](2026-10-27-decision-log-intervention-actors.md)
- [CoS 消費の帰属](2026-10-27-cos-usage-context.md)
- [論理要求の台帳](2026-10-27-eval-request-ledger.md)
- [指示の取り次ぎ](2026-10-27-relayed-instructions.md)
- [案件の停滞](2026-10-27-project-stagnation-inbox.md)
- [配備監査](2026-10-27-deployment-audit.md)
