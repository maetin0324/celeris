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

LLM を伴う (a)〜(e) は、人の決定 `live2-llm-run` のとおり**運用セッション（Claude Opus 5.5）が台本を `full` で流した**
（worker は別の LLM の CLI を起動しないため。ADR 2026-10-07-worker-no-subagents-no-llm-cli）。この節の結果は、その証跡
（WU 成果物 `artifacts/live2/full/evidence/`）を worker が読んでまとめたもの。台本は `live2-ops.sh`
（`live2.sh` からの修正: run id を POST の応答でなく `GET …/messages` で待つ、provider の concurrency を 4 にする）。
先の 2 回の試行（`full-attempt1-runid-null/`・`full-attempt2-capacity1/`）は台本の不備で止まったもので、結果には使っていない。

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

### (a)〜(e) の結果（full、2026-10-07 07:09〜07:17）

- 試験用 daemon: pid 1240335、`http://127.0.0.1:17932/api/v1`、data dir `artifacts/live2/full/`（ADR-0126）、harness claude-code・llm_source claude_oauth
- code: tree `5664ef5fb20e`（daemon の表示は記録 commit `deb89817f845`。差分は live-check.md だけ）
- thread `01M4AK3YNTX2WXSMKD8GKT6NBJ`、node_sessions row `01M4AK410K4TEHZ5P426HV65TF`（session `01a11532-0413-…`、turns 5）
- 添付: 青い PNG `01M4AK3YPXCXAHA57ECYXFNC3C`、screenshot `01M4AK3Z8VXY7MWW94ANPRXX8K`、PDF `01M4AK3ZT5T3AX6D7EBH07YC5F`（3 件とも ready）

| turn | run id | state | session_mode |
|---|---|---|---|
| t1 | `01M4AK40YYEC2EGYF9J944V1QC` | completed | new |
| t2 | `01M4AK4CV5R7P3GRZ5XHQTDQG6` | completed | resumed |
| t3 | `01M4AK4RMY5FQGZHD7JXWF9K0F` | completed | resumed |
| t4 | `01M4AK59D7WC3M2KAXF9JCA30D` | completed | resumed |
| t5 | `01M4AK7B9DD0PSHHQQCJHAJ8HE` | completed | resumed |

| 確認 | 判定 | 証跡 |
|---|---|---|
| (a) 2 往復以上で completed、new→resumed、同じ node_sessions row | **PASS** | 5 run とも completed で `reason` は空（result.json 不在の failed は出ない＝不具合 1 の修正を実機で確認）。5 run の `session_row_id` はすべて `01M4AK410K4T…`。t2 の返事「合言葉はみかん42」で、前の turn の内容を継いでいる |
| (b) 画像添付を CoS が読む | **PASS** | t2 の返事「添付画像は青色の正方形です」 |
| (c) `--api-url` なしの `celerisctl add` が試験用 daemon に届き、events に `cos_operation` actor=cos | **PASS** | 試験用 DB に task `01M4AK4ZP77PVHYHSBJGBJFV0H`（done）、`cos_operations` の `01M4AK4ZP6TQGE4XV5FG1K8Q9N` は task.create applied、events seq 1 は `cos_operation` actor=cos（thread_id・run_id・reason つき）。試験用 DB に行があるので daemon 自身の API（`CELERIS_API_URL`）に届いた。本番 API には届いていない |
| (d) screenshot を pin した UI 修正 task の添付 pin と入力 manifest | **FAIL** | task `01M4AK6NZJ8KGTT9GHZJ6YX9FH` を起票（4 回 422 の後で applied）。pin `01M4AK6TET2C…`（attachment.reference applied、`chat_attachment_refs` に owner_kind=task）はある。しかし作業 run `01M4AK6PMS0GMW6ZX95TTSF74M` は pin の約 4 秒前に始まっていて、`prompt.txt` に「## 入力の添付」が無く、stage も空（`staged.txt` 0 byte）。worker は対象を尋ね、task は blocked。原因は下の D1 |
| (e) PDF の KB inbox candidate と provenance | **FAIL** | 候補は作られなかった（`kb-inbox.json` の items は空）。CoS は PDF を読んで codeword「SAKURA-77」を正しく答えた。しかし `celerisctl knowledge record` は CoS credential で拒否され、`POST /api/v1/knowledge/inbox` は `cos_operation_not_allowed`（422。operation `01M4AK884RQ0…` rejected）。CoS は監査を通らない直接書き込みを選ばず、人に選択肢を返した。原因は下の D2 |
| codex の 1 往復 | 未実施 | 運用セッションの full には含めていない |
| 停止と残り | OK | `pgrep -f <data dir>` は該当なし（`pgrep-after-stop.txt` は空） |

