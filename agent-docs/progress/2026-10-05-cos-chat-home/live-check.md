---
title: CoS チャットホーム 実機確認（live-check）
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-07
---
# CoS チャットホーム — live-check WorkUnit

## 実機確認

2026-10-06。branch tree `c40b3669` の debug build（`cargo build -p celeris -p celerisctl`、1m30s）、
Claude Code 2.1.287、claude_oauth（人の既存ログイン）。外部への通信は Claude の LLM 呼び出しだけ。

### 環境（ADR-0126 の試験用 data dir。本番には触れていない）

- data dir: WU の成果物ディレクトリ配下の `live/`（DB `live/celeris.sqlite3`、workspaces・memory・state・kb もすべてこの下）
- `[api] listen = "127.0.0.1:17931"`（loopback）、token は一時ファイル。`[db] worker_read_only = false`（userns 不可の sandbox のため）
- provider は 1 行: `adapter = "claude-code"`、`llm_source = "claude_oauth"`、account pool なし。
  `[cos] enabled = true / harness = "claude-code" / llm_source = "claude_oauth" / tier = "frontier"`
- `[knowledge] root` は一時の `kb/` にし、`config/skills/cos-operator` と `cos-inbox-triage` を `kb/skills/` に写した
- daemon 環境: `PATH=<branch の target/debug>:$PATH`、`CELERIS_CONFIG`・`CELERIS_DB`・`CELERIS_STATE_DIR` を一時の data dir に向けた
- 本番 daemon・本番 DB・`~/.config/celeris` は読み書きしていない。試験の終わりに一時 daemon を止め、残ったプロセスが無いことを `pgrep` で確かめた

### 結果（試験用 thread `01M48GYPCJEGV0G1WY9XSEXP5D`）

| 確認 | 方法 | 結果 |
|---|---|---|
| 1 往復目 | `POST /chat/threads/{t}/messages`（合言葉「みかん42」を覚えさせる） | run `01M48GYPHB5HPZ6SQ1YQBY8VPF` completed、`session_mode=new`、返事「了解」 |
| 2 往復目が同じ session の続き | 画像を添付して「前の合言葉と画像の色」を質問 | run `01M48GZ0ZP4MPSZSYWCCTB2035` completed、`session_mode=resumed`。返事「合言葉は「みかん42」、添付画像は赤い正方形です。」 |
| session id | `node_sessions`（kind=cos_chat）と `chat_runs.session_row_id` | 3 run とも同じ row `01M48GYPHEASMG4FX4668J651V`、Claude session id `01a1110f-5a2e-434d-824c-10cd120f0275`、turns=2 → 3 |
| 画像 1 枚の添付 | `POST /chat/threads/{t}/attachments`（multipart、32×32 の赤い PNG） | 201、attachment `01M48GZ07H27H24SNQ2DD72WRM`、`image/png`、97 B、sha256 `becf0cf3…bb98`、state=ready。CoS が色を正しく答えた（画像を見た） |
| celerisctl で task を起票 | 3 往復目で branch の `celerisctl --api-url http://127.0.0.1:17931/api/v1 add …` を依頼 | run `01M48H0R9F7EYBK2Z6WJSDG2FP` completed（resumed）。task `01M48H0ZMRZ074F5CFBBGHWSWP` を起票 |
| events に actor=cos | `select json from events where task_id=<task>` | seq 0 `created`、seq 1 `{"type":"cos_operation","actor":"cos","thread_id":"01M48GYP…","run_id":"01M48H0R…","operation_id":"01M48H0ZMQCYWWRJQ41N5W0JCB","reason":"人が live-check で起票を依頼","state":"applied","target_kind":"task",…}`。`cos_operations` に action=task.create・state=applied の行 |
| streaming | `GET /chat/threads/{t}/runs/{r}/events` | `text_delta`（offset 0）、`tool`（Skill cos-operator・Bash、running→completed。Bash の詳細は redact）、`run` の終端が出た |

