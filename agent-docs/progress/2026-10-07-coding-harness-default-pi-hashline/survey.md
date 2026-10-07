---
title: coding harness 既定選択の現状調査（adapter 決定経路・family 判定材料・metrics・Pi/Hashline の host 確認）
tasks: [01M49XA3X50V0R2YFTQRR9E8D5]
status: done
updated: 2026-10-07
---
# coding harness 既定選択の現状調査（survey）

WorkUnit `survey`。コードは変更していない。基点は `95769271`（WU base）。根拠は file:line で示す。後続の ADR `2026-10-07-coding-harness-default-pi-hashline` の材料。

## 決定経路

### 1. 用語
- adapter id は文字列。worker 実装の定数は次の 3 つ: `ClaudeCodeAdapter::ID = "claude-code"`（crates/task-worker/src/claude_code.rs:67）、`CodexAdapter::ID = "codex"`（crates/task-worker/src/codex.rs:100）、`AcpAdapter::ID = "acp"`（crates/task-worker/src/acp.rs:103）。
- `WorkerHint { tier, adapter: Option<String> }`（crates/task-core/src/model.rs:84-87）が task ごとの明示 adapter を持つ。`None` のときは provider 選択に任せる。
- ハーネス（`[[harnesses]]`）は `HarnessSpec.adapter: Option<String>` を持つ（crates/task-core/src/harness.rs:97-98）。組み込みハーネスで adapter を固定しているのは `smoke`（fake、harness.rs:203）と `knowledge`（langmem、harness.rs:215）だけ。`conversation`・`plan`・`reviewer` は `None`（harness.rs:175-198）。コード側に組み込みの `coding` ハーネスは無く、config の `[[genres]]`／`[[harnesses]] id = "coding"` で定義する（例: config/celeris.research.example.toml:199）。

### 2. 明示 adapter の指定箇所（task 作成時に決まる。上の行ほど優先）
`task-ops::add` の解決（crates/task-ops/src/add.rs:842-846）:
1. browser を要求する task では `browser_adapter`（add.rs:655-669）
2. 要求の `adapter`（`AddSpec.adapter`、add.rs:141-143。API/celerisctl の明示値）
3. 明示した `role` の `adapter`（add.rs:579）
4. assignee（組織 node）の役割の `adapter`（`org_role`、add.rs:595 付近の注記）
5. genre の `default_role` の `adapter`（`genre_role`、add.rs:625-628）
6. どれも無ければ `None`

ほかの経路:
- 子 task（delegate）: role → genre_role → 親の `worker_hint.adapter`（crates/task-core/src/delegate.rs:407-410）。
- 対話 task: 役割の adapter（crates/task-ops/src/conversation.rs:283）。
- 後から変える: `PATCH` の `adapter`（crates/task-ops/src/edit.rs:62-64, 365-367）。
- planner run: dispatch 時に `[execution.planner].adapter` で上書きし（crates/task-dispatch/src/dispatcher/dispatch_run.rs:144）、既定値は `"claude-code"`（crates/celeris/src/config/execution.rs:369-371）。
- knowledge の fallback: dispatch 時に `worker_hint.adapter = None` に戻し、cheap の汎用ハーネスへ倒す（dispatch_run.rs:333-339、crates/task-dispatch/src/dispatcher/worker_task.rs:141）。

### 3. 既定 adapter の解決（`worker_hint.adapter == None` のとき）
- 明示 adapter が無い task の adapter は、**選ばれた provider 行の `adapter`** で決まる。`StaticPolicy::matches`（crates/task-dispatch/src/policy.rs:165-175）は次のとおり。
  - `Some(a)` なら `p.adapter == a` の行だけを候補にする。
  - `None` なら `paperqa` / `local-deep-research` / `langmem` 以外の全行を候補にし、さらに `p.tiers` に lane を含む行に絞る。
