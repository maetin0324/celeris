---
title: 戦略層 v1 子 9 WU audit — 本番 DB の読み取り専用の監査（人の承認・actor・昇格と publish・Project.request と Roadmap の不変）
tasks: [01M7XQA79910VEZ66HWTX4HZVT]
status: done
completed: 2026-11-27
---

# 戦略層 v1 子 9 WU audit — 本番 DB の読み取り専用の監査

## 結論

- Project.request の sha256 は期待値と一致し、案件には Roadmap の版も Outcome の event も 1 件も無い。2026-11-21 以後に Roadmap・Outcome は変わっていない（承認済み Roadmap はそもそも存在しない）。
- 親と子 0〜9 の木（17 task）の決定は 9 件で、9 件とも needed_before の unit・段が走る前に答えられた。記録上の回答者は operator 8 件・CoS 1 件で、`human:` の actor で答えた決定は 0 件。人の選択は operator の対話（AskUserQuestion）から operator が書き写した形でだけ残る（note の書き出しによる分類で 5 件。推定）。
- 2026-11-20 以後の本番昇格は 18 回。17 回は actor が `external_operator:claude-code-ops-session`、1 回は actor の記録が無い（未確認）。worker・CoS・mcp の actor の昇格は記録に 0 件。publish の署名の決定（`publish-signoff-v<n>`）は DB 全体で 0 件。
- 2026-11-21 以後、外部 LLM・mcp の発言が human として記録された件数は 0。mcp の発言 2 件は `mcp:chatgpt-rdc-2` として記録されている。
- 人が知っておくべき点: 人の決定の記録の actor は operator で、DB だけでは「人が選んだ」ことを機械的に区別できない（note の本文にだけ書かれている）。また昇格はすべて operator が行っている（人の常設の指示による。DB・台帳には指示そのものの記録が無い）。

## 方法と再現

- DB は `file:/local/celeris/data/db/celeris.sqlite3?mode=ro` を `python3 -I` の sqlite3（`uri=True`）で開いた。書き込みはしていない。
- 昇格は DB に event が無い（`events` の type に promot・publish・signoff・release を含むものは 2026-11-20 以後 0 件）ので、本番 host の `/local/celeris/state/logs/promote-*.log`、`/local/celeris/state/logs/release-records.jsonl`、`/local/celeris/state/releases/*/audit/*.json` を読むだけにした。
- 再現: repo の root で `python3 -I agent-docs/progress/2026-11-27-strategy-v1-9/audit_query.py --out <path>`（約 1.4 秒）。出力は `audit.json`（この dir）。
- 行の本文（chat の text、決定・gate の note、mcp の request）は写していない。件数・id・時刻・actor と、note の書き出しから決めた分類（`human_via_operator`・`operator_own`・`cos_own`）だけを書いた。
- 対象: 親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE、子 0〜9（id は子 0〜8 の進捗の front matter の tasks と、この task）。DB の `parent_id` の子と一致した（`children_match_db: true`）。子孫は子 1 に 2、子 4 に 1、子 7 に 2、子 8 に 1 で、計 17 task。監査の時刻は `audit.json` の `generated_at`（2026-11-27T13:34:38Z）。

## 確認済み（DB・host の記録で直接確かめた）

### Project.request と Roadmap

- `request_sha256` = d7b584adb2c5992856f6b6a721c9ec1598b66b3d7ab1fa3610c4feeab871a699（期待値と一致）。`projects.updated_at` は 2026-09-29T10:23:35Z のまま。
- `project_roadmap_versions` は 0 行、`project_roadmap_outcome_events` は 0 行（全期間）。`roadmap_changes` は空配列。木の task に `task_roadmap_specs` の行も無い。
- ProjectBrief（`project_briefs`）は 2026-11-27 に更新されている。派生の要約で、request・Roadmap ではない。

### 決定（子ごと）