起票した task は試験用 daemon の中で `running`（routed=planned）になったところで daemon を止めた。この task の作業は確かめていない（範囲外）。

### 見つかった不具合（未修正）

1. **claude-code の CoS chat run が result.json 不在で必ず failed になる**（`crates/task-worker/src/claude_code.rs`）。
   - 現象: 修正前の build では返事（「了解」）が chat に流れたのに、run は `failed`、reason `worker: claude exited without .taskd/chat-runs/<run>/result.json` になった（run `01M48GT6NV2Z3VWN7X8MT83YBT`、`01M48GVWRZVR71JGR79J2AF52G`）。message も `failed` になる。
   - 原因: CoS chat の前置き（`task_worker::cos_chat::build_prompt`）は result.json を求めない（返事は本文）。一方で adapter は `terminal_from_result` で result.json を必須にしたまま。偽ハーネスの試験は result.json を書く前提なので見えなかった。
   - 一時の確認: `missing_result_json` かつ `context.cos_chat` かつ success のとき `Terminal::Done{summary: result の本文}` にする 15 行の patch を当てて、上の表の確認をした。**この patch は commit していない**（この WU の範囲外。patch は成果物 `live/cos-result-json.patch`）。
   - codex adapter も `codex exited without …/result.json` で同じ形になるとみられる（実機未確認）。
2. **CoS run の celerisctl は既定で本番の設定を読む**。`celerisctl` は `--api-url` が無いと `CELERIS_CONFIG`（無ければ `~/.config/celeris/config.toml`）の `[api] listen` に送る。CoS run の環境はこれを入れない。さらに PATH の `celerisctl` が古い release（cos_ops なし）なら `add` が DB を直接書く経路になる。本番では同じ daemon の API なので実害は出にくいが、試験用・staging の daemon で CoS を動かすと本番へ向かう。
3. **`[knowledge] root` の既定（`~/.local/share/celeris/knowledge`）に cos skill が無いと CoS は動かない**（reason `CoS unavailable: CoS skills unavailable: cos-operator, cos-inbox-triage`）。本番に入れる前に KB の `skills/` へ 2 つの skill を置く手順が要る。
4. 3 往復しても `node_sessions.summary_through_seq` は 0 のままだった（CoS が checkpoint API を呼ばなかった）。前置きでは「run を終える前に」保存を求めている。短い往復で省くのを許すか、決める必要がある。
5. `chat_runs` の実効 model は null（provider に model を書かなかったため）。

## 提案

- 1 は close-out より前に直す。CoS chat run では result.json を必須にせず、`result` の本文（または流した本文）で Done にする。偽ハーネスにも result.json を書かない場面を足す。claude-code・codex の両方。
- 2 は CoS run の環境に `CELERIS_API_URL` 相当（daemon 自身の `api_base_url`）を入れ、`celerisctl` はそれを `CELERIS_CONFIG` より優先する。PATH には daemon と同じ release の `celerisctl` を前に置く。
- 3 は `docs/ops/cos-chat.md`（ops-docs WU）の導入手順に「KB の `skills/` に cos-operator・cos-inbox-triage を置く」を入れる。

## 再現の手順（人が本番でない環境で繰り返すとき）

1. `cargo build -p celeris -p celerisctl`
2. 一時 dir に上の config（loopback listen、claude-code の provider 1 行、`[cos]`、`[knowledge] root = "kb"`、`[memory] dir`）と token を作り、`kb/skills/` に 2 つの skill を写す
3. `PATH=$CARGO_TARGET_DIR/debug:$PATH CELERIS_CONFIG=<tmp>/config.toml CELERIS_DB=<tmp>/celeris.sqlite3 CELERIS_STATE_DIR=<tmp>/state celeris --config <tmp>/config.toml`
4. `POST /api/v1/chat/threads` → `POST …/messages` を 2 回（2 回目は `POST …/attachments` で上げた画像 id を付ける）→ `GET …/runs/{r}` の `session_mode` が new → resumed、`node_sessions` が同じ row であることを見る
5. task の起票を依頼し、`events` に `type=cos_operation, actor=cos` があることを見る

