# coding タスクの既定ハーネスを model family で決める（Claude 系は Claude Code、非 Claude は Pi + Hashline）

- 日付: 2026-10-07
- 状態: 実装済み（2026-10-07 close-out。leaf の実装突き合わせは「付記（実装との突き合わせ）」節。本番での有効化は人が行う: `docs/ops/coding-harness-pi-hashline.md`）
- 関連: ADR-0024（account pool）、ADR-0026（adapter）、ADR-0046（harness）、ADR-0049（専用 adapter の除外）、ADR-0052（harness fallback）、ADR-0061（coding harness routing の基盤）、ADR-0132（provider / llm_source 分離・cheap の Qwen）、ADR 2026-10-04（多目的 model routing）、ADR 2026-10-06（opencode go と model catalog）
- 調査記録: `agent-docs/progress/2026-10-07-coding-harness-default-pi-hashline/survey.md`（以下「survey」。根拠の file:line はそこにある）

## 背景

coding タスクの adapter（ハーネス実装）は、明示されなければ「選ばれた provider 行の `adapter`」で決まる（survey §3）。provider の並びと残量の副産物なので、Claude 以外の model（GPT・opencode go の各 model・cheap の Qwen）が、その model に合わない harness（opencode を ACP で包んだもの等）で走ることがある。人の方針は「Claude 系は Claude Code、非 Claude 系は Pi（pi coding agent）に Hashline の編集 tool を載せて走らせる」。ADR-0061 の `task_core::routing::StaticRoutingPolicy` は候補列を返すだけでライブ経路に配線されていない（ADR-0061 D4 の意図どおり）。

この ADR は、既定の決め方・family の判定・Pi の起動形・既存の fallback / account pool / cheap lane との関係・metrics を決め、後続の葉に分ける。

## 決定点

### 優先順位: 明示 adapter > 既定解決

1. **明示 adapter は今までどおり最優先で、この ADR は触らない。** `task-ops::add` の解決（browser_adapter → 要求の `adapter` → 明示 role → assignee の役割 → genre の `default_role`、crates/task-ops/src/add.rs:842-846）、delegate（crates/task-core/src/delegate.rs:407-410）、対話（crates/task-ops/src/conversation.rs:283）、PATCH（crates/task-ops/src/edit.rs:62-64）、planner の `[execution.planner].adapter`（crates/task-dispatch/src/dispatcher/dispatch_run.rs:144）が `WorkerHint.adapter = Some(_)` を入れたら、既定解決は走らない。
2. **既定解決は `WorkerHint.adapter == None` で、かつ task の harness が family 既定を選んでいるときだけ走る。** 判定は genre の文字列比較（`genre == "coding"`）ではしない。`HarnessSpec` に `adapter_policy: AdapterPolicy`（enum。`ProviderOrder`＝現状、既定 / `ModelFamily`＝この ADR）を足し、config の `[[harnesses]] id = "coding"` に `adapter_policy = "model_family"` を書いた harness だけが対象になる。これが本番の有効化スイッチを兼ねる（書かなければ挙動は今と同じ。戻すのは 1 行消すだけ）。
3. knowledge の fallback で `worker_hint.adapter = None` に戻した run（dispatch_run.rs:333-339）は knowledge harness なので対象外。

### 解決する関数と位置

- **task-core（純関数）**: 新 module `crates/task-core/src/coding_harness.rs`。
  - `CodingHarness { ClaudeCode, Pi }`、`CodingHarness::for_family(ModelFamily) -> CodingHarness`（Claude → ClaudeCode、それ以外すべて → Pi）。
  - `CodingHarness::adapter_id(self) -> &'static str`。`ClaudeCode` は `AccountAdapter::ClaudeCode.as_str()` と同じ値、`Pi` は `"pi"`。adapter id 文字列の正本はここ（と `AccountAdapter`）に置き、task-worker の `PiAdapter::ID` は同じ値であることを試験で固定する。
  - `prefers(row_adapter: &str, family: ModelFamily) -> bool`: 行の adapter がその family の既定 harness と一致するか。
- **task-dispatch（適用）**: `select_provider_excluding`（crates/task-dispatch/src/dispatcher/provider_select.rs:334）の冒頭で候補行を作った直後（`legacy_provider_rank(hint)` の allowlist を得た後、sticky・cheap local・pool score の前）に、`hint.adapter.is_none()` かつ harness の `adapter_policy == ModelFamily` のとき、候補を **「行の family から見て既定 harness に一致する行」（preferred）に絞る**。preferred が 0 行、または preferred がすべて cooldown・枠切れ・除外なら、絞る前の候補で従来の選択を行う（fallback。理由を routing audit に残す、下記 metrics）。preferred の中の順序（sticky → cheap local → pool score → 最初の行）は既存のまま変えない。
- 行の family は、provider_select.rs:136-138 で今は空文字の `ModelProfile.family` と `DeploymentProfile` を作るところで導出する（次節）。
- `StaticPolicy::matches`（crates/task-dispatch/src/policy.rs:165-175）は変えない。adapter の互換判定と preferred の絞り込みを混ぜない。
- sticky session（crates/task-dispatch/src/sessions.rs:188 `decide_sticky`）は preferred の中でだけ効かせる。preferred でない行に貼り付いた session は、preferred 行が使えるなら捨てる（continuation は checkpoint fallback で続く。ADR-0140）。`SUPPORTED_ADAPTERS`（sessions.rs:12）に `pi` を足すのは pi-adapter 葉で session resume を実装してから（それまで Pi の run は毎回新しい session）。

