# ADR 2026-10-07: worker は内部で subagent や別の LLM CLI を起動しない（全 adapter の既定禁止・前置きの規則・検出と記録）

- 日付: 2026-10-07
- 状態: 実装済み（task 01M49WJP27NNVDE5E3FCY82DDP）。CoS の例外（D7）は人の決定待ち（既定は禁止）
- 関連: ADR-0001（dispatcher / store に LLM を入れない）、ADR-0003（worker protocol）、ADR-0006 D6（claude-code adapter の設定）、
  ADR-0008 D3/D4（codex adapter）、ADR-0026 D2/D3（ACP adapter）、ADR-0048 D2（進行の正規化）、ADR-0069（routing の 4 層）、
  ADR-0074 D4（quota）、ADR-0079（再帰 task 木）、ADR-0117 D1/D2（reviewer に渡す人の決定と決定的 check）、
  ADR 2026-10-05-cos-chat-home D3（CoS の「全道具・全権限」。別 branch）

## 1. 文脈

2026-10-07、task 01M49KWT59（BenchFS の比較）の run 01M49KWW6J255FA99BJSFHS05E（claude-code、claude-fable-5-1）が、
計画のレビューを Claude Code の `Agent` 道具の subagent に任せた。人の確認になるはずの段が worker の内部で済まされ、
subagent の使用量は Celeris の run・routing・quota の記録（ADR-0069 / ADR-0074 D4）の外に出た。

Celeris の設計では、並列化・分担・レビューは**計画**（execution plan の子 task / WorkUnit。ADR-0072 / ADR-0079）と
**人への質問**で行い、1 run = 1 process = 1 つの LLM の会話として記録する（ADR-0003）。worker が内部で別の agent を
起こすと、(a) コスト・quota の会計が壊れ、(b) 人の確認や reviewer の判定を worker 自身が肩代わりして「完了は reviewer か
決定的な検査が決める」原則が崩れ、(c) task flow（子 task・WU の木）に残らない作業が生まれる。

応急処置として運用セッションが本番 config に `[adapters.claude_code] extra_args = ["--disallowedTools=Agent,Task"]` を
入れた。これは claude-code にしか効かず、config を外せば消える。コードの既定にする。

### 1.1 一次情報（この host の実機で確認。2026-10-07）

| harness | 版 | subagent / 並列 agent の機能 | 塞ぐ手段 |
|---|---|---|---|
| Claude Code | 2.1.287 | 道具 `Agent`（旧名 `Task`）が subagent を起こす。`Workflow` 道具が複数 subagent を orchestrate する。`--forward-subagent-text` オプションと `claude agents` サブコマンドがあり、subagent は製品の中核機能 | `--disallowedTools <tools...>`（`claude --help`: "Comma or space-separated list of tool names to deny"）。`--allowedTools` より deny が勝つ |
| Codex CLI | 0.160.1 | `codex features list` に `multi_agent  stable  true`、`multi_agent_v2  stable  false`、`agent_message_board  under development  false`。binary に `spawn_agent` 等の道具名が 44 箇所。既定で有効 | `-c features.multi_agent=false`（`codex exec --help`: `--disable <FEATURE>` は `-c features.<name>=false` と等価）。`codex features list -c features.multi_agent=false` で `false` に変わることを確認。`-c` は `exec resume` でも受け付ける（ADR-0054 Phase 68c のホワイトリスト）。未知の `features.*` キーは exit 0（警告のみ） |
| opencode（ACP） | 1.18.35（本番 `~/.opencode/bin/opencode`） | 道具 `task`（"Launch a new agent to handle complex, multistep tasks autonomously."、入力 `subagent_type`）。agent の `mode: "subagent" | "primary" | "all"`、command の `subtask` | config の top-level `tools: { [key: string]: boolean }`（`@opencode-ai/sdk` 1.18.31 `Config` 型）で `task: false`。config は env `OPENCODE_CONFIG_CONTENT`（JSON）を「local」の層として読む（binary の `Config` 読み込み経路で確認）。ACP protocol 自体には道具単位の deny は無い（ADR-0026 D3 / ADR-0054 Phase 68 追記のとおり） |