本番 daemon・本番 DB・`~/.config/celeris` は読み書きしていない。外部への通信は claude（claude_oauth）の LLM 呼び出しだけ。

### 実機で見つかった点（運用セッションの判定 D1〜D4。2026-10-07 に直した。実機の再実行は未）

- **D1**（(d) の FAIL）: skill §3a の「起票してから pin」は dispatcher と競合する。CoS が起票した task はすぐ ready になる（`crates/task-api` handlers/tasks.rs の作成経路）。そのため、pin より先に worker が始まり、入力の添付が渡らない。直し方の候補は 3 つ: draft で作って pin してから ready にする、作成時に attachment id を受ける、run の開始時に pin を stage し直す。 **→ 直した（task 01M4APB5FP8T3TAAE51Z20E3M1、ADR 2026-10-07-cos-live-fixes D1。進捗 agent-docs/progress/2026-10-07-cos-live-fixes.md）**
- **D2**（(e) の FAIL）: CoS には、監査つきで KB 候補を作る経路が無い（`celerisctl knowledge record` は CoS credential を拒否し、`/api/v1/knowledge/inbox` は CoS operations の許可表に無い）。scope の書式も skill（`projects/agent-platform`）と CLI（`project:agent-platform`）で揃っていない。 **→ 直した（task 01M4APB5FP8T3TAAE51Z20E3M1、ADR 2026-10-07-cos-live-fixes D2。進捗 agent-docs/progress/2026-10-07-cos-live-fixes.md）**
- **D3**: cos-inbox-triage SKILL §5 は confidence を求めるが、`ResolveBody`（deny_unknown_fields）に欄が無い。実機の triage は confidence を reason の中に書いて回避した（`01M4AK5WCD…` 0.97、`01M4AK8PKP…` 0.3）。 **→ 直した（task 01M4APB5FP8T3TAAE51Z20E3M1、ADR 2026-10-07-cos-live-fixes D3。進捗 agent-docs/progress/2026-10-07-cos-live-fixes.md）**
- **D4**: triage thread `01M4AK5ADAQXY2GJDSFSYGT8ZW` で、終わった run（`01M4AK5ADFP337DY6C6QFR3EA3`・`01M4AK86TZA1MNEAWZMENJ7ACE`）が `orphan takeover; continuing in a new run` で interrupted になり、続きの run（`01M4AK68WJ…`・`01M4AK959T…`）が重複して起きた（cos_chat/control.rs の takeover 判定）。 **→ 直した（task 01M4APB5FP8T3TAAE51Z20E3M1、ADR 2026-10-07-cos-live-fixes D4。進捗 agent-docs/progress/2026-10-07-cos-live-fixes.md）**
- (d) の起票で 422 が 4 回出た（`acceptance` 欠落、`description`/`path` の欄名違い、成果物の置き場所）。CoS は自分で直したが、skill に POST /tasks の最小例を置くと往復が減る。

これらを直す葉は、計画で既存の段に入れる（運用セッションの指示）。修正後に運用セッションが live2 を再実行する。

### 不具合 1〜5 の扱い

| # | 内容 | 扱い |
|---|---|---|
| 1 | claude-code の CoS chat run が result.json 不在で failed | **直した**（result-fix。偽ハーネスの `cos_chat_harness_e2e_claude_without_result_json_completes` ほか）。実機の (a) で 5 run とも completed を確認した |
| 2 | CoS run の celerisctl が既定で本番の設定を読む | **直した**（api-env。run の env に `CELERIS_API_URL` と daemon の dir を先頭にした `PATH`。`cos_chat_run_launch_sets_api_url_env_and_path`）。実機の (c) で、`--api-url` なしの起票が試験用 daemon に届いたことを確認した |
| 3 | KB に cos skill が無いと CoS が動かない | **文書化した**（ops-docs2 が `docs/ops/cos-chat.md` に、KB の `skills/` に置く手順を書いた）。挙動は変えない（skill が無ければ理由つきで unavailable） |
| 4 | 短い往復で `summary_through_seq` が 0 のまま | **未解決**。実機でも 5 turn 後に 0 のまま（`db.txt` の node_sessions）。resume が効く間は害が無い。rollover 時は、未要約の範囲を明示して履歴で補う設計。短い往復で要約を省くのを許すかは人が決める |
| 5 | `chat_runs` の実効 model が null | **未解決（設定の問題として文書化）**。実機でも `model: null`（provider に model を書いていない）。運用では `[cos] model` か provider の model を書く |

### 新たに見つけた点（コードを読んで。未修正）