## 実機確認（修正後）

2026-10-07。WorkUnit live-check2（run `01M4AJ3YEN0ZBD40H0236130CQ`）。修正後の branch tree **`5664ef5fb20e`**（result-fix・api-env・attach-handoff の統合後。patch は当てない）を
`cargo build -p celeris -p celerisctl` で debug build した（exit 0、1m29s）。

**LLM を伴う (a)〜(e) は未実施**。この run からは試験用 daemon に `claude` を起動させない。理由は 2 つある。
worker が別の LLM の CLI を起動することは禁止されている（ADR 2026-10-07-worker-no-subagents-no-llm-cli。
試験用 daemon の CoS run も task run も `claude` を起動する）。また計画承認の回答で、運用セッションが
「LLM を伴う実機確認が要る場合は Celeris の決定の要求で止めて知らせる」よう指示している。
LLM を呼ばない部分は確かめ、残りを流す台本を用意した。

### LLM なしで確かめたこと（dry）

台本 `live2.sh`（WU 成果物 `artifacts/live2/live2.sh`。手順は下の「人への依頼」）を `dry` で流した。data dir は
`artifacts/live2/dry/`（ADR-0126。DB・kb・workspaces・state・logs・backups はすべてこの下）。listen は `127.0.0.1:17932`、
token は一時ファイル、`CELERIS_*` は run の env を継がず、すべてこの data dir に向けた。

| 確認 | 結果 |
|---|---|
| 修正後 tree の daemon が起動し API が応える | pid 1092529、`GET /chat/threads` 200 |
| thread 作成 | `01M4AJBCPD1FVHNQGB8YZ6N1KC` |
| 添付 3 件の upload（multipart） | 青い PNG `01M4AJBCQ5CYXFMYTFC3DZECK5`、画面の模型 PNG `01M4AJBEQFE09Z3WNP4V872VQJ`、1 頁の PDF `01M4AJBHJGANVS95Q5NXCKT9DR`。3 件とも state=ready、sha256 つき |
| 停止と残り | process group に TERM を送り、`pgrep -f <data dir>` は該当なし |

本番 daemon・本番 DB・`~/.config/celeris` は読み書きしていない。外部への通信は無い（message を送っていないので LLM 呼び出しも無い）。

### (a)〜(e) の結果

| 確認 | 状態 | 台本での確かめ方（`full`） |
|---|---|---|
| (a) 2 往復以上で completed、`session_mode` new→resumed、`node_sessions` が同じ row | 未実施（人の判断待ち） | t1〜t5 の `GET …/runs/{r}`（`evidence/run-t*.json`）、`evidence/db.txt` の `chat_runs`・`node_sessions` |
| (b) 画像添付を CoS が読む | 未実施 | t2 で青い正方形の色を答えるか |
| (c) `--api-url` なしの `celerisctl add` が試験用 daemon に届く、events に `cos_operation` actor=cos | 未実施 | t3。`db.txt` の `cos_operations`・`events cos_operation`。台本は `CELERIS_*` を run の env から外すので、本番 API へ向かう経路は `~/.config/celeris` の既定だけ。試験用 DB に行があれば daemon 自身の API（`CELERIS_API_URL`）に届いた |
| (d) screenshot を pin した UI 修正 task、その task の添付 pin と入力 manifest | 未実施 | t4。`chat_attachment_refs`（owner_kind=task）、作業 run の `prompt.txt` の「## 入力の添付」（`evidence/input-manifest.txt`）と stage（`staged.txt`）。task が planner 経由になると manifest は WU の run で出る（planner run には stage しない） |
| (e) PDF の KB inbox candidate と provenance | 未実施 | t5。`evidence/kb-inbox*.json` の候補と `provenance[]`（sha256・thread_id・message_id） |
| codex の 1 往復 | 未実施 | 同じ理由で起動していない |

### 不具合 2〜5 の扱い