- 選択の順序は `select_provider_excluding`（crates/task-dispatch/src/dispatcher/provider_select.rs:334-532）による。
  0. sticky session（同じ adapter・account に留まる。provider_select.rs:346-358、crates/task-dispatch/src/sessions.rs:188 `decide_sticky`、対応 adapter は `SUPPORTED_ADAPTERS = ["claude-code","codex","acp"]` sessions.rs:12）
  1. cheap lane で local 行を優先（ADR-0132 付記 L2/L3。provider_select.rs:366-423）
  2. account pool の行を残量 score で選ぶ（provider_select.rs:460-487）
  3. pool でない最初の行を fallback にする（provider_select.rs:488-493）
  4. `best_pool.or(fallback)`（provider_select.rs:507-510）
- **したがって現状の「既定 adapter」は provider の並びと残量の副産物である。** Claude 系モデルなら claude-code、非 Claude なら別の harness、という規則はどこにも無い。
- ADR-0061 の `task_core::routing::StaticRoutingPolicy`（crates/task-core/src/routing.rs:120-180）は aider / mini-swe-agent / acp / claude-code / codex の候補列を返すが、**ライブ経路には配線されていない**。workspace 全体で `routing::StaticRoutingPolicy` / `RoutingSignals::from_task` の呼び出しは試験だけにある。これは ADR-0061 D4 の意図どおり（agent-docs/adr/0061-coding-harness-routing-foundation.md:129-145）。routing.rs:7-9 の doc にも「未導入のハーネス（例: aider・pi）が挙がっても … `adapter: None` にフォールバック」とある。
- 使える adapter id は config の検証で閉じている。providers は crates/celeris/src/config/providers.rs:470-481、`[[harnesses]]` は crates/celeris/src/config/harness.rs:563-576、`[[roles]]` は harness.rs:607-615 で、いずれも `fake, claude-code, codex, aider, acp, browser-specialist, paperqa, local-deep-research, langmem` 以外を拒否する。`pi` を足すにはこの 3 か所と `build_adapters` の match（crates/celeris/src/daemon/adapters.rs:24, 44, 87）が要る。
- provider ごとの adapter の実体は `build_adapters`（crates/celeris/src/daemon/adapters.rs:20-）が作る。ACP 行は `command`/`args` を行ごとに上書きできる（adapters.rs:87-93。他の adapter で `command`/`args` を書くと providers.rs:492-500 が拒否する）。
- 現状 Pi を ACP として載せる道は無い: Pi に ACP モードは無い（下の host 確認を参照）。

### 4. fallback / retry・account pool・cheap lane Qwen
- **retry の失敗分類**: `provider_failure_outcome`（provider_select.rs:73）は AdapterError を Throttled / AuthFailed / Exhausted に分け、`StaticPolicy::report`（crates/task-dispatch/src/policy.rs:195-217）で provider 単位の cooldown を付ける。次の選択では cooldown の行を飛ばす。明示 adapter がある task は同じ adapter の別行にしか倒れない（policy.rs:166-167）。
- **harness fallback**: `HarnessFallback`（crates/task-core/src/harness.rs:62-89）を今読むのは knowledge だけ（harness.rs:57）。
- **run 間 escalation**（lane を上げる）は ADR 2026-10-04 §5（agent-docs/adr/2026-10-04-multi-objective-model-routing.md:148-157）。adapter は変えない。
- **account pool**: `AccountAdapter = ClaudeCode | Codex | OpencodeGo`（crates/task-core/src/accounts.rs:15-20、文字列は accounts.rs:31-37）。`AccountPoolSetting::pool_adapter`（accounts.rs:96-102）は行の adapter（`true` のとき）か、名前で指定した pool を返す。ACP 行は opencode-go pool を名前で指す（ADR 2026-10-06 D2/D3: agent-docs/adr/2026-10-06-opencode-go-and-model-catalog.md:38-48）。
- **llm_source の導出**（`derive_llm_source`、crates/celeris/src/config/providers.rs:282-）: claude-code → `ClaudeOauth`、codex → `CodexOauth`（providers.rs:283-287）。ACP は pool か model 接頭辞 `opencode-go/` なら `OpencodeGo`（providers.rs:290-296）、proxy なら `Celeris`、それ以外は openai_compatible などになる。`LlmSourceRef` は crates/task-core/src/provider_source.rs:14-23。
- **cheap lane の Qwen**: Qwen は cheap だけ（ADR-0132 D3/D5: agent-docs/adr/0132-provider-llm-source-split-and-cheap-qwen.md:33-50）。config 検査は crates/celeris/src/config/model_routing.rs:725-734（`Qwen is cheap only`）。dispatch では cheap lane の local 行（Qwen への ACP 直結、または Qwen を持つ proxy）を先に見る（provider_select.rs:366-423、ADR-0132 付記 L1-L3）。proxy 内の Qwen 失敗は proxy が Claude / GPT の cheap へ倒す（ADR-0132 D3、harness.rs:224-227 の注記）。その場合、**同じ run の中で model の family が変わっても adapter は変わらない**。

