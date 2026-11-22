---
title: 戦略層 v1 子 4 rdc-path — RDC の celeris-chat 経路で thread_id・project_id・出典で続ける試験と ChatGPT・Muse 向けの手引き
tasks: [01M7XQ9QK1KC34Q2A6PJ960TJB]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# rdc-path（WorkUnit。子 4 の段 rdc）

完了日: 2026-11-22。設計は [ADR 2026-11-22-external-llm-discussion-exchange](../../adr/2026-11-22-external-llm-discussion-exchange.md) の
D6・D7.1・D7.2。基準は `$CELERIS_WU_BASE`（この WU の base）。

## 結論

- 試験 `crates/celerisctl/tests/rdc_celeris_chat.rs`（2 件）を足した。今の RDC の経路 `scripts/rdc/celeris-chat` → `celerisctl mcp call` →
  MCP を、試験の MCP server（`127.0.0.1:0`、token の認証）に向けて実際に通す。script はそのまま実行する（shebang と環境変数）。
  `CELERIS_BIN` は試験で build した `celerisctl`、`CELERIS_MCP_URL` は試験の server、`CELERIS_MCP_TOKEN_FILE` は試験の一時 file（0600）。
  外部のネットワークには出ない。
- client `chatgpt-rdc-2` の scope は `scripts/rdc/setup-chatgpt-rdc.sh` の `mcp client scopes chatgpt-rdc-2 --scope …` の値を読んで作る。
  人が本番で使う推奨 scope のまま、道具の可否を確かめる。
- `scripts/rdc/celeris-chat`・`celerisctl mcp call` は変えていない（ADR D6）。production の Rust の変更は無い。
- 手引き: `docs/guides/mcp.md` に §9（3 つの経路 (a) MCP の client・(b) RDC・(c) Markdown/JSON の交換、人の共有の手順、作らない・使わない機能）を
  足し、§8 の図と §8.3 の例に議論の道具を足した。`docs/guides/chatgpt-rdc.md`（RDC の専用ユーザーの `~/CELERIS-CHAT.md` になる）に
  §5（3 道具の呼び方と続け方）と §6（Celeris が読まないもの・しないこと）を足した。
- 新しい表・列・migration・Manager・trait・状態機械・Agent サービスは無い。

## 実装前 brief

1. 根本原因: ChatGPT が今使う RDC の経路（script → `celerisctl mcp call` → MCP）を、議論の 3 道具で通した試験が無い。MCP の試験
   （`mcp_discussion_`・`mcp_external_scope_`）は HTTP を直接話すので、script・`celerisctl mcp call` の引数と token file と出力、setup の
   scope の一覧が壊れても通る。手引きには議論の 3 道具と MCP 非対応の経路が書かれていない。
2. 責務 owner: 経路は `scripts/rdc/celeris-chat` と `celerisctl mcp call`（`crates/celerisctl/src/commands/mcp.rs`・`crates/celeris-mcp/src/call.rs`）。
   試験は実 binary を起動する celerisctl の統合試験に置く。手引きは `docs/guides/mcp.md`・`docs/guides/chatgpt-rdc.md`。
3. 既存の再利用先: `celeris_mcp::serve`・`McpState::from_store`・`auth::hash_token`、store の `chat_message_post_as`・`chat_exchange_share`
   （人の共有）、`tests/common` の `celerisctl_path()`（nextest の実行時の binary の path）。
4. 選択肢: (a) celerisctl の統合試験で script を実 binary で実行する（採用。経路の全部を通る）。(b) celeris-mcp の試験で `call::call_tool` を
   呼ぶ（不採用。script と CLI の引数・token file・exit code を通らない）。(c) script に試験用の分岐を足す（不採用。D6 は script を変えない）。
5. 新しい状態・分岐・fallback・port: 無し。dev-dependency に `tokio`（`rt-multi-thread`・`net`・`time`。既に lock にある版）を足した。
   試験の server を別 thread の runtime で動かし、本体の thread で script を同期に実行するため。
6. 撤去対象: 無し（置き換える仕組みが無い）。
7. 不変条件: RDC の経路で snapshot → propose → 次の snapshot が `thread_id`・`project_id`・出典で続く。共有していない発言は出ない。
   同じ propose の再送は 1 件。推奨 scope の RDC の client は決定・計画・task の承認に答えられない。script の出力と監査に token が出ない。
   試験は下の 2 件。

## 試験（接頭辞 `rdc_celeris_chat_`、2 件）