| # | 内容 | 扱い |
|---|---|---|
| 1 | claude-code の CoS chat run が result.json 不在で failed | **直した**（result-fix。偽ハーネスの `cos_chat_harness_e2e_claude_without_result_json_completes` ほか）。実機での再確認は (a) で行う（未実施） |
| 2 | CoS run の celerisctl が既定で本番の設定を読む | **直した**（api-env。run の env に `CELERIS_API_URL` と daemon の dir を先頭にした `PATH`。`cos_chat_run_launch_sets_api_url_env_and_path`）。実機での再確認は (c)（未実施） |
| 3 | KB に cos skill が無いと CoS が動かない | **文書化する**（並行の ops-docs2 が `docs/ops/cos-chat.md` に「KB の `skills/` に置く」手順を書く）。挙動は変えない（無ければ理由つき unavailable） |
| 4 | 短い往復で `summary_through_seq` が 0 のまま | **未解決**。ADR D2 は「通常の run 終了時に次回用を保存する」とするが機構で強制しない。resume が効く間は害が無く、rollover 時は未要約範囲を明示して履歴で補う設計。短い往復で省くのを許すかは人が決める（提案に記す） |
| 5 | `chat_runs` の実効 model が null | **未解決（設定の問題として文書化）**。provider に model を書かないと CLI 既定で走り、実効 model は記録されない。運用では `[cos] model` か provider の model を書く |

### 新たに見つけた点（コードを読んで。未修正）

6. CoS run の `celerisctl knowledge record`（PDF を KB 候補にする経路）は API を通らず、KB の根を `CELERIS_KNOWLEDGE_ROOT` → `CELERIS_CONFIG` の `[knowledge] root` → 既定の順で決める
   （`crates/celerisctl/src/commands/knowledge.rs` の `configured_root`）。CoS run の env は daemon の env を継ぐだけで、`CELERIS_KNOWLEDGE_ROOT` は入れない。
   daemon を `--config` だけで起動した試験用・staging の daemon では、CoS の KB 候補が本番の KB に書かれうる（不具合 2 と同じ形）。
   台本は `CELERIS_CONFIG` を export するので、この経路でも試験用の `kb/` に向かう。

### 人への依頼（LLM を伴う確認を流す手順）

本番でない shell（Claude Code の claude_oauth ログイン済み）で、修正後の tree の worktree から:

1. `cargo build -p celeris -p celerisctl`（`CARGO_TARGET_DIR` はローカル）
2. `bash <WU 成果物>/live2/live2.sh "$PWD" "$CARGO_TARGET_DIR/debug" <WU 成果物>/live2/full full`
   - 試験用 daemon を `127.0.0.1:17932` で起動し、t1〜t5（(a)〜(e)）を順に送る。各 run の終端まで最長 10 分待つ
   - 終わると `full/evidence/`（`steps.log`・`run-t*.json`・`db.txt`・`kb-inbox*.json`・`input-manifest.txt`・`staged.txt`・`daemon.log`）を残し、daemon を止めて `pgrep` の結果を `steps.log` に書く
3. 結果を確かめる: `steps.log` で t1 `session_mode=new`、t2〜t5 `resumed`、全 run completed。`db.txt` の `node_sessions` が 1 行、`cos_operations`・`events cos_operation` に actor=cos、`chat_attachment_refs` に task と knowledge_inbox の pin 各 1、`kb-inbox-*.json` の `provenance[]`
4. codex の 1 往復を足すなら、`config.toml` の `[cos] harness = "codex"`・`llm_source = "codex_oauth"` に変えた別の data dir で t1 だけ送る

## 提案（live-check2）

- 4: 「run 終了時の checkpoint」を CoS の前置きで必須にするか、N 往復ごとでよいとするかを決める。
- 6: CoS run の env に `CELERIS_KNOWLEDGE_ROOT`（daemon の `[knowledge] root`）を入れる。`CELERIS_API_URL` と同じ `cos_run_env` で足せる。あるいは KB 候補の作成を API の操作にする。