## family 判定の材料

- `ModelProfile.family: String`（crates/task-core/src/model_router/profiles.rs:47-56）は自由文字列で、enum ではない。
  - legacy proxy catalog は `"claude"` / `"gpt"` / `"qwen"` を書く（crates/llm-proxy/src/legacy_catalog.rs:100, 109, 121-127）。model id は `legacy:{family}:{wire}`（legacy_catalog.rs:47）。
  - `routing_catalog()` が provider 行の `model` / `tier_models` から作る model は `unknown_model` で `family: "unknown"` になる（crates/celeris/src/config/model_routing.rs:422-426, 527-531, 551-553）。`[[model_routing.models]] family = …` で上書きできる（model_routing.rs:349, 601-603）。
  - **dispatcher の legacy profile は `family: String::new()`（空）**（crates/task-dispatch/src/dispatcher/provider_select.rs:136-138）。dispatch 時点では family は分からない。
  - 既存の family 文字列判定は Qwen の 1 か所だけで、`eq_ignore_ascii_case("qwen")` と id/source の `contains("qwen")` を使う（model_routing.rs:725-728）。直書きの文字列判定の前例になっている。
- `DeploymentProfile`（profiles.rs:67-86）: `source_ref`（例 `claude-oauth` / `codex-oauth` / `openai-compatible:<id>` / `opencode-go`）、`model_profile_id`、`upstream_model`、`adapter_constraints: Vec<String>`、`billing`。dispatcher の legacy deployment は `adapter_constraints: vec![spec.adapter]`（provider_select.rs:171）で、ここから adapter が分かる。
- `AccountAdapter`（accounts.rs:15-20）と `LlmSourceRef`（provider_source.rs:14-23）は**型として閉じている**。`ClaudeOauth` / `AccountAdapter::ClaudeCode` は「Claude の subscription」を確実に示す。`Celeris`（proxy）・`OpencodeGo`・`OpenaiCompatible` は model を見ないと family が決まらない（opencode go は Claude 以外のモデルも出す。proxy は lane によって Claude / GPT / Qwen のどれにもなる）。
- 判定材料のまとめ（ADR で型にするための観察）:
  - 確実に Claude: `LlmSourceRef::ClaudeOauth`、`AccountAdapter::ClaudeCode`、`source_ref == "claude-oauth"`、family `"claude"`（legacy catalog）。
  - 確実に非 Claude: `CodexOauth`、family `"gpt"` / `"qwen"`、cheap の local 行（Qwen）。
  - 不明: `Celeris`（proxy、lane 依存）、`OpencodeGo`・`OpenaiCompatible`（model 次第）、family `"unknown"` / 空。→ 依頼の方針（未知は非 Claude 扱い）を当てはめる対象。
  - 文字列の直書きを避けるには、`ModelProfile.family` を enum 化するか、`LlmSourceRef` / `AccountAdapter` / `DeploymentProfile.source_ref` から導く関数を task-core に 1 つ置く必要がある。

## metrics

