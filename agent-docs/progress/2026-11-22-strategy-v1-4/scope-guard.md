---
title: 戦略層 v1 子 4 scope-guard — 外部 LLM の MCP client が人の決定・plan の承認に答えられないようにする
tasks: [01M7XQ9QK1KC34Q2A6PJ960TJB]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# scope-guard（WorkUnit。子 4 の段 wire）

完了日: 2026-11-22。設計は [ADR 2026-11-22-external-llm-discussion-exchange](../../adr/2026-11-22-external-llm-discussion-exchange.md) の D6・D7.2・D7.3。
人の決定 `ext-client-scope` の答え b（rmaeda。operator が AskUserQuestion で聞いた）に従った。

## 結論

- MCP の `decision_answer`・`task_plan_gate` の scope を `tasks:interact` から `tasks:decide` に移した（`crates/celeris-mcp/src/tools/decisions.rs` の
  `ToolDef.scope` の 2 行）。人の決定・計画・task の承認に答える MCP の道具は `tasks:decide` の 4 つ（`task_approve`・`task_reject`・`decision_answer`・
  `task_plan_gate`）だけになった。`decision_list` は `tasks:interact` のまま（読むだけ）。
- `chatgpt-rdc-2` の推奨 scope（`knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read,discussion:read,discussion:propose`）
  には `tasks:decide` が無いので、ChatGPT は決定を `decision_list` で読めるが答えられない。`db:read`・`api:read` は答え b のとおり残した。
- 文書と setup の推奨 scope を ADR D6 に合わせ、旧い推奨と「`tasks:interact` で決定に答えられる」記述を消した:
  `docs/guides/mcp.md` §4 の表・§5 の `decision_answer`・`task_plan_gate`・§8 の client 名・§8.2、`docs/guides/chatgpt-rdc.md` §1、
  `scripts/rdc/setup-chatgpt-rdc.sh` の案内。Muse など他の MCP client の推奨は `discussion:read,discussion:propose` だけと §8.2 に書いた。
- `McpScope` の doc comment（`crates/task-core/src/mcp.rs`）は変えていない。道具と scope の対応の正本は `ToolDef.scope` と mcp.md §4 の表で、
  `McpScope` の `JsonSchema` は手書きの enum なので doc comment は schema に出ない。公開 API の schema の生成物は変わらないので再生成していない。
- ADR-0079 に付記を足した（D7・D8 の「scope `tasks:interact`」はこの変更で置き換わる）。ADR 2026-11-22 の D7.2 の表に、この unit で足した試験 2 件の行を足した。

## 実装前 brief

1. 根本原因: `decision_answer`・`task_plan_gate` の `ToolDef.scope` が `tasks:interact`（コメント・`task_answer` と同じ重さ）で、外部 LLM の client の
   推奨 scope（`tasks:interact` を含む）で人の決定と計画の承認に答えられた。記録は `by=mcp:<client_id>` で人の承認には数えられないが、決定の効き目
   （待っている unit の再開・replan・取り下げ）はそのまま起きる。
2. 責務 owner: 道具と scope の対応は `crates/celeris-mcp/src/tools/decisions.rs` の `answer_def`・`plan_gate_def`（registry は `tools::all()`、
   scope の照合は既存の `crate::rpc`）。推奨 scope の正本は `docs/guides/mcp.md` §8.2 と `scripts/rdc/setup-chatgpt-rdc.sh`。
3. 既存の再利用先: `McpScope::TasksDecide`（`task_approve`・`task_reject` が既に使う）、`tools/list` と `tools/call` の scope の照合（無い道具は `-32601`）、
   `task_ops::decision::answer`・`task_ops::plan_gate::plan_gate`（変えない）、子 2 の `envelope::human_approved`（R10）。試験は `mcp_integration.rs` の
   server・client・task の helper を使った（新しい helper は `awaiting_plan_approval` だけで、既存の試験の closure を関数にしたもの）。
