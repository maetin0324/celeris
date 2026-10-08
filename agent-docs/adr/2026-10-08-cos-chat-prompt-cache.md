# ADR 2026-10-08: CoS chat の prompt cache 最適化 — 現状の事実・仮説表・計測設計（草案）

---
tasks: [01M4D3XKDS8DK9QEK081QKA8VE]
---

- 日付: 2026-10-08
- 状態: **草案（提案）**。この ADR はコードを変えない。D1 は最新 main（`9d2a73141f0e`。skill 配送の v3 content hash 照合を含む）の読解と、LLM を呼ばない決定的な計測で確かめた事実。D2〜D5 は後続 task の提案で、人の採否を待つ
- 関連: [ADR 2026-10-05 cos-chat-home](2026-10-05-cos-chat-home.md)（D2 の session・要約・差分配送）、[ADR 2026-10-06 cos-chat-harness-adapters](2026-10-06-cos-chat-harness-adapters.md)、[ADR 2026-10-07 cos-live-fixes](2026-10-07-cos-live-fixes.md)（`scripts/dev/cos-chat-live.sh`）、[ADR-0054](0054-stateful-sessions-and-streaming-chat.md)、[ADR-0140](0140-claude-session-resume.md)、ADR-0056 D3・[ADR-0127](0127-skills-native-delivery.md)（skill の配送）、ADR-0072（実行 metrics・単価表）
- 範囲: CoS chat run だけ。worker 一般（task run・planner・reviewer）の既存挙動は変えない。計測は隔離環境だけで行い、本番の config・DB・KB・release・systemd に触れない。LLM 呼び出しは subscription の lab account（`claude_oauth`）だけ。API 課金 source の利用・promote・権限変更は人の決定

## 1. 文脈

CoS chat は人との常設の会話の場（ADR 2026-10-05 cos-chat-home）である。1 turn ごとに worker が 1 回起動し、thread ごとの session を resume する。返答の遅さと利用量（subscription の枠、API なら名目 cost）の主因は、provider の prompt cache に乗らない入力だという見立てがある。ただし、何が cache に乗っていて何が乗っていないかは、chat run の usage を DB に残していないので誰も測っていない。この ADR では、最新 main の事実を確定し、仮説と判定基準、計測の設計、後続 task の受け入れ条件案を書く。

## 2. D1: 最新 main の事実

### D1.1 起動の流れ（dispatcher 側）

- `crates/task-dispatch/src/dispatcher/cos_chat/launch.rs` の `launch_claimed` が 1 turn を組み立てる。手順は次のとおり。
  1. `rollover::choose_session` で resume か retire→fresh かを決める（照合 key・cache の有無・前回 run の終わり方・`approx_tokens >= [sessions] rollover_tokens`。既定は 400,000、`crates/celeris/src/config/dispatch.rs`）。session_mode は `chat_runs.resolved_config_json` に入る
  2. `rollover::history_since_summary` で thread の summary と、未要約の履歴（`summary_through_seq+1 ..= input.seq-1`）を作る。**session_mode が resumed でも fresh でも同じように呼ぶ**
  3. `CosChatContext`（thread_id・run_id・inputs・summary・unsummarized・attachments・skills 名・credential env 名・api_base_url・inbox_items）を作る
  4. `chat.transient_task(...)` で一時 Task を作る（`TaskId::new()`。**run ごとに新しい id**）
  5. `skills_context(["cos-operator","cos-inbox-triage"])` で KB の `skills/<name>/` を mount する（無ければ起動しない）
  6. `RunContext { cos_chat, skills, session: SessionHandle{resume}, ..default }` を作る。その他の preamble 入力（profile・organization・knowledge index・memory・comments 等）は**空**
- artifacts は `<thread workspace>/.taskd/chat-runs/<run_id>/`。workspace は `<data_dir>/cos/threads/<thread_id>/`（thread ごとに固定）

### D1.2 prompt の組み立て（worker 側）と system/stdin の内訳

本文は `crates/task-worker/src/cos_chat.rs::build_prompt` が作る（`claude_code::build_prompt` が `context.cos_chat` を見て振り分ける。codex・acp・pi も同じ関数を使う）。並び順は次のとおり。

