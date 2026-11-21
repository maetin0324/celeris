# 再利用できる口と未実装の口（基準 main 7f69057907ad649829ec5a08b5ac7c892b9fe055）

この表は [inventory.tsv](inventory.tsv) から [inventory.py](inventory.py) が作る。行番号は基準 commit の file で pattern に最初に合う行。
`python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/inventory.py --check` が、reuse・partial・risk の行は基準と HEAD の両方で行が引けること、
missing の行は基準の `crates/`・`control-ts/`・`web/`・`scripts/` で `git grep -P` が 0 件であることを確かめる（57 行、失敗 0）。

状態の意味: **reuse** はそのまま使える口。**partial** は似た口があるが戦略層の要件（仮説・反証・versioned draft）を満たさない。
**risk** は今の口のままだと脅威モデルの前提を破りうる所（設定で塞ぐか、子 1・4 で確かめる）。**missing** は基準に無い口（作るのは担当の子）。

## 要点

- 人の目的の正本（P2 `Project.request`）、承認済み Roadmap（RM1〜RM3、decide は人の token だけ）、controller の wake → snapshot → TS の決定 → Rust の gate → 効果（C1〜C6）は揃っている。戦略層は新しい Goal の正本を作らず、これらを読む側に置ける。
- CoS Chat は thread・message・relay（operator だけ）・SSE・添付・FTS5 の履歴検索（H1〜H10）が揃っている。`proposals` の口（H6）は統括 CoS の転送だけで、versioned な戦略案の記録ではない。
- MCP は `console_instruct`・`console_reply`（thread_id・relay）と `knowledge_propose` がある（M1〜M3・M7）。外部 LLM の入口に使える。
- risk: MCP の `decision_answer`・`task_plan_gate`（M5・M6）は scope `TasksInteract` の client なら人の決定・plan の承認に答えられる（記録は `by=mcp:<client_id>`）。
  ChatGPT・Muse の client にこの scope を渡すと、外部 LLM の発言が承認として効く。本番の client の scope の設定は未確認（子 1・4 で確かめる）。
- 外部公開・本番の変更を人に限る拒否は goal_guard にある（P5・P6）。ただし対象は案件 CoS の operation で、publish の manifest・人の署名は無い（X6）。
- 基準に無いもの: 仮説・戦略候補・epistemic progress・親の途中評価・discuss-only の mode・publish の署名・Muse・外部の会話の抜粋を同意つきで取り込む口（X1〜X8）。

## 表