4. 選択肢: (a) 2 道具の scope を `tasks:decide` に移す（採用。人の答え b）。(b) 新しい scope（例 `decisions:answer`）を作る（不採用。`task_approve` と
   同じ重さの承認の操作で、意味が同じものは同じ scope にする）。(c) `tasks:interact` のまま client の推奨から外す（不採用。`task_answer`・
   `task_decompose` も使えなくなる。答え b と違う）。
5. 新しい状態・分岐・fallback・port: 無し（既存の `ToolDef.scope` の値を 2 つ変えただけ）。
6. 撤去対象: `tasks:interact` で `decision_answer`・`task_plan_gate` を呼べる経路、mcp.md §4・§5 の「scope `tasks:interact`」の記述、§8.2 と setup の旧い推奨
   scope（`discussion:*` が無く、client 名 `chatgpt-rdc`）、「`tasks:interact` のクライアントは一覧・回答ができる」を確かめていた既存の試験の正の case。
7. 不変条件: 外部 LLM の推奨 scope の client は人の決定・計画・task の承認に答えられない。`discussion:*` だけの client は DB・API・CoS への指示を使えない。
   MCP の答え（`by=mcp:`）は子 2 D2.2 の戦略の承認にならない。文書の推奨 scope は D6 と一致する。試験は下の 7 件。

## 試験（`mcp_external_scope_`、`crates/celeris-mcp/tests/mcp_integration.rs`）

| 試験 | 守ること | 既存の試験で検出できない理由 |
|---|---|---|
| `mcp_external_scope_recommended_lists_no_decide_tools` | registry で `tasks:decide` の道具は 4 つだけ。D6 の 2 つの推奨 scope の `tools/list` にどれも出ない。`chatgpt-rdc-2` は `decision_list`・`db_query`・`api_get`・`console_instruct`・議論の道具を持つ。Muse の推奨は議論の 3 道具だけ | 推奨 scope と registry を突き合わせる試験が無かった |
| `mcp_external_scope_recommended_has_no_decide` | mcp.md・chatgpt-rdc.md・setup の `mcp client add/scopes <id> --scope …` の値が D6 と一致し（`chatgpt-rdc*` 3 か所、`muse` 1 か所）、どれも `tasks:decide` を含まない。§4 の表と §5 の項で 4 道具が `tasks:decide` にある | 文書と code を比べる試験が無かった |
| `mcp_external_scope_decision_answer_needs_tasks_decide` | `chatgpt-rdc-2` の scope の client は決定を読めるが、`decision_answer`（注入文の note を含む）は `-32601`。event は増えず、決定は open のまま | 既存の試験は `tasks:interact` で答えられることを正の case にしていた |
| `mcp_external_scope_plan_gate_needs_tasks_decide` | 同じ client の `task_plan_gate` の approve・replan・withdraw は `-32601`。task の状態と event は変わらない | 同上 |
| `mcp_external_scope_task_approve_needs_tasks_decide` | 同じ client の `task_approve`・`task_reject` は `-32601`。承認 task と event は変わらない | 既存の試験は scope 無しの client だけを見ていた（推奨 scope の回帰の guard） |
| `mcp_external_scope_discussion_only_cannot_read_db_api_or_instruct` | `discussion:read,discussion:propose` だけの client は `db_query`・`db_schema`・`api_get`・`console_instruct`・`console_reply`・`knowledge_search`・`tasks_list` が `-32601`、偽の API は要求を受けない | 既存の `db_and_api_tools_require_their_scopes` は `tasks:read` の client だけを見ていた |
| `mcp_external_scope_mcp_answer_not_strategy_approval` | `strategy-approve-v1` の決定に、`chatgpt-rdc-2` の scope の client は答えられない（`-32601`）。人が明示で `tasks:decide` を与えた client が MCP で `approve` と答えても（note に「approved by human:alice」）、記録は `by=mcp:ops-bot` で、記録から作った `ApprovalRecord` に R10（`envelope::human_approved`）は false。同じ行を `human:alice` が答えた場合は true（対照） | MCP が記録した答えを R10 に通す試験が無かった |

既存の試験の変更（契約の変更による。規約 10）:

- `decision_tools_require_tasks_interact_and_record_the_mcp_source` → `decision_list_needs_tasks_interact_and_decision_answer_needs_tasks_decide`:
  client の scope を `[tasks:interact, tasks:decide]` にした。`tasks:interact` だけでは答えられないことは `mcp_external_scope_decision_answer_needs_tasks_decide` が確かめる。
- `task_plan_gate_requires_tasks_interact_and_records_the_mcp_source` → `task_plan_gate_requires_tasks_decide_and_records_the_mcp_source`:
  client の scope を `[tasks:decide]` にした。承認待ちの task を作る closure を関数 `awaiting_plan_approval` にして新しい試験と共用した。
  `tasks:interact` だけでは呼べないことは `mcp_external_scope_plan_gate_needs_tasks_decide` が確かめる。

### 変更前に落ちる probe

新しい試験を残したまま、`crates/celeris-mcp/src/tools/decisions.rs`・`docs/guides/mcp.md`・`docs/guides/chatgpt-rdc.md`・`scripts/rdc/setup-chatgpt-rdc.sh` を
`git checkout HEAD -- …`（変更前の base の内容）に戻して実行し、その後に変更を戻した:

```
cargo nextest run -p celeris-mcp --test mcp_integration --no-fail-fast -E 'test(mcp_external_scope_)'
Summary 7 tests run: 2 passed, 5 failed
```