| # | 節 | 中身 | run ごとに変わる値 | 実測 bytes（新規 thread の 1 turn 目） |
|---|---|---|---|---|
| 1 | 見出し | `# CoS chat: thread <thread_id>` と `(worker run <run_id>, chat run <chat run_id>, task <一時 task id>)` | **thread_id（2 行目）、worker run id・chat run id・一時 task id（3 行目）** | 158 |
| 2 | preamble（`preamble::render`。入力が空なので固定の注意書き 4 節だけ） | 成果物の置き場所・本番 host の操作・道具の起動の制約・一時 file | 無し | 1,995 |
| 3 | 人からの入力 | `### seq <n> (message <id>)` と本文、添付 id | seq・message id・本文 | 124〜 |
| 4 | これまでの要約 | `chat_threads.summary` の全文（最大 32 KiB） | summary・`summary_through_seq` | 80〜32 KiB 強 |
| 5 | 要約未作成の範囲 | 未要約の発言を全文で 1 行ずつ。渡していない範囲の明示 | seq 範囲・各発言 | 数百 B〜（上限なし） |
| 6 | 受信箱の未解決の件（受信箱 thread だけ） | 件ごとの状態・選択肢と relay の手順 | 件の一覧 | 0 か 数 KB |
| 7 | 添付（あるときだけ） | manifest・pin の手順 | 添付 id・path・sha256 | 0 か 数 KB |
| 8 | 使う skill | skill 名 2 つ（本文は入れない） | 無し | 128 |
| 9 | 返事と操作・要約の保存・履歴の読み方 | 規則と curl の雛形 | **API base・thread_id・chat run id・`through_seq`・`expected_summary_through_seq`** | 1,108 + 653 + 357 |

決定的な計測（D1.6）では、新規 thread の 1 turn 目が 4,927 B だった。そのうち固定文（#2・#8・#9 の地の文）は約 4.2 KB（約 86%）を占める。それでも prompt 同士の先頭一致は、**別 thread の 1 turn 目どうしで 29 B、同じ thread の 1 turn 目と 2 turn 目で 67 B** しかない。#1 の 1〜3 行目に thread id と run id があるため、固定文の手前で prefix が切れる。#9 にも thread_id・run_id・seq が埋め込まれている。

harness 別の渡し方:

| harness | system 側 | stdin / 入力 message 側 | skill の届け方 | session 継続 |
|---|---|---|---|---|
| claude-code（`claude_code.rs`） | Claude Code 自身の system prompt と道具定義、`--append-system-prompt` の `HEADLESS_RUN_NOTE`（固定）。Claude Code は `.claude/skills/*/SKILL.md` の frontmatter（name・description）を自動で一覧に入れる。cwd・日付などの環境情報も Claude Code が自分で入れる（入る位置と内容は Claude Code の版に依存し、**不明**。D3 で実測） | `-p` の stdin に上の本文の全文（画像が native のときは stream-json の user message） | `skills::deliver_claude_code`。`<cwd>/.claude/skills/<name>/` へ写す。内容 hash（marker v3）と届け先の実内容が一致すれば書き直さない（9d2a7314） | 初回は `--session-id <uuid>`、2 回目以降は `--resume <uuid>`。session が無い run は `--no-session-persistence` |
| codex（`codex.rs`） | codex 自身の instructions と、作業場所の `AGENTS.md` の celeris 節（skill の一覧。`skills::deliver_agents_md`） | stdin に `work_dir_note`＋本文 | `.agents/skills/<name>/` に写す（同一内容なら省略。9d2a7314）＋AGENTS.md の一覧 | `codex exec resume <id>`（`resume_mode = exec_resume` のとき）。保証できない設定では明示的に fresh になり、status に理由を出す |
| acp（opencode 等。`acp.rs`） | agent 側の system prompt | `session/prompt` に本文＋`skills::preamble_section`（一覧 1,310 B。**本文の末尾**に付く） | `.agents/skills/` に写す（同一内容なら省略。9d2a7314） | `loadSession` 能力があれば `session/load`、拒否されたら fresh |
| pi（`pi.rs`） | pi 側 | stdin に本文＋`skills::preamble_section` | `.agents/skills/` に写す（同一内容なら省略。9d2a7314） | `--session-dir <run_dir>/pi-sessions`（run ごと）。thread をまたいだ継続は無い |

### D1.3 thread session の resume 規則と再送の中身