## Claude family 判定

### 型

- `crates/task-core/src/model_family.rs` に `ModelFamily { Claude, Gpt, Qwen, Other, Unknown }` を置く（`Serialize` は snake_case）。`is_claude(self) -> bool` は `Claude` のときだけ true。**未知（`Unknown`・`Other`）は非 Claude として扱う。**
- `ModelFamily::parse(&str) -> ModelFamily` を、自由文字列 `ModelProfile.family` から enum へ変える **唯一の場所** にする（大文字小文字を無視して `claude` / `gpt` / `qwen`、空は `Unknown`、それ以外は `Other`）。他の crate で `family == "claude"`・`contains("claude")`・`starts_with("claude-")` のような直書きをしない。既存の Qwen 判定（crates/celeris/src/config/model_routing.rs:725-728 の `eq_ignore_ascii_case("qwen")` と `contains("qwen")`）も family 葉で `ModelFamily::parse(..) == Qwen` に置き換える（id/source の `contains("qwen")` は catalog に family が無い古い config 用に残すかを family 葉で判断し、残すなら `ModelFamily` の関数に移す）。
- `ModelProfile.family: String` の型は変えない（API schema・legacy catalog の互換のため）。読む側が `ModelFamily::parse` を通す。

### 導出（`derive_family`）

`pub fn derive_family(source: &LlmSourceRef, pool: Option<AccountAdapter>, model: &ModelProfile, deployment: &DeploymentProfile) -> FamilyDecision`。`FamilyDecision { family: ModelFamily, basis: FamilyBasis }`、`FamilyBasis { LlmSource, AccountPool, ModelProfile, Unknown }`。上から順に最初に決まったものを採る:

1. **`LlmSourceRef`**（crates/task-core/src/provider_source.rs:14-23、閉じた型）: `ClaudeOauth` → `Claude`、`CodexOauth` → `Gpt`。
2. **`AccountAdapter`**（crates/task-core/src/accounts.rs:15-20、行の `pool_adapter`）: `ClaudeCode` → `Claude`、`Codex` → `Gpt`。`OpencodeGo` は model 次第なので決めない（次へ）。
3. **`ModelProfile.family`** を `ModelFamily::parse`: routing catalog（`[[model_routing.models]] family = …`、model_routing.rs:349, 601-603、ADR 2026-10-06 の model catalog）で family が書かれていれば決まる。dispatcher は provider_select.rs:136 で空文字を入れているので、dispatch 葉で catalog の model profile（`model_profile_id` で引く）から family を写す。
4. どれでも決まらなければ `Unknown`（→ 非 Claude）。

`DeploymentProfile` は `source_ref`（provider_select.rs:154-162 で `LlmSourceRef` から作る文字列）を持つが、**文字列の `source_ref` は判定に使わない**。dispatch では `live.llm_source.source`（`LlmSourceRef` そのもの）を渡す。`DeploymentProfile` からは `adapter_constraints`（行の adapter）だけを `prefers` に使う。

### 決まりにくい行の扱い（明記）

| 行 | family | 既定 harness | 備考 |
| --- | --- | --- | --- |
| claude-code・`claude_oauth` | Claude（LlmSource） | claude-code | 今と同じ |
| codex・`codex_oauth` | Gpt（LlmSource） | pi | codex 行は preferred でなくなる。Pi の codex 行が無ければ fallback で今どおり codex が走る |
| acp / pi・`opencode_go` pool | catalog の family。無ければ Unknown | Claude 系 model（opencode go が Claude を出す場合）なら claude-code、他は pi | Claude model を pi 行で出していても preferred でない |
| acp / pi・`openai_compatible`（cheap の Qwen） | Qwen（catalog）/ Unknown | pi | |
| 任意の adapter・`celeris`（llm-proxy） | lane 次第で決まらない → Unknown | pi | proxy の Claude lane も非 Claude 扱い。proxy 行を Claude として使いたいなら catalog で family を書いた model 行にするか、明示 adapter を使う |
| claude-code・`celeris`（claude-code を proxy に向けた行） | Unknown | pi | この行は preferred でない（fallback でのみ使う） |

## Pi 起動形と tool 集合

### host 上の確認の要約（survey「Pi/Hashline の host 確認」）