claude-code 以外の 2 つも subagent を持つので、全 adapter で既定禁止にする。

## 2. 決定

### D1. claude-code adapter は既定で subagent の道具を禁止する

- argv に `--disallowedTools Agent,Task,Workflow` を付ける。道具名の一覧は `task_worker::tool_policy::SUBAGENT_TOOLS`（1 か所。
  将来の同種の道具はここに足す）。
- **`extra_args` の後ろ**に付ける（最後の語が勝つ。運用側の `extra_args` に `--allowedTools Agent` や `--tools default` が
  あっても deny は外れない）。既存の本番の `extra_args = ["--disallowedTools=Agent,Task"]` と重なっても害は無い
  （同じ道具を二重に deny するだけ）。コードの既定が入ったら運用セッションが `extra_args` を外す。
- 外す方法は **明示の設定だけ**: `[adapters.claude_code] subagents = "allow"`（D7 の `"allow_cos"` も）。`extra_args` では外れない。
- 既存の `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`（F5-fix5）はそのまま（background の subagent も塞ぐ）。

### D2. codex adapter は既定で multi-agent を無効にする

- fresh / `exec resume` の両方に `-c features.multi_agent=false` と `-c features.multi_agent_v2=false` を付ける
  （`-c key=value` は両形で受け付ける。ADR-0054 Phase 68c）。`extra_args` より**後ろ**に置き、`extra_args` では外れない。
- 外す方法は `[adapters.codex] subagents = "allow" | "allow_cos"` だけ。

### D3. ACP adapter（opencode）は既定で `task` 道具を無効にする

- 起動時の env `OPENCODE_CONFIG_CONTENT` に `{"tools":{"task":false}}` を**重ねる**（`tool_policy::opencode_overlay`）。
  既に入っている JSON（運用側の `[adapters.acp] env` / `[[providers]].env` の値、または
  `routing_context_transport::configure_acp` が入れた provider header の overlay）があれば、その object に `tools.task=false`
  を merge する（他の鍵は保つ）。JSON でなければ上書きしない（壊れた値を壊れたまま渡すのは今までどおり）。
- ACP adapter は opencode 専用ではない（`command` を差し替えられる。ADR-0127）。他の ACP agent にはこの env は効かないので、
  D4 の前置きと D5 の検出で補う。ACP protocol に deny を頼む経路は無い。
- 外す方法は `[adapters.acp] subagents = "allow" | "allow_cos"` だけ。

### D4. 全 harness の前置きに規則を足す

`preamble::tool_launch_policy_note`（`preamble::render` が常に出す。ADR-0067 D1 の「成果物の置き場所」・ADR-0095 付記 D-d の
「本番 host の操作」と同じ扱い）:

> ## 道具の起動の制約 (no subagents, no other LLM CLIs)
> subagent・並列 agent・別の LLM の CLI（`claude`・`codex`・`opencode`・`gemini`・`aider` など）や LLM の API
> （`api.anthropic.com`・`api.openai.com` など）を自分で起動しない。並列化や分担、別の目で見る確認が要るなら、
> 計画（execution plan の子 task / WorkUnit）、`delegate.json`、人への質問で行う。起動は検出されて event に記録され、
> レビュアーに渡る。

`claude-code` / `codex` / `acp` は `claude_code::build_prompt` を共有し、`paperqa` も `preamble::render` を使うので、
1 か所の追加で全 harness に出る。`local-deep-research` は設計上前置きを使わない（ADR-0029。検索に渡す問いを濁さない。
その harness は shell を持たないので対象外）。

### D5. LLM CLI / API の起動と subagent 道具の使用を検出し、event に記録する

- `task_worker::tool_policy::inspect_tool_use(tool, input)`（純粋関数。LLM も I/O も無い）が、各 adapter の進行の写像
  （ADR-0048 D2）と同じ場所で `tool_use` を見る:
  - claude-code: stream-json の `tool_use`（`name` / `input.command`）
  - codex: `item.started` / `item.updated` の `command_execution`（`command`）
  - acp: `session/update` の `tool_call`（`title` と `rawInput.command`）