| 試験 | 守る不変条件 |
|---|---|
| `rdc_celeris_chat_snapshot_propose_continue` | `project_id` だけの snapshot が案件の固定の `thread_id`・`project_id` と、人が共有した発言 1 件の `citation`・`content_hash` を返す（共有していない発言は出ず、検索にも当たらない）。検索は同じ `citation` を返す。snapshot の `thread_id` と `citation`（`kind: chat`）を付けた propose は同じ thread に 1 件記録され、再送は `duplicate: true` で 2 件目を作らない。次の snapshot は `thread_id` で続けても `project_id` だけで始め直しても、案を `kind: proposal`・`actor = mcp:chatgpt-rdc-2`・`source.label = chatgpt`・同じ `references` で返す。案は `discuss` の `mcp:` の発言。同じ client の `--list` に `tasks:decide` の 4 道具は無く、呼ぶと exit 1・`-32601`。task・決定は 0 件のまま。`mcp_calls` に client の呼び出しが残り、議論の道具の `request` は NULL |
| `rdc_celeris_chat_token_not_in_output` | `--list`・成功・拒否された propose・scope の外の道具・JSON の誤り（exit 2）・誤った token（401）・token file が無い場合のどれでも、標準出力・標準エラーに token の値が出ない。`mcp_calls` にも出ない |

既存の試験がこれを検出できない理由: script を実行する試験が base に無い。MCP の試験は HTTP を直接話し、setup の scope の一覧ではなく
試験の中の定数を使う。

### 変更前に落ちることの確認（probe。どちらも確かめた後に元に戻した）

- setup の推奨 scope を子 4 の前の一覧（`discussion:read`・`discussion:propose` が無い。`git merge-base HEAD main` の版と同じ並び）にして
  流すと、2 件とも落ちた（`2 tests run: 0 passed, 2 failed`。`discussion_snapshot: exit Some(1), stderr: error: MCP エラー -32601: unknown tool "discussion_snapshot"`）。
  子 4 の前の main では道具そのものが無いので、同じく `-32601` で落ちる。
- `celeris-chat` を「`set -x` を付け、token を file から読んで環境変数 `CELERIS_MCP_TOKEN` で渡す」形に変えて流すと、
  `rdc_celeris_chat_token_not_in_output` が `list: token in stderr` で落ちた（`1 passed, 1 failed`）。失敗の文は token を `[TOKEN]` に置き換えて示す。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p celerisctl -E 'test(/rdc_celeris_chat_/)'` → exit 0、`2 tests run: 2 passed`。
- `cargo nextest run -p celeris-mcp -E 'test(/mcp_external_scope_recommended/)'` → exit 0、`2 tests run: 2 passed`
  （手引きの `--scope` の例の数と値を数える試験。§9 には `mcp client add/scopes … --scope` の例を足していない）。
- `bash scripts/dev/test-affected.sh` → exit 0（対象: celerisctl）。`repo_health_gate: all checks passed`、nextest `201 tests run: 201 passed, 0 skipped`。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p celerisctl --test rdc_celeris_chat -- -D warnings` → exit 0。
  （`--all-targets` を付けると task-worker・campaign・celeris の既存の試験の file で clippy の error が出る。この WU の file ではない。）
- `cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `crates/celerisctl/Cargo.toml`・`crates/celerisctl/tests/rdc_celeris_chat.rs`・
  `docs/guides/mcp.md`・`docs/guides/chatgpt-rdc.md`・`agent-docs/progress/2026-11-22-strategy-v1-4/rdc-path.md`・ADR 2026-11-22 の付記だけ。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD`）

- production Rust: 0。script: 0。
- test Rust: +503（`crates/celerisctl/tests/rdc_celeris_chat.rs`）。`Cargo.toml`: +3（dev-dependency と理由の注記）。
- 文書: `docs/guides/mcp.md` +121 −1、`docs/guides/chatgpt-rdc.md` +44 −1、ADR の付記、この進捗。
- 純増の理由: RDC の経路の試験と、3 つの経路の手引きは base に無い。production の行は増えていない。

## 本番への移行（人が行う RDC home-dev での確認。この unit では本番に触れていない）

前提: 子 4 の変更を含む release を人が昇格し、[scope-guard の進捗](scope-guard.md)の「本番への移行」1〜5（`chatgpt-rdc-2` の scope の
置き換えと、`scripts/rdc/setup-chatgpt-rdc.sh` の再実行による `~/CELERIS-CHAT.md` と `celerisctl` の更新）を済ませる。
共有・取り込みの `curl` は `docs/guides/mcp.md` §9.1・§9.4 の形で、`CELERIS_API_TOKEN` に人の API token を入れて行う。token の値は記録に書かない。

1. 道具が見えること: `sudo -iu chatgpt-rdc -- celeris-chat --list | cut -f1 | grep -E '^discussion_'`
   — 期待: `discussion_snapshot`・`discussion_evidence_search`・`discussion_propose` の 3 行。
2. 人が Web の chat で案件 agent-platform（project_id `01M2WTS3DKNZBSZ2JMVB4CZMBW`）の thread に、秘密を含まない短い発言を 1 件書く。
   `GET /api/v1/chat/threads/<thread>/messages` で message id を調べ、§9.1 の `exchange/shares` で共有する（`expires_at` は翌日）。
   — 期待: 201 と `share_id`。