`pi` は PATH に無い。openclaw 2026.5.20 の依存として `@earendil-works/pi-coding-agent` 0.75.4 が `/usr/lib/node_modules/openclaw/node_modules/` に同梱され、`node …/pi-coding-agent/dist/cli.js --help` が動く。Hashline は host のどこにも無い。以下は同梱版の `--help` と同梱 docs から確かめた事実と、確かめていない点の区別。

### adapter

- **adapter id: `pi`**（新規。`CodingHarness::Pi.adapter_id()`）。task-worker に `crates/task-worker/src/pi.rs` の `PiAdapter` を足す。
- **ACP ではない。** pi-coding-agent に ACP の実装は無い（dist に `agent-client-protocol` / `session/prompt` が無い）ので、既存の `acp` adapter に `command = "pi"` で載せることはできない。
- **起動形は `--mode json -p`（非対話・JSON Lines）を採る。** `--mode json` は session event を JSON Lines で stdout に出す（…/docs/json.md）。claude-code / codex adapter と同じ「1 run = 1 process、stdout の event を読み切って判定」の形に揃い、既存の run log・result 読み取り・timeout・SIGSTOP stutter 試験の型をそのまま使える。`--mode rpc`（stdin/stdout の双方向 JSON、id 対応、…/docs/rpc.md）は、run 中の割り込み・追加入力が要るまで使わない。
- 起動の引数（PiAdapter が組み立てる。config で上書きしない固定部分）:
  - `--mode json -p <prompt>`
  - `--no-session` は使わず `--session-dir <run の作業領域>/pi-sessions`（resume を後で実装できるように session を run の作業領域に残す。`PI_CODING_AGENT_SESSION_DIR` でも同じ）
  - `PI_CODING_AGENT_DIR=<account dir または run 専用 dir>`（本物の `~/.pi/agent` を読ませない。auth.json・settings・自動発見の extension をここに閉じる）
  - `--provider <p> --model <p>/<id>`（行の `model` から。`opencode-go/…` 等）
  - `--no-extensions` と `-e <hashline の path>`（下記）
  - `--tools <allowlist>`（下記）
  - `--no-skills`（Celeris の skill 配送は別経路で行う。ADR-0056 の codex/acp 同様の配送を Pi に足すかは pi-adapter 葉の範囲外）
  - context file（AGENTS.md / CLAUDE.md）は読ませる（`--no-context-files` は付けない。リポジトリの規約を Pi にも効かせる）
  - `--offline` は付けない（推論に network が要る）。起動時の更新確認を止める env があれば付ける（未確認。pi-adapter 葉で `--help` の env 一覧から決める）
- 資格情報: pi-ai には `anthropic`・`openai-codex`・`opencode-go`・`opencode` 等の provider がある（同梱 pi-ai の `models.generated.js` で確認）。資格情報は `PI_CODING_AGENT_DIR/auth.json`（docs/sdk.md:414-421）か env（`OPENCODE_API_KEY`、`ANTHROPIC_OAUTH_TOKEN` 等）。**Celeris の account dir（`AccountAdapter::OpencodeGo` の `opencode/auth.json`、`Codex` の `auth.json`）を Pi の auth.json に写す形式は未確認。** 第 1 段は env で鍵を渡せる `opencode-go` pool と、cheap の Qwen（`openai_compatible`。Pi の `models.json` の custom provider）に限る。codex（ChatGPT subscription の OAuth）を Pi で使うのは、auth の受け渡しを確かめてからの別葉にする（それまで codex 行は fallback で今どおり走る）。
- self-host の Qwen: Pi の `models.json`（`PI_CODING_AGENT_DIR/models.json`）に OpenAI 互換の custom provider として base URL と model を書く。PiAdapter が run ごとに生成する（行の `base_url`・`model` から）。形式の細部は未確認（pi-adapter 葉で同梱 docs/models.md 相当を読んで決める）。

### Hashline の読み込み

- **host に無く、仕様は未確認。** 理解（推測）: hash を付けた行参照で編集する Pi の extension で、`.ts` か `index.ts` を持つディレクトリとして配布される。
- このため PiAdapter は Hashline を **config で受ける**: provider 行（adapter = `pi`）に `extensions = ["<path>"]` と `tools = ["read", "bash", …]` を書き、PiAdapter は `--no-extensions` の後に各 path を `-e` で渡す。path が存在しなければ run を始めずに設定誤りとして失敗させる（黙って Hashline 無しで走らせない）。
- Hashline の tool 名は config の `tools` に人が書く。既定値は置かない（名前が未確認のため）。
- extension の自動発見（`~/.pi/agent/extensions`・project の `.pi/extensions`）は `--no-extensions` で止める。リポジトリ側に `.pi/extensions` があっても読まない。

### tool の絞り方

- `--tools <names>` は built-in・extension・custom の全 tool に効く allowlist（`--help` で確認）。PiAdapter は常に `--tools` を付け、値は行の `tools`。行に無ければ既定 `read,bash,edit,write,grep,find,ls`（built-in。Hashline を使う行は edit/write を外して Hashline の tool に替えるのを人が config で選ぶ）。
- `--no-builtin-tools` は使わない（allowlist で足りる）。