- FAIL `mcp_external_scope_recommended_lists_no_decide_tools`: `tools under tasks:decide` が `["task_approve", "task_reject"]` だけ。
- FAIL `mcp_external_scope_recommended_has_no_decide`: `docs/guides/mcp.md: chatgpt-rdc` の値が旧い `knowledge:read,…,db:read,api:read`。
- FAIL `mcp_external_scope_decision_answer_needs_tasks_decide`: `decision_answer` が成功し `"answer":{"by":"mcp:chatgpt-rdc-2","option":"vault"}`、`status: answered`。
- FAIL `mcp_external_scope_plan_gate_needs_tasks_decide`: `task_plan_gate` が成功し `reason: plan_approved`（`blocked` → `ready`）。
- FAIL `mcp_external_scope_mcp_answer_not_strategy_approval`: `chatgpt-rdc-2` が `strategy-approve-v1` に `approve` で答えられた（`by: mcp:chatgpt-rdc-2`）。
- PASS（base でも通る回帰の guard）: `mcp_external_scope_task_approve_needs_tasks_decide`（`task_approve` は base でも `tasks:decide`）、
  `mcp_external_scope_discussion_only_cannot_read_db_api_or_instruct`（`discussion:*` は mcp-tools が足した scope で、DB・API・console を含まない）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p celeris-mcp --test mcp_integration -E 'test(mcp_external_scope_)'` → 7 tests run: 7 passed。
- `cargo nextest run -p celeris-mcp --test mcp_integration -E 'test(mcp_external_scope_) | test(decision_list_needs) | test(task_plan_gate_requires)'`
  → 8 passed（文書の試験を足す前の 6 件 + 既存の 2 件。文書の試験は上の 7 件の実行に入る）。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-mcp・celerisctl。`Summary 1067 tests run: 1067 passed (2 slow), 2 skipped`、
  doc test は全 crate で ok。repo-health は `verdict=pass`（`fail_grade=0`）。new 10・worsened 2 はどれも warning の `dup.*` で、根拠の path は先行 unit の
  file（`tools/discussion.rs`・`discussion_exchange*`・`strategy/fixtures/` など）。この unit の変えた file を含むものは無い。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の出力は `agent-docs/adr/0079-recursive-task-decomposition.md`・`agent-docs/adr/2026-11-22-external-llm-discussion-exchange.md`・
  `crates/celeris-mcp/src/tools/decisions.rs`・`crates/celeris-mcp/tests/mcp_integration.rs`・`docs/guides/chatgpt-rdc.md`・`docs/guides/mcp.md`・
  `scripts/rdc/setup-chatgpt-rdc.sh` と、この進捗 file。どれも D7.3 の scope-guard の範囲の中。

## LoC（`git diff --numstat`、この unit の commit）

| 区分 | file | + | − |
|---|---|---|---|
| production Rust | `crates/celeris-mcp/src/tools/decisions.rs` | 8 | 3 |
| test Rust | `crates/celeris-mcp/tests/mcp_integration.rs` | 495 | 30 |
| 文書・script | `docs/guides/mcp.md`・`docs/guides/chatgpt-rdc.md`・`scripts/rdc/setup-chatgpt-rdc.sh` | 44 | 23 |
| ADR | `0079`・`2026-11-22-external-llm-discussion-exchange` | 10 | 0 |

production の変更は scope の値 2 行と module の doc comment だけで、新しい型・状態・分岐・DB 表・migration は無い。試験の純増は新しい 7 件と、
既存の closure を関数にした分。

## 本番への移行（人が行う。この unit では実行していない）

この変更が入った release を人が昇格すると、`chatgpt-rdc-2` は scope を変えなくても `decision_answer`・`task_plan_gate` を呼べなくなる
（`tools/list` からも消える）。議論の道具を使うには scope の置き換えが要る。scope の変更は人が行う（credential と scope は人だけが扱う）。

1. 昇格の前に今の scope を記録する（token は出ない）:
   `~/.local/celeris/current/bin/celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client ls`
   — 期待: `chatgpt-rdc-2` の scopes が `knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read`。
2. 人が GUI の「リリース」画面からこの変更を含む release を昇格する。
3. scope を置き換える（token はそのまま。置き換えなので全部書く）:
   `~/.local/celeris/current/bin/celerisctl --db ~/.local/celeris/celeris.sqlite3 mcp client scopes chatgpt-rdc-2 --scope knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read,discussion:read,discussion:propose`
4. 確認: `celerisctl … mcp client ls` で `chatgpt-rdc-2` の scopes が 3. の値で、`tasks:decide` を含まない。RDC の専用ユーザーで
   `sudo -iu chatgpt-rdc -- celeris-chat --list` を実行し、`decision_list`・`discussion_snapshot`・`discussion_propose` があり、`decision_answer`・
   `task_plan_gate`・`task_approve`・`task_reject` が無いこと。
5. 専用ユーザーの `~/CELERIS-CHAT.md` を新しい `docs/guides/chatgpt-rdc.md` にするなら、人が `scripts/rdc/setup-chatgpt-rdc.sh` を sudo 付きで再実行する
   （冪等。TOKEN_FILE は不要）。
6. 戻すとき: 1. で記録した scope を `mcp client scopes chatgpt-rdc-2 --scope …` で書き戻す。release の rollback で 2 道具は `tasks:interact` に戻る。

- `tasks:decide` を持つ MCP client が本番にあるかは未確認（2026-11-22 の ADR の確認では `chatgpt-rdc-2` と `claude-code` の 2 つが有効で、どちらも
  `tasks:decide` を持たない）。昇格の前に 1. で確かめる。持つ client があれば、その client は昇格の後も 2 道具を呼べる。

## 親の目的への寄与

- 全体の不変条件「外部 LLM の発言を human actor や承認と取り違えない」「prompt injection で権限が上がらない」に効く（確認済み: 上の試験）。base では
  ChatGPT（`chatgpt-rdc-2`）が `decision_answer`・`task_plan_gate` で人の決定と計画の承認に答えられた（記録は `mcp:` だが効き目は起きた）。この変更の後は、
  推奨 scope の外部 LLM は決定を読めても答えられない。
- 改善 A〜D と observability への直接の寄与は無い（子 4 の安全の前提）。本番での効果は、昇格と上の scope の置き換えの後で未確認。

## 未確認

- 本番の `chatgpt-rdc-2` の scope の置き換えと、本物の RDC home-dev・ChatGPT・Muse での確認（人が昇格の後に行う）。
- `task_answer`・`task_decompose`・`task_comment` は答え b の範囲（承認系の 2 道具）の外なので `tasks:interact` に残る（ADR の「未確認」のとおり）。
- `db:read`・`api:read` を持つ `chatgpt-rdc-2` は DB・API の全読みで未発表の結果を読める（人が受け入れた運用。試験はこの経路を閉じていない）。

## 提案

- 無し。