- 照合 key は `(thread_id, harness, provider, llm_source, account_id, cwd, model)`。一致し、cache があり、前回 run が resume 拒否・context 枯渇で終わっておらず、`approx_tokens < rollover_tokens` なら resume する。それ以外は retire して fresh。resume を拒否された run は、同じ run の中で fresh として 1 回だけ再試行する（`rollover::run_attempts`）
- **resume の run でも、dispatcher は summary と未要約履歴の全文を毎回 stdin に入れる**。worker は固定の規則（#2・#8・#9）も毎回入れる。Claude Code の `--resume` は保存した transcript（前回までの system・user・assistant・tool_use/tool_result）をそのまま会話の先頭として送り、今回の stdin を新しい user message として後ろに足す。このため resume の turn ごとに、規則の約 4 KB と、その時点の summary・未要約履歴が会話に**重ねて**積まれる。summary が checkpoint で更新されれば、同じ内容の別版も積まれる
- これは cos-chat-home D2 の「再開中は未配送 seq 以後の差分と現在の規則だけを渡す」と食い違う（H3）。fresh のときに summary＋未要約履歴を渡すのは D2 どおり
- transcript 部分は前回と同じ prefix なので、cache の TTL 内なら cache read になる見込み。ただし chat run の usage を残していないので**実測は無い**（D1.5）

### D1.4 provider の prompt cache 規則（計測で確かめる前提）

| provider / 経路 | 規則（一次情報は計測 task で再確認する） |
|---|---|
| Anthropic（Claude Code が API に送る request） | 完全な prefix 一致（tools → system → messages の順）。breakpoint は最大 4 つで、Claude Code が自分で置く（celeris からは制御できない）。最小 cache 長はモデルごとに約 1,024〜4,096 token。TTL は既定 5 分（利用のたびに延長）、1 時間の選択肢あり。課金倍率は書き込み 1.25×（1h は 2×）、読み取り 0.1×。stream-json の `usage` に `cache_creation_input_tokens`・`cache_read_input_tokens` があり、`input_tokens` は**非 cache 分だけ** |
| Claude Code の subscription（`claude_oauth`） | 課金は API の従量でなく plan の利用枠。CLI が出す `total_cost_usd` は名目値。cache が枠の消費にどう効くか（read を軽く数えるか）は**不明**。TTL が 5 分か 1 時間かも**不明**（usage の `cache_creation.ephemeral_5m_input_tokens` / `ephemeral_1h_input_tokens` が出れば分かる。D3 で記録する） |
| OpenAI（codex） | 自動 cache。1,024 token 以上の prefix が対象で、`cached_input_tokens` で報告される（`codex.rs` は `cache_read_tokens` に写す）。cache 書き込みの量と TTL は報告されない（**不明**） |
| acp / pi | agent と provider 次第。pi は `cacheRead` を足し込む（`pi.rs`）。acp の usage は provider 依存 |

### D1.5 chat run の usage は DB に残らない

- adapter は `Terminal::*{usage}` に `input_tokens`・`output_tokens`・`cache_read_tokens`・`cache_creation_tokens`・`cost_usd`・`session_resumed` を載せる（`task_core::Usage`）。task run なら `execution_metrics` に残る
- CoS chat は `ChatRunSink` も `chat_runs` も usage を保存しない（列が無い。`0050_cos_chat.sql`）。残るのは `rollover::usage_tokens` が `node_sessions.approx_tokens` に足す `input_tokens + output_tokens` だけ
- Claude の `input_tokens` は非 cache 分なので、**`approx_tokens` には cache read も cache write も入らない**。resume のたびに transcript 全体が context を占めるのに、`approx_tokens` はその増え方を数えない（H4）。`rollover_tokens = 400,000` は、実際の context 占有より遅れて、あるいは永遠に効かない見込みがある
- 初回応答 latency（最初の text_delta）と総 latency は `chat_events` の時刻から後で算出できる。ただし、そのための専用の欄は無い

### D1.6 skill の大きさ（依頼文の数値の是正）

| skill | repo（`config/skills/<name>/SKILL.md`、`9d2a73141f0e`） | 本番 KB の写し（`celerisctl knowledge get skills/<name>/SKILL.md`、2026-10-08 読取り） |
|---|---|---|
| cos-operator | 20,158 B（`SOURCE.md` 1,226 B は別） | 17,801 B（repo より古い。frontmatter は同じ） |
| cos-inbox-triage | 11,386 B（`SOURCE.md` 964 B は別） | 8,633 B（repo より古い） |