### subagent の無効化

- Pi の core に subagent は無い（…/README.md:476「No sub-agents.」）。subagent は extension（examples/extensions/subagent）で足すもの。
- `--no-extensions` と、`-e` には config に書いた path だけ、`--tools` allowlist の 3 つで subagent tool は届かない。worker が別の LLM CLI を起こす経路を作らない方針（worker は subagent を作らない）と合う。PiAdapter の試験で、引数列に `--no-extensions` と `--tools` が必ず入ることを固定する。

### 未確認点（pi-adapter 葉の前に人か葉が確かめる）

1. Hashline の配布形態・path・tool 名・version（host に無い）。
2. 本番 host での pi の置き場（openclaw 同梱の `dist/cli.js` を `node` で呼ぶか、別に導入するか）。PiAdapter は `command`（既定 `pi`）を行ごとに上書きできるようにし、どちらでも動くようにする。人の決定事項。
3. `--mode json` の終端 event と usage（token・cost）の取り方。同梱 docs/json.md の AgentEvent 定義を pi-adapter 葉で fixture にする。
4. codex の OAuth を Pi に渡す方法、opencode go の鍵を env で渡すときの名前（help の `OPENCODE_API_KEY` で足りるか）。
5. 起動時の更新確認・telemetry を止める env。

## fallback・account pool・cheap lane

- **retry / cooldown**: 変えない。`provider_failure_outcome`（provider_select.rs:73）の Throttled / AuthFailed / Exhausted と `StaticPolicy::report`（policy.rs:195-217）の provider 単位 cooldown はそのまま。PiAdapter は失敗を同じ `AdapterError` の分類で返す（429 `GoUsageLimitError` → Throttled 等。ADR 2026-10-06 の一次情報）。
- **adapter 間の fallback**: preferred が全滅したら絞る前の候補へ倒れる（決定点）。つまり Pi 行がすべて cooldown・枠切れなら、非 Claude の task は今どおり codex / acp 行で走り、Claude 行が全滅なら Pi 行に倒れることもある。どちらも routing audit に `adapter_fallback` の理由を残す。倒れたこと自体は失敗ではない。
- **run 間 escalation**（ADR 2026-10-04 §5、lane を上げる）: lane が変わると候補行が変わり、新しい lane の行で family と preferred をやり直す。adapter を固定して持ち越さない（明示 adapter のある task は今どおり同じ adapter）。
- **harness fallback**（`HarnessFallback`、ADR-0052）: knowledge だけのまま。coding harness には足さない。
- **account pool**: 行の pool（`AccountPoolSetting::pool_adapter`、accounts.rs:96-102）は family 導出の入力（2 番目）として使うだけで、pool の採点（残量 score）は preferred の中で今どおり。Pi 行は `account_pool = "opencode-go"` を名前で指せる（ADR 2026-10-06 D2/D3 の ACP 行と同じ形）。新しい `AccountAdapter::Pi` は作らない（Pi は harness であって subscription ではない）。
- **cheap lane の Qwen**: cheap lane の local 行優先（ADR-0132 付記 L1-L3、provider_select.rs:366-423）は preferred の中で効く。Qwen を ACP 直結で持つ行と Pi 行の両方があれば Pi 行が preferred。Pi の Qwen 行が無ければ従来の ACP 行が fallback で走る。llm-proxy 経由（`celeris` source）の cheap は family Unknown → Pi が既定。proxy 内で Qwen が落ちて Claude / GPT に倒れても（ADR-0132 D3）同じ run の adapter は変わらない（今と同じ）。Qwen は cheap だけ（model_routing.rs:725-734 の検査）という制約も変えない。

## metrics 整合

- **routing audit（run 単位）は今のままで (model, harness) を比較できる。** `RoutingAudit`（crates/task-core/src/routing_audit.rs:40-58）は `adapter`（`WorkerStarted.adapter`）と `model`（`RoutingDecided.resolution.model_id` で上書き）を別欄で持つ。Pi の run は `adapter = "pi"` で出る。`harness` 欄は genre（`"coding"`）で adapter ではない点に注意（同じ `coding` 内の比較は `adapter` 欄で行う）。
- **足りない欄（dispatch 葉で足す）**:
  1. `LaneResolution`（crates/task-core/src/model_routing.rs:38-39 付近）に `family: Option<ModelFamily>`・`family_basis: Option<FamilyBasis>`・`adapter_choice: Option<AdapterChoice>` を足す。`AdapterChoice { Explicit, Preferred, Fallback { reason }, ProviderOrder }`。routing audit がこれを読んで出す。これで「既定解決で選ばれたか、倒れたか、明示か」が run ごとに分かる。
  2. proxy 経由の run の `model` は lane 抽象名（`celeris/<tier>`）で実 model ではない（ADR 2026-10-04 §2）。この ADR では直さない。(model, harness) の比較では proxy 経由の run を別群として扱う（ops-docs 葉で集計手順に書く）。
