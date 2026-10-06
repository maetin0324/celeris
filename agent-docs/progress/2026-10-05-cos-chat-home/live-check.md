---
title: CoS チャットホーム 実機確認（live-check）
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-06
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