依頼文の数値は最新 main と一致した（是正は不要だった）。本番 KB の写しが古いのは、KB の skill が repo から自動で同期されないためである（本 ADR の範囲外。人の同期手順）。両 skill の description は「CoS チャット・受信箱スレッドの run では常に読む」「resolve を呼ぶ前には必ず読む」と書いているので、ほぼ毎 run、model が SKILL.md の全文（合計 31,544 B）を tool で読む見込みがある。回数は**未計測**（H2）。日本語が主なので、token 数は bytes/3 前後と見込むが、tokenizer 次第で**不明**。

### D1.7 cos-operator skill §3 の表と `/cos/operations` の登録の食い違い（是正対象）

`crates/task-api/src/cos/operations.rs` の `ALLOWED` に登録されているのは、POST tasks・comments・decisions answer・approvals decide・execution phase-gate / plan-gate・task answer、PATCH projects、knowledge inbox / reject、attachment references、inbox items answer だけである。skill §3 の表は、登録されていない次の操作を `/cos/operations` の `request` として載せている。CoS が試すと 422 `cos_operation_not_allowed` になる。

- `PUT /api/v1/tasks/<id>/execution-plan`（replan 行）
- `POST /api/v1/tasks/<id>/pause`・`…/resume`、`POST /api/v1/projects/<id>/pause`（pause / resume 行）
- `/api/v1/standing-rules`（承認・認可行）

是正は skill 再構成の task（D5 の T5）で行う。表から外すか「未登録（人の画面で行う）」と書く。登録を足すのは権限の変更にあたるので人の決定とする。

## 3. D2: 仮説表

判定の基準値は、D3 の baseline（T2）で測った値に対する比で書く。p は台本 1 回分（D3.3）の中央値。

| id | 仮説 | 根拠（D1） | 検証方法 | 判定基準（採択） |
|---|---|---|---|---|
| H1 | 固定 Core（#2・#8・#9 の地の文と skill 一覧）を run ごとに変わる値（#1 の run id・一時 task id、#9 の thread/run id・seq）より前に置くと、thread をまたいだ prompt と同じ thread の turn 間で、prefix が固定部全体まで一致する | 先頭一致が 29 B / 67 B しかない（D1.2） | (a) LLM 無しベンチ: 同じ入力で run id・thread id・seq だけを変えた 2 つの prompt の先頭一致の bytes。(b) live: 独立新規 thread の 2 件目以降の 1 turn 目の cache read | (a) 先頭一致 ≥ 固定部の bytes（現状の約 4.2 KB）を、別 thread どうしと同じ thread の turn 間の両方で満たす。(b) 新規 thread の 1 turn 目の非 cache input が baseline 比 −10% 以上。cache read が増えない（Claude Code が cwd 等を system に先に入れていて切れる）場合は「効果なし（claude-code）」と記録し、codex 等で別途判定する |
| H2 | 毎 run の skill 全文の読込（31,544 B）が、非 cache input と初回応答 latency を押し上げている | description が「常に読む」と指示している（D1.6） | live: run ごとの SKILL.md への Read/Skill tool call の回数と、その tool_result の bytes。単純相談の台本で、skill 読込のある run と無い run の初回応答 latency・非 cache input を比べる | 単純相談の 80% 以上の run で両 skill を読んでおり、読込がある run の初回応答 latency の中央値が無い run より 20% 以上大きい、または非 cache input の 30% 以上が skill 本文にあたる |
| H3 | resume の turn でも summary＋未要約履歴＋固定規則を毎回再送しているため、同じ thread の turn が進むほど 1 turn あたりの入力 token が線形以上に増える（D2「差分だけ渡す」と不一致） | `history_since_summary` が mode を見ない（D1.3） | (a) LLM 無しベンチ: resumed の run の stdin bytes を turn 1〜10 で出す。(b) live: 同じ thread の 10 turn で、turn ごとの cache read＋cache write＋非 cache input の合計 | (a) resumed の stdin のうち、summary・未要約履歴・固定規則の重複が 50% 以上。(b) turn 10 の入力合計が turn 2 の 2 倍以上で、その増分の半分以上が重複した再送にあたる |
| H4 | rollover の `approx_tokens`（input+output）は cache read を含まないので、実際の context 占有と大きくずれる | D1.5 | live: 同じ thread の 10 turn で、`node_sessions.approx_tokens` の値と、各 turn の `input+cache_read+cache_creation`（＝その turn の context 長）を並べる | turn 10 の時点で context 長 / `approx_tokens` ≥ 3。そうなら `approx_tokens` を context 長（最新 turn の入力合計）にする案を T3 で採る |
| H5 | 内容が同じ skill の書き直しの省略は **9d2a7314 で実装済み**。残る問いは、その効果（起動 latency、prompt への影響）が測れるほどあるか | `skills.rs` の `deliver_to` が marker v3 の元内容 hash・所有印・届け先の実内容 hash がすべて一致すると `replace_copy` を呼ばない（`agent-docs/progress/2026-10-08-cos-chat-prompt-cache/skill-delivery.md`） | 新しい実装はしない。T2 baseline と T8 after の LLM 無しベンチで 2 回目以降の配送時間（ms、10 回）と file の mtime/inode 不変を測る。実 chat の latency・token への効果は live ベンチまで **不明** | 2 回目以降の配送で書き込みが無く、prompt に入る一覧が bytes 単位で同じなら「実装どおり」として採択。配送時間が測定誤差内なら「効果は小さい」と記録 |
| H6 | cold（TTL 切れ・新規）と warm（TTL 内の続き）で、非 cache input と初回 latency が大きく違う。したがって cache の効果は、turn 間隔の分布に左右される | provider 規則（D1.4） | live: 同じ thread で、turn 間隔を 30 秒（warm）と 6 分以上（5 分 TTL 切れ。1 時間 TTL なら 61 分以上）で比べる。usage の `ephemeral_5m/1h` の内訳も記録する | warm の非 cache input が cold の 50% 以下で、初回応答 latency の中央値が 20% 以上短い。TTL の種類（5m / 1h）を確定し、記録する |