- **execution metrics（task 単位）**: `ExecutionMetrics`（crates/task-core/src/execution_metrics.rs:130-216）には adapter 欄も model 欄も無く、`GET /metrics/execution` の `group_by` は `gate_mode, genre, assignee, lane, depth` だけ（crates/task-api/src/stats.rs:300-301）。1 task に複数 run・複数 adapter があり得るので、task 単位の集計に adapter を足すと「最後の run の adapter」等の規約が要る。**この ADR では足さない。** (model, harness) の比較は routing audit で行い、task 単位の要望が出たら別 ADR で `group_by=adapter`（最後の実行 run の adapter で数える）を決める。

## 後続葉

各葉の試験は名前の接頭辞で選べるようにする（`cargo nextest run -E 'test(/^<接頭辞>/)'` 等）。

### family（task-core）
- 触る file: `crates/task-core/src/model_family.rs`（新規: `ModelFamily`・`FamilyBasis`・`FamilyDecision`・`derive_family`）、`crates/task-core/src/coding_harness.rs`（新規: `CodingHarness`・`AdapterPolicy`・`AdapterChoice`・`prefers`）、`crates/task-core/src/lib.rs`（re-export）、`crates/task-core/src/harness.rs`（`HarnessSpec.adapter_policy`）、`crates/celeris/src/config/model_routing.rs`（Qwen 判定を `ModelFamily::parse` へ）、`crates/celeris/src/config/harness.rs`（`adapter_policy` の読み込み・検証）。
- 試験名接頭辞: **`coding_harness_default`**（task-core。例 `coding_harness_default_claude_oauth_is_claude`、`coding_harness_default_unknown_is_non_claude`、`coding_harness_default_opencode_go_uses_catalog_family`、`coding_harness_default_proxy_is_non_claude`、`coding_harness_default_parse_is_case_insensitive`）。
- schema 再生成（`HarnessSpec` が API に出るなら UPDATE_SCHEMA=1・gui/web の型生成）。

### pi-adapter（task-worker・celeris）
- 触る file: `crates/task-worker/src/pi.rs`（新規 `PiAdapter`、`ID = "pi"`）、`crates/task-worker/src/lib.rs`、`crates/celeris/src/daemon/adapters.rs`（`build_adapters` の match に `pi`、adapters.rs:24, 44, 87 付近）、`crates/celeris/src/config/providers.rs`（adapter id の許可一覧 providers.rs:470-481 に `pi`、`extensions`・`tools`・`command` を pi 行に許す。providers.rs:492-500 の拒否規則の調整）、`crates/celeris/src/config/harness.rs`（`[[harnesses]]` harness.rs:563-576 と `[[roles]]` harness.rs:607-615 の許可一覧に `pi`）。
- 試験名接頭辞: **`pi_adapter`**（task-worker。偽の `pi` 実行ファイルは `crate::test_support::write_executable` で書く。例 `pi_adapter_args_include_no_extensions_and_tools`、`pi_adapter_missing_extension_path_fails_before_spawn`、`pi_adapter_parses_json_events_to_result`、`pi_adapter_usage_limit_is_throttled`、`pi_adapter_id_matches_coding_harness`）。実 Pi・外部ネットワークは使わない。
- session resume（sessions.rs の `SUPPORTED_ADAPTERS`）はこの葉に入れない。

### dispatch（task-dispatch）
- 触る file: `crates/task-dispatch/src/dispatcher/provider_select.rs`（family の導出を ModelProfile/DeploymentProfile 生成箇所 136-171 に、preferred の絞り込みと fallback を `select_provider_excluding` に）、`crates/task-dispatch/src/dispatcher/dispatch_run.rs`（`LaneResolution` の `family`・`adapter_choice` を埋める、631-634 付近）、`crates/task-dispatch/src/sessions.rs`（sticky を preferred の中に限る）、`crates/task-core/src/model_routing.rs`（`LaneResolution` の欄）、`crates/task-core/src/routing_audit.rs`（新欄の出力）。
- 試験名接頭辞: **`coding_harness_default`**（task-dispatch。例 `coding_harness_default_explicit_adapter_wins`、`coding_harness_default_claude_row_prefers_claude_code`、`coding_harness_default_non_claude_prefers_pi`、`coding_harness_default_falls_back_when_pi_cooled_down`、`coding_harness_default_provider_order_policy_unchanged`、`coding_harness_default_cheap_local_qwen_pi_first`）。
- 時間依存は `tokio::time::pause` / 注入した時計で決める。

### ops-docs（文書）
- 触る file: `docs/ops/coding-harness-pi.md`（新規: pi の置き場・Hashline の導入・provider 行と `[[harnesses]] adapter_policy` の config 例・有効化と戻し方・routing audit での (model, adapter) の比較手順・proxy 経由 run の扱い）、`config/celeris.research.example.toml`（coding harness と pi 行の例、コメント）、`docs/architecture-map.md`（coding harness の決定点の索引）、この ADR の付記（実装との突き合わせ）。
- 試験: 文書検査（`sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`）と、config 例が読めることの既存試験。
- 本番 config の変更・pi の導入は人の手順として書く（この葉では行わない）。