6. CoS run の `celerisctl knowledge record` は API を通らない。KB の根は `CELERIS_KNOWLEDGE_ROOT` → `CELERIS_CONFIG` の `[knowledge] root` → 既定、の順で決まる（`crates/celerisctl/src/commands/knowledge.rs` の `configured_root`）。
   実機では CoS credential で拒否されたので書き込みは起きなかった（D2）。D2 を CLI 直書きで直すなら、同じ形の「本番 KB に書く」穴が開く。API の operation にするのが安全。

### 再実行の手順（D1〜D4 の修正後）

本番でない shell（claude_oauth ログイン済み）で、修正後の tree の worktree から `cargo build -p celeris -p celerisctl` の後、
`bash <WU 成果物>/live2/live2-ops.sh "$PWD" "$CARGO_TARGET_DIR/debug" <新しい data dir> full`。
`evidence/steps.log`・`db.txt`（`chat_runs`・`node_sessions`・`cos_operations`・`chat_attachment_refs`）・`kb-inbox.json`・`prompt-files.txt`・`staged.txt` を上の表と同じ観点で見る。

## 実機確認（D1〜D4 修正後）

2026-10-07。WorkUnit live3-record（run `01M4AW6EEE5C2J0B4MZ17RSVHQ`）。人の決定 `live3-llm-run` のとおり、**運用セッション（Claude Opus 5.5）**が
live-fixes の tip **`767a15760901872f8c753ce91b5c5e4afcf0e1da`** で `scripts/dev/cos-chat-live.sh <repo> <bin dir> <data dir> full` を本番でない shell（claude_oauth）から流した。
この節は、その証跡を worker が読んでまとめたもの（worker は LLM を伴う実機確認を流していない）。

- 合格の証跡: `/var/tmp/cos-live3-20261007-094511-full-fixture/evidence/`（`steps.log`・`db.txt`・`run-t*.json`・`kb-inbox.json`・`prompt-files.txt`・`staged.txt`・`input-manifest.txt`・`verdict.txt`・`daemon.log`）
- この run は運用セッションが台本の手元の写しで 2 点を補ったもの（daemon 起動前の `celerisctl knowledge init --root $OUT/kb`、試験用 DB への project `agent-platform` の fixture）。台本のままの 2 回は (e) が FAIL（下の「台本のままの記録」）
- 試験用 daemon: pid 131807、`http://127.0.0.1:17932/api/v1`、`steps.log` の表示 commit `767a15760901`、mode full、harness claude-code・llm_source claude_oauth・provider `claude-live`・tier frontier
- fixture project: `01M4AW00000000000000000PRJ`（agent-platform、active）
- thread `01M4AW1CKJ8367V4CWVF1R0498`、node_sessions row `01M4AW1D1AFR126X2SYD15CEXF`（session `01a115c0-b42a-…`、turns 5）
- 添付: 青い PNG `01M4AW1CMA9WH9A78A0REVEDZR`、screenshot `01M4AW1CNRDN1T6AC3E2YDPZSZ`、PDF `01M4AW1CQB1E38F18S5XDANK1F`（3 件とも ready、sha256 つき）

| turn | run id | state | session_mode |
|---|---|---|---|
| t1 | `01M4AW1D180F35Q8Y3WAMRWJ9C` | completed | new |
| t2 | `01M4AW1RTBXX033WDDCM5S86GK` | completed | resumed |
| t3 | `01M4AW24MJR8CTETY8F7V8Y41Y` | completed | resumed |
| t4 | `01M4AW2NDX7V4RJPWP6ERTR5MJ` | completed | resumed |
| t5 | `01M4AW3C3ABP6F3AXD11174D42` | completed | resumed |

### (a)〜(e) の結果（full、2026-10-07 09:45〜09:46）