## 4. D3: 計測設計

### D3.1 指標（chat run ごと）

| 指標 | 出所 | 現状 |
|---|---|---|
| 非 cache input tokens | adapter の `Usage.input_tokens` | adapter は取得しているが保存していない |
| cache read tokens | `Usage.cache_read_tokens`（Claude `cache_read_input_tokens`、codex `cached_input_tokens`、pi `cacheRead`） | 同上 |
| cache write tokens | `Usage.cache_creation_tokens`（Claude だけ）。5m / 1h の内訳は stream-json の `cache_creation` | 同上。内訳は adapter が取っていない |
| output tokens | `Usage.output_tokens` | 同上 |
| model・harness・provider・llm_source・account | `chat_runs.resolved_config_json` | 保存済み |
| session_mode（new / resumed / fresh_after_refusal / retire の理由） | `chat_runs.resolved_config_json` | 保存済み |
| 初回応答 latency（run 開始 → 最初の text_delta）と総 latency | `chat_runs.started_at`、`chat_events` の時刻、`finished_at` | 後から算出はできる。専用の欄は無い |
| 成功率 | `chat_runs.state`（completed / failed / …）と `reason` | 保存済み |
| 名目 cost | `Usage.cost_usd`（単価表）または CLI の `total_cost_usd` | 保存していない |
| skill 読込の tool call 回数と bytes | `chat_events` の tool（name・summary）。path の照合は adapter の tool_use 観測（ADR-0140 D4 の `duplicate_reads` と同じ層） | tool 名は残るが、対象 path と bytes は集計していない |
| prompt bytes の固定部 / 可変部 | worker が `runs/<run_id>/prompt.txt` を書く。節ごとの bytes は LLM 無しベンチで分ける | prompt.txt はあるが、節別の集計は無い |

### D3.2 測れない指標（「不明」として扱う）

- subscription の利用枠の消費量と、cache read の枠への効き方（Anthropic が公開していない）
- Claude Code が system 側に入れる環境情報（cwd・日付・git 状態など）の正確な bytes と位置（CLI の内部。cache read の差から間接に推定するだけ）
- provider 側の cache の hit / miss の理由（usage の数値から推定するだけ）
- codex の cache 書き込み量と TTL
- server 側の待ち時間と model の生成時間の内訳（総 latency しか分からない）
- tokenizer の正確な token 数（provider の usage で代える。LLM 無しベンチは bytes で測る）

### D3.3 台本