順序: family → pi-adapter と dispatch（並行可。dispatch は `"pi"` 行が無くても fallback で今どおり動く）→ ops-docs。

## Pi/Hashline の host 上の有無

2026-10-07、worker の host で読み取りだけで確認（install・外部ネットワークなし。`pi --help` は `HOME=/tmp/pi-help-home` で実行）。詳細は survey の表。

- `command -v pi` / `command -v hashline` / `command -v pi-coding-agent`: いずれも出力なし → **pi・Hashline とも PATH に未導入**。
- `~/.pi`・`~/.pi/agent/extensions`・`~/.config/pi`: 無し → **Pi の extension（Hashline を含む）は未導入**。
- openclaw 2026.5.20 の依存に `@earendil-works/pi-coding-agent` 0.75.4 が同梱（`bin: pi → dist/cli.js`）。`node …/dist/cli.js --version` → `0.75.4`、`--help` exit 0。この ADR の起動形・tool・subagent の記述はこの同梱版で確かめた。
- Hashline: 同梱物（openclaw/dist、@earendil-works/*、pi-coding-agent の README/docs/CHANGELOG/examples）を `grep -ril hashline` して一致なし → **host のどこにも無く、仕様は未確認**。
- ACP: pi-coding-agent/dist に ACP の実装は無い。
- 参考: claude・codex は `~/.local/bin` に有り、opencode 1.18.35 は `~/.opencode/bin`（PATH 外）。

## 結果

- 明示 adapter のある task・`adapter_policy` を書いていない harness は挙動が変わらない。
- 有効化すると、非 Claude（未知を含む）の coding run は Pi 行があれば Pi で走り、Claude の run は claude-code 行で走る。codex 行・ACP 行は preferred でなくなり、fallback でのみ使われる。codex の account pool の消費が減ることは有効化前に人が了承する。
- 戻し方: config の `adapter_policy = "model_family"` を消す（または `provider_order`）。データの移行は無い。

## 付記（実装との突き合わせ、2026-10-07）

leaf family / pi-adapter / dispatch / ops-docs の統合後（branch tip `a2ca31ad`）に実装と突き合わせた記録。進捗は `agent-docs/progress/2026-10-07-coding-harness-default-pi-hashline.md`。

### 関数名と位置（「解決する関数と位置」・「Claude family 判定」節の actual）

- `crates/task-core/src/model_family.rs`（新規）: `ModelFamily { Claude, Gpt, Qwen, Other, Unknown }`・`ModelFamily::parse`・`is_claude`、`FamilyBasis { LlmSource, AccountPool, ModelProfile, Unknown }`、`FamilyDecision`、`derive_family(source: &LlmSourceRef, pool: Option<AccountAdapter>, model: Option<&ModelProfile>) -> FamilyDecision`。
  - 本文の `derive_family(source, pool, model, deployment)` の **`deployment` 引数は置かない**。`DeploymentProfile` は family の材料を持たず（`source_ref` の文字列も判定に使わないと本文が定めているため）。`model` は `Option`（catalog に行が無い provider でも呼べる）。
  - 導出の優先順位は本文どおり（source → pool → catalog の `ModelProfile.family` → `Unknown`）。
- `crates/task-core/src/coding_harness.rs`（新規）: `CodingHarness { ClaudeCode, Pi }`・`CodingHarness::for_family`・`CodingHarness::adapter_id`（`ClaudeCode` は `AccountAdapter::ClaudeCode.as_str()`、`Pi` は定数 `PI_ADAPTER_ID = "pi"`）、`prefers(row_adapter, family)`、`coding_harness_default_adapter(explicit: Option<&str>, family) -> &str`（明示 adapter があればそのまま返す）、`AdapterPolicy { ProviderOrder（既定）, ModelFamily }`、`AdapterChoice { Explicit, Preferred, Fallback { reason }, ProviderOrder }`。
- `crates/task-core/src/harness.rs`: `HarnessSpec.adapter_policy: AdapterPolicy`（既定 `ProviderOrder`、serde で `provider_order` 時は省略）。
- task-dispatch: 本文は `select_provider_excluding`（provider_select.rs）の冒頭に絞る処理を書く設計だった。**実際は `crates/task-dispatch/src/dispatcher/coding_default.rs`（新規 module）に `Dispatcher::select_provider_with_default(active, hint, …)`** として重ねた。`active = false`（対象外の run）なら従来どおり `select_provider_excluding` をそのまま呼ぶ。絞りは「`prefers` に合わない行を excluded 集合へ足して `select_provider_excluding` を呼ぶ」で、preferred の中の順序（sticky → cheap local → pool score → 設定順）は `select_provider_excluding` のまま。preferred が全滅したら除外なしで同じ関数をやり直す（fallback）。`select_provider_excluding` 本体は 3 行（catalog の family を `ModelProfile.family` に入れる処理）しか変えていない。
- 行の family: `Dispatcher::row_family(&LegacyProfile)`（LLM source は `Config::provider_llm_source`、pool の adapter は pool を使う行だけ、catalog の family は `CodingHarnessDefault.model_families`）。`legacy_provider_profiles` が作る `ModelProfile.family` に catalog の値を入れる（catalog に無い model は空文字 → `Unknown`）。
- 適用の条件（dispatch_run.rs）: `coding_default_harness = !cos && !is_planner_dispatch && genre ∈ coding_default.harnesses`、`coding_default_active = coding_default_harness && worker_hint.adapter.is_none()`。
- daemon 配線: `Config::coding_harness_default()`（crates/celeris/src/config/dispatch.rs）が `CodingHarnessDefault { harnesses, sources, model_families }` を作り、`Dispatcher::set_coding_harness_default` で渡す（`crates/celeris/src/daemon/bootstrap.rs` の起動時と `daemon/admin.rs` の設定の再読込）。
- sticky: `SUPPORTED_ADAPTERS`（sessions.rs:12）は `["claude-code", "codex", "acp"]` のまま（`pi` は未追加、本文どおり session resume 未実装）。worker run の sticky session は CoS の対話 run のみ（`cos_conversation_session`）なので、preferred 絞りとの実効的な交わりは無い（CoS は既定解決の対象外）。
- Qwen 判定（crates/celeris/src/config/model_routing.rs:724-728）は **置換していない**（本文「残すなら判断」の分岐: `contains("qwen")` の legacy 判定を残す。`ModelFamily::parse` への置き換えは未実施・未必要 — 制約は catalog の family 以外の id/source 名にも掛かるため）。

### adapter id と config 欄

- adapter id: `PiAdapter::ID = "pi"`（crates/task-worker/src/pi.rs:92）。一致を固定する試験は task-dispatch の `coding_harness_default_pi_adapter_id_matches_worker`。
- provider 行の欄: `command` / `args`（実行ファイル prefix だけ、`-`・`@` 始まりは config 検証で拒否）/ `extensions`（Hashline の path）/ `tools`（allowlist）/ `model`（`provider/id`）。pi 行の `extensions` と `tools` は必須、他の adapter 行では拒否（`crates/celeris/src/config/providers.rs`、試験 `pi_adapter_config_rejects_missing_hashline_and_tools_on_other_adapters`）。
- `build_adapters`（crates/celeris/src/daemon/adapters.rs）: `PiAdapter::ID` の match arm。`base_url` は行の LLM source から決める（`Celeris` → llm-proxy の listen + `CELERIS_PI_API_KEY`、`OpenaiCompatible(id)` → その source の `base_url` と `api_key`、その他 → 行の env の `OPENAI_BASE_URL`/`OPENAI_API_BASE`）。
- `[[harnesses]]` の `adapter_policy = "model_family"` は固定 `adapter` と `conversation = true` との併用を config 検証で拒否（`crates/celeris/src/config/harness.rs`）。

### Pi 起動形（「Pi 起動形と tool 集合」節の actual）

- 起動引数（PiAdapter が組み立てる固定部分、crates/task-worker/src/pi.rs:222-241）: `--mode json -p`（prompt は **stdin**、argv 上限に当たらない）`--no-extensions --no-skills --no-prompt-templates --no-themes --session-dir <run>/pi-sessions --provider <p> --model <p>/<id> --tools <allowlist>`、各 extension を `-e <path>`。本文の「context file は読ませる」はそのまま（`--no-context-files` は付けない）。
- `PI_CODING_AGENT_DIR=<run>/pi-agent`、そこに `settings.json`（Pi 内 retry 無効 + install telemetry 無効）と、必要なら `models.json`（openai 互換の custom provider。鍵と llm-proxy のルーティング文脈 header は env 参照 `CELERIS_PI_API_KEY` / `CELERIS_PI_ROUTING_CONTEXT` で渡す）。
- opencode-go pool: 選ばれた account dir（`XDG_DATA_HOME`）の `opencode/auth.json` から鍵を読んで `OPENCODE_API_KEY` として渡す（auth.json を写さない。`crates/task-worker/src/opencode_account.rs::read_go_key`）。
- **未確認点の消えたもの**: 1（Hashline の path・tool 名は config で人が書く → 導入手順書 `docs/ops/coding-harness-pi-hashline.md`）、2（`command`/`args` で行ごとに上書き可能 → openclaw 同梱の `node …/cli.js` でも npm prefix 版でも動く）、3（終端 event = `agent_end`、usage = assistant の `message_end` の `usage.{input,output,cacheRead,cacheWrite,cost.total}` を `Usage` に合算。fixture は `crates/task-worker/src/pi/tests.rs`）、4（codex の OAuth は **Pi に渡さない**。config 読み込みで pi 行の codex pool は拒否 → codex 行は fallback で今どおり。opencode-go の鍵は `OPENCODE_API_KEY`）、5（telemetry/install 確認は `PI_CODING_AGENT_DIR` の `settings.json` で止める）。
- subagent/planner: `PiConfig::validate` が `tools` に subagent/planner を含む名前で拒否する（試験 `pi_adapter_requires_hashline_tools_and_rejects_subagent_planner_or_cli_overrides`）。`--no-extensions` + `-e` だけ + allowlist の 3 つで subagent 経路を作らない（本文どおり）。

### metrics（「metrics 整合」節の actual）

- `LaneResolution`（crates/task-core/src/model_routing.rs）に **`coding_default: Option<CodingDefaultResolution { family, family_basis, adapter_choice }>`** を足した（本文の 3 個の個別 Option 欄ではなく 1 つの struct 欄。`adapter`・`model_id` は既存の欄のままで、本文どおり別欄で (model, harness) が比較できる）。
- `RoutingAudit.coding_default`（crates/task-core/src/routing_audit.rs）に写す。対象外の run・旧イベントには `None`。
- execution metrics（task 単位）には足していない（本文どおり）。
- schema: `UPDATE_SCHEMA=1` で `docs/api/v1`（api-v1・event の 2 file）を再生成し、`gui/app/celeris/types.ts`（gui の `pnpm gen:types`）と `web/api/generated/`（`node web/scripts/gen-types.mjs`）を反映済み。

### 試験名（「後続葉」節の actual。`cargo nextest run -E 'test(/<接頭辞>/)'` で選べる）

- task-core（`coding_harness_default`、crates/task-core/src/coding_harness/tests.rs、10）: `claude_uses_claude_code`、`gpt_openai_uses_pi`、`opencode_go_uses_pi`（catalog の family が Claude なら `claude-code` に戻るケースも同じ試験内）、`deepseek_uses_pi`、`unknown_provider_uses_pi`、`respects_explicit_adapter`、`parse_is_case_insensitive`、`source_wins_over_pool_and_profile`、`prefers_matches_family_harness`、`serde_names`。必須 6 ケース（Claude / GPT・OpenAI / OpenCode Go / DeepSeek / 未知 / 明示指定）は全てここ。
- task-worker（`pi_adapter`、crates/task-worker/src/pi/tests.rs、14）: 起動引数・tool 集合・cwd・隔離（`args_tools_isolation_cwd_and_large_prompt`）、usage 合算、extension 欠落で spawn 前失敗、subagent/planner 拒否、失敗分類（`failure_classification_and_json_error_on_zero_exit`・`stderr_failure_classification`）、`agent_end` 無し・stale result 拒否、custom models・context header、result の question/yield、go account の鍵、skills 配送、wall/idle timeout、extension 読み込み失敗。
- task-dispatch（`coding_harness_default`、crates/task-dispatch/src/dispatcher/tests/coding_harness_default.rs、12）: `claude_row_prefers_claude_code`、`gpt_openai_prefers_pi`、`opencode_go_prefers_pi`、`deepseek_prefers_pi`、`unknown_provider_prefers_pi`、`falls_back_when_pi_unavailable`（満杯・cooldown・pi 行なしの 3 種）、`provider_order_policy_unchanged`、`cheap_local_qwen_pi_first`、`pi_adapter_id_matches_worker`、`records_adapter_in_metrics`（tick 実行 → `WorkerStarted.adapter == "pi"` → routing audit）、`explicit_adapter_wins`、`other_harness_unchanged`。
- celeris（`coding_harness_default` 2 + `pi_adapter` 3）: `coding_harness_default_config_builds_dispatcher_inputs`・`coding_harness_default_config_rejects_fixed_adapter_and_unknown_policy`、`pi_adapter_provider_config_loads_and_builds_with_relative_extension`・`pi_adapter_config_rejects_missing_hashline_and_tools_on_other_adapters`・`pi_adapter_config_preserves_go_pool_and_direct_compatible_source`。

### 検証（close-out、統合後の branch tip `d0bf8b63` で再実行、2026-10-07）

- `corepack pnpm install --offline` → exit 0（pnpm v12.6.0）。
- `bash scripts/dev/test-parallel.sh` → exit 0（nextest 4225 passed / 0 failed / 13 skipped + doctest。CELERIS_TEST_SUMMARY は ignored 14）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all --check` → 差分なし。
- web: `corepack pnpm@12.6.0 -C web typecheck` → exit 0。gui: `corepack pnpm@11.27.0 typecheck`（gui を cwd に）→ exit 0。生成型は optional 欄の追加（`coding_default`）だけ。
- `git diff --check` と `git diff --quiet "$CELERIS_WU_BASE" -- crates/ gui/` → exit 0。
- 本番の導入・有効化・実 Pi/実 LLM での確認は人の手順（`docs/ops/coding-harness-pi-hashline.md`）。