| 確認 | 判定 | 証跡 |
|---|---|---|
| (a) 2 往復以上で completed、new→resumed、同じ node_sessions row | **PASS** | 5 run とも completed・`reason` 空。`chat_runs` の `session_row_id` はすべて `01M4AW1D1AFR…`。t2 の返事「合言葉は「みかん42」」で前の turn を継いでいる |
| (b) 画像添付を CoS が読む | **PASS** | t2 の返事「添付画像は青色の正方形です」 |
| (c) `--api-url` なしの `celerisctl add` が試験用 daemon に届き、events に `cos_operation` actor=cos | **PASS** | 試験用 DB に task `01M4AW2ADJZEY9D29RCGV7TBF3`（done）、`cos_operations` `01M4AW2ADH048G1ACFW36N4XGF` task.create applied、events seq 1 が `cos_operation` actor=cos（thread_id・run_id・reason つき） |
| (d) screenshot を起票した UI 修正 task に引き継ぐ | **PASS** | operation `01M4AW2YDZZF9QMD17KWA77R1C`（task.create applied、result の `attachment_ids` に screenshot）。task `01M4AW2YDZV4XSBVV5514GKEKQ` に owner_kind=task の pin（09:46:03.58）。最初の作業 run `01M4AW2Z97YDYQ9T7VS47BN5JM` の `prompt.txt` に「## 入力の添付」（screen.png、delivery `image`）があり、`staged.txt` に読み取り専用（`-r--------`）の screen.png。起票と pin は 1 回の operation で、別の attachment.reference は 0 件 |
| (e) PDF を KB inbox candidate に provenance 付きで取り込む | **PASS** | operation `01M4AW3PCHRBQ5JBESEDQHN6E4`（knowledge.record applied、scope `project:agent-platform`）。候補 `20261007T094628Z-cos-chat-live-codeword-cos` が `_inbox/` にあり、provenance に facts.pdf の sha256 `8f1555f9…`・thread・message `01M4AW3B705Q…`・依頼本文。owner_kind=knowledge_inbox の pin と events の `cos_operation`（actor=cos、target_kind knowledge）がある。候補本文に codeword「SAKURA-77」 |
| 停止と残り | 注意 | `steps.log` は `LEFTOVER processes:` を出し、`pgrep-after-stop.txt` に pid 2 件。運用セッションの判定では台本の `pgrep -f "$OUT"` が自分の shell に当たる誤報告（台本の不備 (3)） |

本番 daemon・本番 DB・`~/.config/celeris` は読み書きしていない（data dir は `/var/tmp/cos-live3-20261007-094511-full-fixture/`）。外部への通信は claude（claude_oauth）の LLM 呼び出しだけ。

### D1〜D4 の実機での解消

| # | 内容 | 実機 | 証跡 |
|---|---|---|---|
| D1 | 起票と pin の競合（pin より先に worker が始まる） | **解消** | 作成時の `attachment_ids` で同じ operation の中で pin された。最初の run の manifest に screen.png があり、stage にも置かれた（(d)） |
| D2 | CoS の監査つき KB 候補作成の経路が無い・scope 書式の不一致 | **解消** | `knowledge.record` が `/cos/operations` 経由で applied、scope `project:agent-platform` で候補と pin と監査 event がそろった（(e)） |
| D3 | triage の confidence と `ResolveBody` の不一致 | **解消** | `cos_operations` `01M4AW3955TENS4AGHE1AYGEMS`（inbox.observe applied）の result に `"confidence":0.97` |
| D4 | 終わった triage run の orphan takeover による重複 run | **解消** | triage thread `01M4AW2PDYCDX2FN5R6SCWAC24` の `chat_runs` は `01M4AW2PE2HCH53F6T7TN6ZRN2`（completed）の 1 件だけ、run dir も 1 つ。3 回の full の `daemon.log` に takeover/orphan は 0 件（運用セッションの note も「3 回とも再発しなかった」） |

### 台本のままの記録と残り

- `/var/tmp/cos-live3-20261007-093912-full`: (d) PASS、(e) FAIL（KB 未初期化）。`/var/tmp/cos-live3-20261007-094248-full-kbinit`: (d) PASS、(e) FAIL（project が無い）。dry `/var/tmp/cos-live3-20261007-093912-dry` は exit 0
- 台本 `scripts/dev/cos-chat-live.sh` の直すこと（close-out で直す。人の決定の指示）: (1) daemon 起動前に `celerisctl knowledge init --root $OUT/kb`、(2) t5 用の project `agent-platform` を会話を起こさない作り方で試験用 DB に作る、(3) `pgrep -f "$OUT"` の誤った LEFTOVER、(4) verdict が FAIL でも exit 0
- 軽微: 拒否された CoS operation の `cos_operations.action` が `rejected` になり、要求した action が残らない
- LLM の使用は 3 回の full で約 26 session

## 提案（live-check2）

- D1: 作成時に attachment id を受けて同じ transaction で pin する（起票と pin の間に dispatcher が入らない）。
- D2: KB 候補の作成を CoS operation（API、監査つき）として登録し、`celerisctl knowledge record` は CoS credential のときその API を使う。scope の書式を 1 つにする。
- D3: `ResolveBody` に任意の `confidence` を足すか、skill から外す。
- D4: 終わった run を takeover の対象から外す（終端の記録と lease の解放の順序を確かめる）。
- 4: 「run 終了時の checkpoint」を CoS の前置きで必須にするか、N 往復ごとでよいとするかを決める。