| 子 | key | 記録上の回答者 | note の分類 | 作成 → 回答（UTC） | needed_before | 間に合ったか |
|---|---|---|---|---|---|---|
| 1 | first-gate-unmet | operator | human_via_operator | 11-21 09:32 → 11:02 | gate | 間に合った（回答まで gate の run 0） |
| 1 | peer-uid-host-check | operator | operator_own | 11-21 12:18 → 12:22 | gate | 間に合った |
| 2 | prod-release | operator | operator_own | 11-21 13:43 → 14:06 | stage:design・impl・wire・close | 間に合った（最初の run は 14:06:40） |
| 4 | ext-client-scope | operator | human_via_operator | 11-22 08:50 → 08:57 | scope-guard | 間に合った |
| 6 | strategy-read-store-size | CoS | cos_own | 11-22 22:54 → 22:54 | strategy-read | 間に合った |
| 8 | llm-pilot | operator | human_via_operator | 11-24 08:53 → 08:59 | stage:harness | 間に合った |
| 8 | research-use | operator | human_via_operator | 11-24 08:53 → 08:59 | stage:harness | 間に合った |
| 8 | rubric-line | operator | human_via_operator | 11-24 08:53 → 08:59 | stage:score | 間に合った |
| 8 | plan_invalid | operator | operator_own | 11-24 18:05 → 20:30 | self | 間に合った（待つ間の run 0） |

- 「間に合った」の判定: needed_before の unit（`stage:` は段の全 unit、`self` は task 全体）の run が、決定の作成から回答までの間に 1 本も始まっていない。9 件すべて 0 本。
- 子 0・3・5・7・9 と親には決定の行が無い。
- 計画の gate（ADR-0079 D8）の決定は 12 件: operator 11 件、`human:web` 1 件（子 8、11-24 23:26 の approve）。

### 第一ゲート（子 1、task 01M7XQ9Q4P4ZQ38Y509J9XYAQ0）

- 11-21 00:53 計画 v1 の approve（operator、note の分類は human_via_operator）。孫 01M7XSGW4FH258N1K3AMW76Y6F の 02:20 の replan も同じ分類。
- 11-21 09:32 決定 first-gate-unmet（決定 id 01M7YQ5P2XZ2WTBK858NNHYMXW）を gate の worker run が出した。11:02 に replan で答えられた（operator、human_via_operator）。
- 11-21 12:22 決定 peer-uid-host-check（01M7Z0P5Y2GZKRRHNA22E0BRGP）に operator が自分で答えた（operator_own。note は、人が 11:02 の決定で任せた範囲だと書く）。
- 11-21 13:34 reviewer の pass で done。

### 第二ゲート（子 6、task 01M7XQ9QWK58ESMW42Q1TGT3WZ）

- 11-22 22:54 決定 strategy-read-store-size（01M82QEXNG1WN9RQZPWZX59629）に CoS が答えた（cos_own。note は「人の選択ではない」と書く）。
- 11-23 07:21 blocked の問い（第二ゲート未達、案 A〜C）への回答（operator、human_via_operator）。
- 11-23 07:38 計画 v5 の approve、08:42 計画 v6 の approve（operator、どちらも human_via_operator）。
- 11-23 11:47 evalrun 段の後の途中確認への回答（operator、operator_own）。
- 11-23 12:07 2 回目の未達の判定（案 1〜3）への回答（operator、human_via_operator。進捗の記録では案 1「条件付き合格」）。
- 11-23 17:03 reviewer の pass で done。

### 昇格と publish

- 2026-11-20 以後の本番昇格は 18 回（`promote-*.log` の数。rollback の log は 0）。
- 17 回は `logs/release-records.jsonl` の promoted の記録に actor `external_operator:claude-code-ops-session` がある（うち 4 回は release の dir の audit json でも同じ actor）。worker・CoS・mcp の actor は 0 回。
- publish の署名の決定（key が `publish-signoff-v` で始まる）は DB 全体で 0 行。`promotions_publications.publish_signoffs` は空。
- 子 2 の決定 prod-release の回答の時刻（11-21 14:06）に、operator が release を昇格した記録（release-records の 11-21 14:05:55）がある。