- 判定（決定的）:
  1. **subagent 道具**: 道具名が `SUBAGENT_TOOLS`（`Agent` / `Task` / `Workflow` / `task` / `spawn_agent` / `spawn_agents_on_csv`）。
  2. **LLM CLI**: shell command を `;` `&&` `||` `|` 改行 `$(` `` ` `` で区切り、各区間の先頭語（`FOO=bar` の環境変数代入、
     `sudo` / `env` / `nohup` / `exec` / `command` / `nice` / `timeout <n>` / `xargs` / `time` / `stdbuf …` の前置きを剥がした後）の
     basename が `LLM_CLIS`（`claude` / `codex` / `opencode` / `gemini` / `aider` / `cursor-agent` / `copilot` / `goose` / `amp` /
     `qwen` / `kimi` / `llm` / `ollama`）に一致する。`which claude` や `cargo test claude_code` は一致しない（先頭語ではない）。
  3. **LLM API**: command に `LLM_API_HOSTS`（`api.anthropic.com` / `api.openai.com` / `generativelanguage.googleapis.com` /
     `openrouter.ai` / `opencode.ai/zen` / `api.x.ai` / `api.mistral.ai` / `api.deepseek.com` / `api.groq.com` / `api.together.xyz`）
     が含まれ、かつその区間の先頭語が読むだけの道具（`grep` / `rg` / `git` / `cat` / `sed` / `awk` / `head` / `tail` / `less` /
     `find` / `echo` / `printf` / `diff` / `wc` / `sort`）ではない。
- 検出したら sink の `policy_violation(&ToolPolicyViolation)` を呼ぶ。dispatcher の `StoreSink` / `ReviewerSink` は
  `Event::WorkerPolicyViolation { run_id, kind, tool, matched, command }`（`kind` は `subagent_tool` / `llm_cli` / `llm_api`、
  `command` は 500 文字で切る）を追記し、同時に人が Console で見られるよう `WorkerProgress`（`kind = status`、`error = true`、
  `msg = "policy: …"`）も残す。run は**止めない**（警告。止めるかは reviewer と人が決める）。
- 誤検出の可能性（例: `claude --version` の確認、API host 名を含む curl 以外のコマンド）は残る。検出は「警告の記録」であり、
  それだけで fail にはしない（D6）。

### D6. reviewer に見せる

- `ReviewRequest.policy_violations: Vec<ReviewPolicyViolation>`（`kind` / `tool` / `matched` / `command`）。dispatcher
  （`review_spawn`）が対象 run の `WorkerPolicyViolation` を store の events から集めて渡す（ADR-0117 D1 の decisions / answers と
  同じ経路。LLM は関与しない）。
- review prompt に節 "Tool policy violations recorded by celeris (authoritative)" を出す: worker が subagent や別の LLM を
  起動した事実、その usage が Celeris の会計の外にあること、**その起動が criterion の要求する人の確認・独立のレビュー・
  決定的 check の代わりになっているなら、その criterion は fail** にすること、そうでなければ verdict の reason に明記すること。
- GUI / web はこの event を特別扱いしない（`worker_progress` の `error = true` の行として見える。専用 UI は別 task）。

### D7. CoS の例外は人の決定。既定は禁止

ADR 2026-10-05-cos-chat-home D3 は CoS を「全道具・全権限」の特別 worker にする。subagent をその「全道具」に含めるかは
人が決める。選択肢:

| 選択肢 | 設定 | 利点 | 欠点 |
|---|---|---|---|
| **A. CoS も禁止（既定・推奨）** | `subagents = "deny"`（省略時） | CoS の並列化も Celeris の task 起票（`create_task` / 計画）で行われ、会計と task flow に全部残る。1 run = 1 会話の記録が保たれる | CoS が「ちょっと調べる」程度の分担を自分で切れない |
| B. CoS の対話 run だけ許す | `subagents = "allow_cos"` | CoS の軽い並列化が速い | CoS の subagent の usage は run の usage に合算されるだけで、routing / quota の内訳に残らない。人の確認を CoS が内部で済ませる余地が残る |
| C. 全 run で許す | `subagents = "allow"` | 応急処置前の挙動 | 本 ADR の文脈の事故が再発する |

- 実装は 3 択を全 adapter の `[adapters.<id>] subagents` で受ける（既定 `"deny"`）。`"allow_cos"` は
  `RunContext.conversation_addressee == Secretary` の run（この tree での CoS の対話の印。ADR-0054 D2）にだけ効く。
  CoS chat（別 branch の ADR 2026-10-05-cos-chat-home D2）の run は、取り込み時に同じ印を立てるか、この判定に
  その印を足す（統合する側の task が決める。本 ADR では `Secretary` のみ）。
- 人への問いは task の進捗ファイル「未解決事項」と comment に書く。答えが A なら config は何も変えない。

### D8. 採らない: worker の PATH から LLM CLI を外す

- harness の binary 自身（`claude` / `codex` / `opencode`）は同じ PATH で解決され、自分自身を再起動する（`claude` は
  `--bg` / 更新確認、codex は app-server）ことがある。PATH から外すと harness が壊れる経路が読み切れない。
- `container` / `db_guard::launch` の経路（ADR-0043 D3 / ADR-0095）で PATH の形が変わり、絶対 path の起動
  （`~/.local/bin/claude`）は PATH では塞げない。
- 代わりに D1–D3 の道具禁止（起動の手前で塞ぐ）、D4 の規則、D5 の検出（記録して reviewer に渡す）で足りると判断する。
  不足が実機で見えたら、shell の wrapper（`PATH` の先頭に `claude` という名前の「拒否して exit 1 する」script を置く）を
  再検討する。

## 3. 試験（決定的。LLM もネットワークも使わない）

- claude-code の argv: 既定で `--disallowedTools` の値が `Agent,Task,Workflow`、`extra_args` に `--allowedTools Agent` /
  `--disallowedTools=Agent,Task` / `--tools default` があっても deny が**その後ろ**に残る、`subagents = "allow"` なら無い、
  `"allow_cos"` は CoS の対話 run にだけ無い。
- codex の argv: fresh / `exec resume` の両方で `features.multi_agent=false` と `features.multi_agent_v2=false` が
  `-c` の値にある。`"allow"` なら無い。
- acp: 偽 ACP agent が受け取った `OPENCODE_CONFIG_CONTENT` が `tools.task == false` を含み、既存の JSON の鍵が保たれる。
- 前置き: `preamble::render(&RunContext::default())` が規則の節を含む（既存の byte 一致の試験は 3 節の連結に更新）。
- 検出: `inspect_tool_use` の正例（`Agent`、`claude -p …`、`FOO=1 timeout 60 codex exec …`、`a && opencode run`、
  `curl https://api.anthropic.com/v1/messages`）と負例（`which claude`、`cargo test claude_code`、
  `grep api.anthropic.com -r crates`、`git commit -m "claude"`）。各 adapter の `handle_line` / `handle_notification` が
  sink に `policy_violation` を届ける。dispatcher の `StoreSink` が `WorkerPolicyViolation` を追記し、`review_spawn` の
  収集関数がそれを `ReviewPolicyViolation` に写し、review prompt に節が出る。
- celeris config: `[adapters.claude_code] subagents = "allow"` が解決され、未知の値は `deny` に倒れる。

## 4. 影響

- `Event` に `WorkerPolicyViolation` を足す（`EVENT_TYPES` 68 種。`docs/api/v1/{api-v1,event}.schema.json` を再生成し、
  gui / web の生成型と `web/api/realtime/{event-kinds,invalidation-map}.ts` を更新）。
- `ReviewRequest.policy_violations`（`#[serde(default)]`。古い request は空として読める）。
- `[adapters.claude_code|codex|acp] subagents`（既定 `"deny"`）。
- 本番: 運用セッションがこの ADR の release の後に `[adapters.claude_code] extra_args` の `--disallowedTools=Agent,Task` を
  外す（人の手順。本 task では本番 config に触れない）。