| 台本 | 内容 | 回数 |
|---|---|---|
| S1 独立新規 thread | 新しい thread を作り、同じ単純相談（例「今の受信箱と走っている task の状況を 3 行で」）を 1 turn だけ送る | 10 thread |
| S2 同一 thread の連続 | 1 thread に、単純相談と実管理操作を交互に 10 turn 送る（前の返事が終わってから次を送る） | 1 thread × 10 turn（warm） |
| S3 単純相談 | 状況の質問だけ（道具は読み取りだけで済む） | S1・S2 の中で |
| S4 実管理操作 | 試験用 DB の task へのコメント、KB 候補の作成、受信箱の件への回答（`/cos/operations` を通る） | S2 の中で 5 turn |
| S5 cold / warm | S2 と同じ thread で、turn 間隔を 30 秒と 6 分以上（TTL が 1h と分かれば 61 分以上）で 3 turn ずつ | 各 3 |

### D3.4 2 種類のベンチ

1. **決定的な LLM 無しベンチ**（試験と同じ扱い。外部通信なし）: `task_worker::claude_code::build_prompt` を固定の fixture（thread・run id・seq・summary・履歴の長さ）で呼び、prompt の総 bytes、節ごとの bytes（固定 / 可変）、2 つの prompt の先頭一致の bytes を出す。S1 相当（別 thread の 1 turn 目）と S2 相当（同じ thread の turn 1〜10、resumed）を作る。本 ADR の D1.2 の数値は、$TMPDIR に置いた scratch crate（repo の外。task-worker を path で参照）で出した。T2 でこれを repo の試験（または `scripts/dev/` の台本）にする
2. **隔離 daemon の live ベンチ**: `scripts/dev/cos-chat-live.sh` と同じ方式。試験用 data dir（config・一時 SQLite・KB・workspaces・log がすべてその下）、別 port（`COS_CHAT_LIVE_PORT`）、`[cos] harness = "claude-code"`、`llm_source = "claude_oauth"`（lab account の既存ログインだけ）。repo の `config/skills` を data dir の KB に写す。台本 S1〜S5 を API で流し、終わったら process group を止め、data dir を指す process が残っていないことを pgrep で確かめる。本番の `~/.config/celeris`・`~/.local/celeris`・`/local/celeris/state` は台本が拒否する。結果は data dir の evidence と、chat run ごとの usage の JSON（T1 の欄が入った後）に残す

## 5. D4: 後続 task の受け入れ条件案

共通: 対象は CoS chat だけで、worker 一般の prompt と挙動は bytes 単位で変えない（既存の `claude_code` 等の試験が無変更で通る）。各 task の完了時に `bash scripts/dev/test-parallel.sh` と `cargo clippy --workspace -- -D warnings` が exit 0。live の計測は D3.4 の隔離環境だけで行う。