- **routing audit**（`RoutingAudit`、crates/task-core/src/routing_audit.rs:40-58）は run ごとに `harness`・`adapter`・`provider`・`account`・`lane`・`model` を**別欄で**持つ。
  - `harness` は `task.genre`（例 `"coding"`）で、adapter ではない（routing_audit.rs:137、`RoutingRecord.harness` は dispatch_run.rs:629-630）。
  - `adapter` は `Event::WorkerStarted.adapter`（routing_audit.rs:148-157、crates/task-core/src/model.rs:1086-1094）。
  - `model` は WorkerStarted.model を `RoutingDecided.resolution.model_id` で上書きする（routing_audit.rs:158-159, 176-177）。
  - `LaneResolution.adapter`（crates/task-core/src/model_routing.rs:38-39）は dispatch 時に実 adapter id を入れる（dispatch_run.rs:631-634）。
  - **したがって run 単位では (model, adapter) の組が取れ、比較できる。** ただし proxy 経由（`celeris/<tier>`）の run は `model` が lane 抽象名になり、実 model ではない（ADR 2026-10-04 §2 の表: agent-docs/adr/2026-10-04-multi-objective-model-routing.md:43-49）。
- **execution metrics**（`ExecutionMetrics`、crates/task-core/src/execution_metrics.rs:130-216）は task 単位の集計で、**adapter 欄も model 欄も無い**。
- `GET /metrics/execution` の `group_by` は `gate_mode, genre, assignee, lane, depth` だけ（crates/task-api/src/stats.rs:300-301）。adapter 別・model 別の集計は無い。account 集計は `"<adapter>:<account>"` key（stats.rs:195-208）で、adapter を区別する。
- 結論: (model, harness=adapter) の比較は routing audit（run 単位）なら今でもできる。execution metrics の group_by に `adapter` を足すかは ADR で決める。Pi を足す場合、`WorkerStarted.adapter = "pi"` が出れば routing audit は変更なしで比較できる。

## Pi/Hashline の host 確認

実行場所はこの worker の host（読み取りだけ。install・外部ネットワークはしない）。`pi --help` は `HOME=/tmp/pi-help-home` で実行し、本物の `~/.pi` を作らないようにした。