| id | 状態 | 領域 | 場所（基準 commit の file:line） | 説明 |
|---|---|---|---|---|
| P1 | reuse | project | `crates/task-core/src/org.rs:354` | 案件の本体（DB の projects 行） |
| P2 | reuse | project | `crates/task-core/src/org.rs:358` | Project.request。人の元の目的の正本。戦略層はここを読むだけで書かない |
| P3 | reuse | project | `crates/task-api/src/cos/goal_guard.rs:30` | CoS が request を変える要求は 422（人の発言の引用が無いとき） |
| P4 | reuse | project | `crates/task-api/src/cos/goal_guard.rs:31` | draft でない task の objective・acceptance を CoS が変えると 422 |
| P5 | reuse | publish | `crates/task-api/src/cos/goal_guard.rs:34` | 案件 CoS の外部公開は人の引用か authority の許可が要る（publish の既存の拒否の口） |
| P6 | reuse | publish | `crates/task-api/src/cos/goal_guard.rs:33` | 本番の変更は人の引用か許可が要る |
| P7 | partial | project | `crates/task-core/src/project_brief.rs:198` | ProjectBrief: 派生の要約（objective・現状・evidence_refs）。未検証の要約として扱う。仮説・反証の欄は無い |
| P8 | reuse | project | `crates/task-api/src/handlers/projects.rs:591` | brief を書けるのは案件 CoS thread の run だけ（D3） |
| RM1 | reuse | roadmap | `crates/task-core/src/roadmap.rs:179` | 承認済み Roadmap（版）。戦略層は読むだけ |
| RM2 | reuse | roadmap | `crates/task-core/src/roadmap.rs:118` | Roadmap の提案（proposed の版。今の版を変えない）。戦略案の versioned draft の前例 |
| RM3 | reuse | roadmap | `crates/task-api/src/handlers/roadmap.rs:107` | Roadmap の decide・Outcome の status は人の token だけ（403） |
| RM4 | reuse | roadmap | `crates/task-core/src/roadmap.rs:146` | Outcome の記録（status・evidence） |
| RM5 | reuse | roadmap | `crates/task-core/migrations/0113_project_roadmap.sql:67` | Outcome の evidence の列（root の task を根拠に取る） |
| C1 | reuse | controller | `crates/task-dispatch/src/dispatcher/project_wake.rs:188` | wake（出来事 → 案件の wake） |
| C2 | reuse | controller | `crates/task-ops/src/project_snapshot.rs:305` | snapshot（決定的な入力の組み立て） |
| C3 | reuse | controller | `control-ts/src/domains/project_controller.ts:289` | TS の決定器（LLM なし） |
| C4 | reuse | controller | `crates/task-dispatch/src/dispatcher/project_decider_ts.rs:520` | Rust の gate（TS の提案の違反を拒否） |
| C5 | reuse | controller | `crates/task-dispatch/src/dispatcher/project_decider_ts.rs:274` | Outcome の達成を Rust が根拠で確かめる |
| C6 | reuse | controller | `crates/task-dispatch/src/dispatcher/project_guard.rs:114` | 予算・停滞・同じ失敗の guard（decider の前。LLM なし）。子 6 の停止はここを使う |
| H1 | reuse | chat | `crates/task-core/src/chat/mod.rs:176` | CoS Chat の thread（案件 thread を含む） |
| H2 | reuse | chat | `crates/task-core/src/chat/mod.rs:207` | message |
| H3 | reuse | chat | `crates/task-api/src/chat/mod.rs:69` | REST: 人の発言 |
| H4 | reuse | chat | `crates/task-api/src/chat/mod.rs:313` | REST: relay（人の指示を operator が運ぶ） |
| H5 | reuse | chat | `crates/task-api/src/chat/mod.rs:327` | relay は operator の principal だけ（403） |
| H6 | partial | chat | `crates/task-api/src/chat/mod.rs:373` | proposals は統括 CoS の転送だけ（直接は 403）。versioned な戦略案の記録ではない |
| H7 | reuse | chat | `crates/task-api/src/chat/stream.rs:36` | SSE |
| H8 | reuse | chat | `crates/task-core/src/chat/store.rs:769` | 履歴の検索（FTS5） |
| H9 | reuse | chat | `crates/task-core/src/chat/mod.rs:522` | relay の要求の形 |
| H10 | reuse | chat | `crates/task-api/src/chat/attachments.rs:28` | 添付 |
| M1 | reuse | mcp | `crates/celeris-mcp/src/tools/console.rs:120` | MCP: CoS chat への指示（thread_id・relay 指定） |
| M2 | reuse | mcp | `crates/celeris-mcp/src/tools/console.rs:239` | MCP: 返事の取得 |
| M3 | reuse | mcp | `crates/celeris-mcp/src/tools/console.rs:37` | relay の記録（人の発言の verbatim を運ぶ。主体は mcp:<client>） |
| M4 | reuse | authz | `crates/task-core/src/mcp.rs:21` | MCP の scope（KnowledgeRead・KnowledgePropose・TasksRead・TasksInteract・TasksDecide・ConsoleInstruct ほか） |
| M5 | risk | authz | `crates/celeris-mcp/src/tools/decisions.rs:144` | scope TasksInteract の client は人の決定に答えられる（by=mcp:<id>）。外部 LLM の client に渡すと承認と取り違える（脅威 T3） |
| M6 | risk | authz | `crates/celeris-mcp/src/tools/decisions.rs:194` | 同上（plan の承認） |
| M7 | reuse | knowledge | `crates/celeris-mcp/src/tools/knowledge.rs:370` | MCP: knowledge の提案（受信箱へ。正本は変えない） |
| M8 | reuse | knowledge | `crates/celeris-mcp/src/tools/knowledge.rs:149` | MCP: 検索（KnowledgeRead） |
| K1 | reuse | knowledge | `crates/task-api/src/knowledge.rs:48` | knowledge の受信箱（記録＝提案） |
| K2 | reuse | knowledge | `crates/task-api/src/knowledge.rs:50` | 受信箱の採否 |
| A1 | reuse | authz | `crates/task-api/src/principal.rs:24` | 主体（Human・Operator・Mcp・Cos・Unauthenticated）。認証だけが作る |
| A2 | reuse | authz | `crates/task-api/src/principal.rs:56` | config の token は mcp・cos を名乗れない（外部 LLM が human を名乗れない根拠の 1 つ） |
| A3 | reuse | authz | `crates/task-api/src/handlers/authority.rs:129` | authority の操作は人だけ |
| A4 | reuse | authz | `crates/task-core/src/authority.rs:160` | authority ledger の許可（予算つき・監査つき） |
| W1 | reuse | ui | `web/features/projects/project-controller-panel.tsx:238` | 案件の自動運営の画面 |
| W2 | reuse | ui | `web/features/projects/project-roadmap-panel.tsx:128` | Roadmap の画面（人の decide） |
| W3 | reuse | ui | `web/features/projects/project-authority-panel.tsx:144` | authority の画面 |
| W4 | reuse | ui | `web/routes/projects.$id.index.tsx:2` | 案件の詳細の route |
| W5 | reuse | ui | `web/features/chat/composer/chat-composer.tsx:34` | Chat の入力欄（添付つき） |
| V1 | partial | evidence | `crates/task-worker/src/protocol.rs:1203` | worker の受け入れ条件の証拠（result.json）。研究の仮説・反証の evidence ではない |
| X1 | missing | strategy | `git grep -P '(?i)hypothes'` → 0 件 | 仮説の記録（Strategy/Hypothesis/Evidence 契約）が無い。子 2 |
| X2 | missing | strategy | `git grep -P '(?i)strategy_candidate\|StrategyCandidate\|struct Strategy'` → 0 件 | 戦略候補の型・比較が無い。子 2・5 |
| X3 | missing | evaluator | `git grep -P '(?i)epistemic'` → 0 件 | epistemic progress の評価が無い。子 6 |
| X4 | missing | evaluator | `git grep -P '(?i)goal_evaluator\|parent_goal_eval'` → 0 件 | 親の目的の途中評価（checkpoint）が無い。子 6 |
| X5 | missing | chat | `git grep -P '(?i)discuss[-_ ]?only\|discussion_mode'` → 0 件 | discuss-only の権限（副作用 0 の mode）が無い。子 3 |
| X6 | missing | publish | `git grep -P '(?i)signoff\|sign_off\|publish_manifest\|PublishManifest'` → 0 件 | publish の manifest と人の署名が無い。子 7 |
| X7 | missing | external | `git grep -P '\bMuse\b'` → 0 件 | Muse への言及・接続が無い。子 4 |
| X8 | missing | external | `git grep -P '(?i)excerpt_import\|import_excerpt\|chatgpt_import'` → 0 件 | 外部 LLM の会話の抜粋を同意つきで取り込む口が無い。子 4 |