| # | task | 受け入れ条件案 |
|---|---|---|
| T1 | telemetry: chat run の usage を残す | (1) chat run の終端で `input/output/cache_read/cache_creation tokens`・`cost_usd`・`session_resumed` と、Claude の 5m/1h の内訳を DB に残す（新 migration。`SCHEMA_VERSION` を上げるときは task-core の版数試験も直す）。(2) 初回応答 latency と総 latency が API から読める。(3) resume 拒否で再試行した run は、試行ごとに区別して残す。(4) 偽 harness の決定的な試験で、各欄が adapter の usage と一致する。(5) 値が無い harness は null（0 にしない） |
| T2 | baseline: 現状の計測 | (1) D3.4-1 の LLM 無しベンチを repo に入れ、S1/S2 相当の bytes・節別・先頭一致を出す（外部通信なし。試験として回る）。(2) T1 の後に隔離 live ベンチで S1〜S5 を流し、D3.1 の指標を表にして progress に残す。測れない指標は「不明」と書く。(3) H1〜H6 の判定に要る値がそろっている |
| T3 | rollover の会計を直す | (1) rollover の判定に、最新 turn の context 長（`input + cache_read + cache_creation`）を使う（または併用する）。(2) 既存の `[sessions] rollover_tokens` の意味の変更を ADR に書く。(3) 決定的な試験: cache read が大きく input が小さい usage でも、閾値を超えれば次の run が fresh になる。(4) CoS 以外の node_sessions の会計は変えない |
| T4 | Core 分離（H1 採択時） | (1) CoS chat の prompt を「固定 Core → thread 固定 → turn 可変」の順に組み直し、run id・一時 task id・seq は可変部へ移す。(2) LLM 無しベンチで、別 thread どうしの先頭一致 ≥ 固定 Core の bytes。(3) 既存の cos_chat の試験（割り込みが先頭・actions 禁止・未要約の範囲の明示）を意味を保って直し、通す。(4) 隔離 live の S1 で、非 cache input が baseline 比で H1 の基準を満たす（満たさなければ結果を記録して止める） |
| T5 | skill 再構成（H2 採択時）と §3 の是正 | (1) cos-operator §3 の表から、`ALLOWED` に無い操作（PUT execution-plan・pause/resume・standing-rules）を外すか「未登録」と明記する。表の各行が `ALLOWED` に存在することを決定的な検査（試験か台本）で確かめる。(2) 毎 run 必要な最小部分を Core（T4）に移し、残りは参照時だけ読む節に分ける。description から「常に読む」を外す。(3) 隔離 live の S3 で、skill 読込の tool call が H2 の基準を下回る。S4 の成功率は baseline 以上 |
| T6 | 差分配送（H3 採択時） | (1) resumed の run には、前回の配送以後の差分（新しい入力と、前回以降に変わった summary だけ）と、変わった規則だけを渡す。fresh・fresh_after_refusal には今までどおり summary＋未要約履歴を渡す。(2) 前回に何を渡したかは DB から決める（worker やメモリに持たない）。(3) 決定的な試験: resumed の stdin に、前回 turn と同じ summary・履歴・固定規則が入らない。resume 拒否の後の fresh 再試行には全文が入る。(4) cos-chat-home D2 の付記に実装を書く |
| T7 | （削除）skill 配送の同一内容省略は 9d2a7314 で実装済み | 後続 task は作らない。効果の測定は T2（baseline）と T8（after）に含める。worker 一般への適用範囲は既に `deliver_to` が共通経路（Claude Code・Codex/ACP）なので、追加の判断は無い |
| T8 | after 比較 | (1) T3〜T6 のうち入ったものの後に、T2 と同じ隔離 live ベンチ（同じ台本・同じ model・同じ lab account）で S1〜S5 を流す。(2) 指標ごとに baseline との差の表を作り、仮説ごとに採択 / 棄却 / 不明を書く。(3) 成功率が baseline を下回る変更があれば、その変更の戻しを提案する。(4) 本番への反映（promote）は人の決定として手順だけを書く |

## 6. D5: やらないこと・人の決定

- API 課金 source での計測、本番 KB の skill の同期、`/cos/operations` への登録の追加、promote と本番への反映は人の決定。この ADR と T1〜T8 の葉は手順と材料だけを書く
- dispatcher に LLM 呼び出しは入れない（要約は今までどおり worker の checkpoint）
- cache の breakpoint を celeris から直接置くこと（Claude Code の CLI を通す限りできない）は範囲外

## 7. 付記 D6（2026-10-08、task 01M4D3XKE2PZKTNSKHJGBQMEK0）: 固定 Core と可変部の分離（T4 の実装）

状態: **実装済み**。H1 の決定的な部分（LLM 無しベンチ）を満たした。live の cache read の変化は未計測（T8）。

### D6.1 分け方

`crates/task-worker/src/cos_chat.rs` の `build_parts` が `CosChatPrompt { core, variable }` を返す（`claude_code::build_cos_chat_parts` が browser の節を可変部の末尾に足す）。

- **Core**（`cos_chat::core`）: 返事と操作の規則、OP（`/cos/operations`）の雛形、checkpoint と履歴 API の雛形、添付の扱いと pin の規則、受信箱の件への relay の規則、headless の注意、作業の規則（成果物の置き場所・本番 host・subagent と別 LLM CLI の禁止・`/tmp`）。パラメータは `api_base_url` と `credential_env` の**名前**だけ。thread/run/task id・seq・日時・checkpoint の値は入れず、`<thread id>`・`<chat run id>`・`<through_seq>`・`<expected>` の置き場だけを書く。5,919 B（ベンチの API URL で。上限 6,000 B を試験で固定）
- 作業の規則は `preamble::fixed_notes()`（全 run 共通の 4 節、1,995 B）の CoS 向けの短縮版（`WORK_RULES`）。Core を 6 KB に収めるため。worker 一般の preamble は変えない（`preamble::render` は従来と同じ bytes。`render_without_fixed_notes` を足しただけ）。片方を変えたらもう片方も直す
- 添付の pin 規則と受信箱の relay 規則は、これまで添付・受信箱の件があるときだけ出していた。Core を run 間で同じ bytes にするため常に入れる（件と添付の一覧は可変部）
- **可変部**: `# CoS chat: thread <id>`、skill 名、「run 固有の操作パラメータ」節（thread id・chat run id・checkpoint の through_seq と expected・worker run id・一時 task id）、context 依存の preamble（今は空）、今回の入力、要約、未要約範囲、受信箱の件、添付の一覧