3. 端末で先に確かめる: `sudo -iu chatgpt-rdc -- celeris-chat discussion_snapshot '{"project_id":"01M2WTS3DKNZBSZ2JMVB4CZMBW"}'`
   — 期待: `thread_id` が案件の固定の thread、`items` に 2. の発言が `citation` 付きで出る。共有していない発言は出ない。
4. ChatGPT（RDC の接続済み）に「~/CELERIS-CHAT.md の §5 に従って、案件 01M2WTS3DKNZBSZ2JMVB4CZMBW の議論を discussion_snapshot で
   読み、thread_id と出典の message_id を示して要約して」と頼む。— 期待: ChatGPT が `thread_id` と 2. の `message_id` を示す。
5. ChatGPT に案を 1 つ選ばせ、「idempotency_key を chatgpt-rdc-check-1 にして discussion_propose で記録して」と頼む。同じ依頼をもう一度する。
   — 期待: 1 回目は `duplicate: false`、2 回目は `duplicate: true` で同じ `message_id`。propose の後に CoS の discuss の run が 1 回起きる（費用は CoS の run 1 回分）。
6. ChatGPT の新しい会話で「project_id 01M2WTS3DKNZBSZ2JMVB4CZMBW の discussion_snapshot を読んで」と頼む。
   — 期待: 5. の案が `kind: proposal`・`actor.id: mcp:chatgpt-rdc-2`・`references`（2. の発言の citation）付きで出る。
7. ChatGPT に「decision_answer で承認して」と頼む。— 期待: `-32601`（unknown tool）で失敗する。
8. 記録を確かめる: `GET /api/v1/mcp/calls?client=chatgpt-rdc-2` に `discussion_snapshot`・`discussion_propose` の行がある。
   `sudo -iu chatgpt-rdc -- celeris-chat db_query '{"sql":"SELECT id, json_extract(metadata_json, ?2) FROM chat_messages WHERE json_extract(metadata_json, ?1) = ?3 ORDER BY seq DESC LIMIT 5","params":["$.author","$.discussion_mode","mcp:chatgpt-rdc-2"]}'`
   — 期待: 5. の案が 1 行で、mode は `discuss`。この確認で task・決定・Roadmap が増えていない。
9. 後片付け: §9.1 の `revoke` で共有を取り消す。案の発言は消さない（chat の記録は残す）。
10. 結果（日時・1〜9 の可否・失敗の出力）を task 01M7XQ9QK1KC34Q2A6PJ960TJB のコメントか案件 thread に書く。token・cookie は書かない。

- ChatGPT の custom remote MCP（`docs/guides/mcp.md` §7.1）での確認は optional で、外部公開の MCP の配置は人の承認の後に人が行う。
- Muse の client は本番に無い。使うときは人が §8.2 の scope（`discussion:read,discussion:propose`）で client を発行し、6. と同じ確認をする。

## 親の目的への寄与

- 確認済み（試験）: 人の主な議論の入口である ChatGPT が今使う RDC の経路で、Celeris を正本にした議論を `thread_id`・`project_id`・出典で
  続けられる。外部 LLM の案は `mcp:` の未承認の案として残り、人の承認にならない。推奨 scope の RDC の client は承認系の道具を呼べない。
  これは「ChatGPT Chat で壁打ちし、採用すべき案を Celeris 側に送る」（原文の observability の要件と子 7 の前提）に効く。
- 確認済み（文書）: MCP の client・RDC・Markdown/JSON の交換の 3 経路と、作らない機能（Memory・会話全文の自動取得、browser の login・Cookie）が手引きにある。
- 推定: 改善 B（仮説・証拠を version・引用付きで共有する）には、外部の議論の案が `citation`（revision・sha256）付きで残ることで寄与する。
- 未確認: 本物の RDC home-dev・ChatGPT・Muse での動作（上の手順で人が行う）。

## 未解決

- ADR D4.1 の「`celerisctl` に共有・取り消し・書き出し・取り込みの subcommand を足す」は未実装（rest-import の未解決のまま）。この WU の範囲は
  `crates/celerisctl/src/` を含まないので足していない。手引きは `curl` で書いた。Web の共有の画面も無い（子 7 の範囲）。
- 要約・戦略の添付 file（`ExchangeDocument`）は MCP・REST のどちらからも出ない（mcp-tools・rest-import の未解決のまま）。手引きにそう書いた。
- `chatgpt-rdc-2` は `db:read`・`api:read` を持つので、共有の範囲の制限は DB と API の全読みには効かない（決定 `ext-client-scope` の答え b で人が受け入れた運用）。

## 提案

- `celerisctl exchange {share,revoke,export,import}` は、`crates/celerisctl/src/` を範囲に入れた unit（子 7 など）で足し、手引きの `curl` を置き換える。