| 実行したコマンド | 出力の要点 |
| --- | --- |
| `command -v pi` | 出力なし（**PATH に未導入**） |
| `command -v hashline` | 出力なし（**未導入**） |
| `command -v pi-coding-agent` / `opencode` / `aider` | いずれも出力なし。claude=`~/.local/bin/claude`、codex=`~/.local/bin/codex` |
| `timeout 10 pi --version` | `timeout: failed to run command 'pi': No such file or directory` |
| `npm root -g` / `npm ls -g --depth=0` | `/usr/lib/node_modules`: `corepack@0.34.6`、`npm@11.11.0`、`openclaw@2026.5.20` だけ |
| `ls ~/.pi ~/.pi/agent/extensions ~/.config/pi ~/.npm-global ~/.bun/bin` | どれも `No such file or directory`（**Pi の extension の導入無し**） |
| `ls ~/.opencode/bin` / `~/.opencode/bin/opencode --version` | `opencode`（PATH 外）/ `1.18.35` |
| `grep '"@earendil-works/pi' /usr/lib/node_modules/openclaw/package.json` | `pi-agent-core` / `pi-ai` / `pi-coding-agent` / `pi-tui` がいずれも `0.75.4` |
| `grep -E '"bin"' …/@earendil-works/pi-coding-agent/package.json` | `"pi": "dist/cli.js"`（openclaw の依存として同梱。global bin には無い） |
| `HOME=/tmp/pi-help-home node …/pi-coding-agent/dist/cli.js --version` | `0.75.4`、exit 0 |
| `HOME=/tmp/pi-help-home node …/pi-coding-agent/dist/cli.js --help` | exit 0、152 行（要点は下） |
| `grep -ril hashline` を openclaw/dist、@earendil-works/*、pi-coding-agent の README/docs/CHANGELOG/examples に | 一致なし（**Hashline は同梱にも無い**） |
| `grep -rli 'agent-client-protocol\|session/prompt'` を pi-coding-agent/dist に | 一致なし（**ACP の実装は無い**） |

openclaw 同梱の `pi-coding-agent` 0.75.4 の `--help` と同梱 docs から確認できたこと（`…` = `/usr/lib/node_modules/openclaw/node_modules/@earendil-works/pi-coding-agent`）:
- **起動形**: `--mode text|json|rpc`、`--print, -p`（非対話で処理して終了）。`--mode json` は session event を JSON Lines で stdout に出す（…/docs/json.md:1-8）。`--mode rpc` は stdin/stdout 上の JSON line のコマンドと応答で、`id` で対応付ける（…/docs/rpc.md:1-25）。**ACP モードは無い**ので、既存の `acp` adapter には載らない。専用 adapter（`--mode json -p`、または `--mode rpc`）が要る。
- **session**: `--no-session`、`--session <path|id>`、`--session-dir`、`--continue`、`--fork`。env は `PI_CODING_AGENT_DIR`（既定 `~/.pi/agent`）、`PI_CODING_AGENT_SESSION_DIR`。
- **extension の読み込み**: `--extension, -e <path>`（複数指定可）、`--no-extensions, -ne`（自動発見を止める。`-e` の明示 path は効く）。自動発見の場所は `~/.pi/agent/extensions/*.ts`・`*/index.ts` と、project の `.pi/extensions/*.ts`・`*/index.ts`（…/docs/extensions.md:7, 116-119）。`pi install <source>` は settings に書き込む（読み取りでないので実行していない）。
- **tool の絞り方**: `--tools, -t <names>` は allowlist で、built-in・extension・custom の tool すべてに効く。`--no-tools`、`--no-builtin-tools` もある。built-in は `read, bash, edit, write`（既定で有効）と `grep, find, ls`（既定で無効）。skill は `--skill` / `--no-skills`。context file（AGENTS.md / CLAUDE.md）の読み込みは `--no-context-files` で止まる。
- **subagent**: core に sub-agent は無い（…/README.md:476「No sub-agents.」、…/docs/usage.md:277）。sub-agent は extension（examples/extensions/subagent、…/docs/extensions.md:2579）で足すものなので、**`--no-extensions` と、`-e` で Celeris が渡すものだけを許すことで無効にできる**（`--tools` の allowlist でも extension tool を外せる）。
- **model / provider**: `--provider`（既定 google）、`--model provider/id[:thinking]`、`--thinking`。help の env 一覧に `ANTHROPIC_OAUTH_TOKEN` と `OPENCODE_API_KEY`（OpenCode Zen/Go）がある。`--offline` / `PI_OFFLINE=1` で起動時のネットワーク操作を止める。
- **Hashline**: 推測（公開仕様は未確認、host に無い）。hash を付けた行参照で編集する Pi の extension として配布されているもの、と理解している。導入すれば `-e <path>` で読み込み、`--tools` の allowlist に入れる形になるはずだが、tool 名・引数は未確認。ADR では「未導入・仕様は推測」と明記したうえで、adapter が extension path と tool 名を config で受ける形にするのが安全。

## 未解決事項
- Hashline の実際の tool 名・配布形態・version（host に無く、外部ネットワークは使えない）。
- `pi` を本番 host のどこに置くか（openclaw 同梱版を使うか、別に global 導入するか）。人の決定事項。
- Pi の `--mode json` の終端 event と usage（token / cost）の取り出し方の詳細（docs/json.md の AgentEvent 定義は読んだが、試験 fixture は後続の pi-adapter 葉で作る）。

## 提案
- family 判定は task-core に enum（例 `ModelFamily { Claude, Other }`）と導出関数を 1 つ置く。入力は `LlmSourceRef` / `AccountAdapter` / `DeploymentProfile.source_ref` / `ModelProfile.family`。`Celeris`（proxy）と不明は非 Claude にする。dispatch で確定するのは provider 行が決まった後なので、明示 adapter の無い coding task だけに、選んだ provider 行から決める形が自然。
- execution metrics の `group_by` に `adapter` を足すと、(model, harness) の比較が task 単位でもできる（routing audit は現状のままで足りる）。