### D6.2 harness ごとの載せ方と根拠

| harness | Core の経路 | 可変部 | 根拠 |
|---|---|---|---|
| claude-code | `--append-system-prompt` の 1 引数: `HEADLESS_RUN_NOTE` + `\n` + Core | stdin | Anthropic の cache は tools → system → messages の完全 prefix 一致。system は `--resume` でも毎 request の先頭に同じ bytes で載り、会話（messages）に積まれない。従来は Core 相当の約 4 KB が resume の turn ごとに user message として積まれていた（D1.3） |
| codex | `-c developer_instructions=<TOML 文字列>`（`exec` と `exec resume` の両方に同じ bytes） | stdin | codex-cli 0.161.0（この host の版。`codex exec --help` と binary の文字列で確認）は AGENTS.md を `# AGENTS.md instructions for <cwd>` の見出し付きの user message にする。CoS chat の cwd は thread ごとの workspace なので、AGENTS.md 方式では thread をまたぐ prefix が Core の手前で切れる。`developer_instructions` は developer message で、AGENTS.md と環境情報（cwd）より前に入り、cwd を含まない。値は TOML 文字列にして渡す（`-c` は TOML として解釈し、失敗時だけ生の文字列になる。`cos_chat_core_codex_developer_instructions_round_trip` で往復を確認）。`instructions`（base instructions）は codex 自身の system prompt を置き換えるので採らない。resume で developer message が再挿入されるか（同じ bytes なので prefix は保たれるが、積まれるかどうか）は live 未確認で**不明** |
| acp（opencode 等） | 入力（`session/prompt`）の先頭: Core → skill 一覧 → 可変部 | 同じ入力の後半 | ACP の `session/new` には system prompt の欄が無い（cwd・mcpServers だけ）。agent 固有の経路（opencode の instructions 等）は agent ごとに違い、汎用の adapter から決められない。`session/load` の resume では turn ごとに Core が積まれる（T6 の差分配送で扱う） |
| pi | 入力（stdin）の先頭: Core → skill 一覧 → 可変部 | 同じ入力の後半 | pi の session は run ごと（`--session-dir <run_dir>/pi-sessions`）で thread をまたがない。system への経路（CLI の system prompt 追記の flag）は、この host に pi が無く版も確かめられないので採らない（**不明**）。入力の先頭に置けば、pi 自身の system prompt が run 間で同じなら Core まで prefix が一致する |

### D6.3 記録

`runs/<run_id>/prompt.txt` は、Core と入力を別の経路で渡す harness（claude-code・codex）では `<!-- celeris:cos-core (<経路>) -->` と `<!-- celeris:cos-input (stdin) -->` の 2 区画で両方を残す（`cos_chat::prompt_record`）。acp・pi は入力の全文がそのまま両方を含む。

### D6.4 決定的ベンチの before / after（`cargo test -p task-worker cos_chat_bench_`）

| 指標 | before（main `8cdd96fb`） | after |
|---|---|---|
| 新規 thread 10 件の先頭一致（1 本の入力として連結） | 43 B | 5,962 B |
| 同上、claude-code（system 引数 + stdin） | 1,262 B（system は HEADLESS_RUN_NOTE 1,219 B だけ + stdin 43 B） | 7,182 B（system 7,139 B が 10 件で同一 + stdin 43 B） |
| 同 thread の turn 間の先頭一致（連結 / claude-code） | 84 B / 1,303 B | 6,257 B / 7,477 B |
| 固定部の bytes | 4,226 B（preamble 1,995 + 規則 2,103 + skill 名 128。run ごとの値で分断） | Core 5,919 B（run 間で byte 一致）+ skill 名 128 B |
| claude-code の stdin（turn 1 / turn 10） | 4,783 B / 11,097 B | 883 B / 7,117 B |
| 入力の総 bytes（連結。acp/pi の入力） | 4,747 B | 6,766 B（pin・relay の規則を常に入れるため +2.0 KB） |

### D6.5 残り

- live（隔離 daemon・claude_oauth）の cache read / write の変化は T8 で測る。baseline では新規 thread の非 cache input がすでに 8 token なので、効果は cache write の減少と cache read の増加で見る
- acp・pi の resume で Core が積まれる問題は T6（差分配送）で扱う