### actor（2026-11-21T00:00Z 以後）

- chat の `role=user` の発言の author の種別: `human:` 0 件、`mcp:` 2 件（どちらも `mcp:chatgpt-rdc-2`・`mcp_receipt`。1 件は人の依頼の message 01M7XPWTXR1A0CNWYN7D1HBZWB）、`operator:` 5 件（うち relay 付き 4 件、relayed_by は operator）、author 無し 175 件（review の cron の入力 158・root の完了の通知 15・CoS 不在の要約 1・legacy の経路 1。daemon・cron が書く経路）。
- 外部 LLM・mcp の発言が human として記録された件数: 0（mcp・relay の印のある user 発言で author が `human:` のもの 0、mcp が答えた決定 0）。
- `chat_events` は 10,064 件（tool 5,264・message 1,512・run 1,379・queue 667・status 618・text_delta 326・card 284・thread 14）。payload に actor の欄は無く、actor は message の側にだけある。
- MCP の呼び出しは 111 件で、すべて client `chatgpt-rdc-2`（api_get 40・tasks_get 25・tasks_list 14・knowledge_get 10・console_reply 8・knowledge_search 8・db_query 4・console_instruct 2。うち失敗 6）。この client の scope に `tasks:decide` は無い（子 4 の決定 ext-client-scope の b と合う）。最後の呼び出しは 11-26 10:01Z。
- 木の task の event の actor: `created` は CoS の thread 11 件・daemon 6 件、`cos_operation` は cos 16 件、`answered` は operator 16 件、`decision_answered` は operator 8 件・CoS 1 件、`edited`（受け入れ条件）は operator 2 件（子 8）、`integration_merged` は daemon 9 件。

## 推定（記録から推した。機械的には確かめていない）

- note の分類 `human_via_operator`（決定 5 件・計画の gate 11 件・blocked の問いと途中確認への回答 6 件、計 22 件）は、人 rmaeda が operator の対話で選び、operator が書き写したと推す。根拠は note の書き出し（「人（rmaeda）の判断です」など）だけで、DB の actor は operator のまま。人の選択そのものを示す署名や `human:` の記録は無い。
- 昇格を operator が行ったのは、人の常設の指示（verify 済みの release は operator が昇格してよい）によると推す。根拠は子 2 の決定 prod-release の note。指示そのものは DB・台帳に無い。
- author 無しの user 発言 175 件は、送り主の経路（root の完了の通知・cron の review 入力など）の文の形から daemon・cron が書いたと推す。人の発言ではない。

## 未確認

- 2026-11-20T15:43Z の昇格 1 回（log `promote-20261120-154302.log`）の actor。release-records に promoted の記録が無く、release の dir は刈られ、log に actor が無い。基準 main への昇格（11-20 17:09Z）より前で、この依頼（11-21）の前。
- `human_via_operator` の 22 件で、人が実際にその選択肢を選んだこと（operator の対話の記録は Celeris の外にある）。
- 監査の時刻より後の event（子 9 の残りの WU、親の closeout）。
- 子 9 以外の task・他の案件の昇格の事情（数は台帳から数えたが、どの task の成果かは見ていない）。

## 原目的への寄与

- 改善 C・D と observability の前提（人の承認・actor の区別）について、本番の記録で次を確かめた: 外部 LLM（ChatGPT の mcp client）の発言は human と記録されていない。決定は needed_before の前に答えられている。Project.request は不変。
- 一方で、人の承認が DB では operator の actor でしか残らない点と、昇格がすべて operator の手で行われている点は、「publish と本番昇格は人だけ」という依頼の規則を DB の記録だけでは示せない。最終報告と親の closeout で人に示す論点にする。
