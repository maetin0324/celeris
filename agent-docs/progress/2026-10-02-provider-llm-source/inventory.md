---
tasks: [01M3YF3NSR46FM314VHG14T3N6]
---
# provider / LLM source の棚卸し（Qwen 前提と providers の性質）

調査日 2026-10-02、base `7b77f17a`。コードは変えていない。行番号はこの base のもの。
処置案は 3 つのどれか: **消す**（Qwen 前提の制約・説明を取る）/ **改名**（名前・文言を抽象に揃える）/
**互換で残す**（旧設定・旧値を読めるまま残し、移行を ADR に書く）。後の葉の担当を「→ 葉」で示す。

## (a) Qwen 専用・ローカル Qwen 必須を前提にした名前・コメント・制約・probe

### 知識整理（langmem）経路と ADR-0052 の fallback

| path:line | 現状 | 処置案 |
|---|---|---|
| crates/task-worker/src/probe.rs:3-5 | 「知識整理 run の LLM は pegasus トンネル越しの Qwen で、トンネルが落ちている間は run が必ず落ちる」 | **改名**: 「`[knowledge.langmem].base_url` の OpenAI 互換口（既定は proxy の `celeris/cheap`）」に。probe 自体は proxy 直結でない直指定構成のため**互換で残す** → knowledge-route |
| crates/task-dispatch/src/dispatcher.rs:1937-1953 `knowledge_fallback_reason` | langmem の base_url に届かなければ cheap の汎用ハーネスへ倒す | **互換で残す**（直指定構成用）。proxy に向けた構成では proxy が倒れ先を持つので probe は proxy の `/v1/models` を見るだけ、と注記 → knowledge-route |
| crates/task-dispatch/src/dispatcher.rs:1352-1354 | 既定 probe = `task_worker::probe_models` | 変更なし（**互換で残す**） |
| crates/task-core/src/harness.rs:224 | 「Qwen（`langmem`）に届かなければ tier cheap の汎用ハーネスへ倒す」 | **改名**: 「langmem の接続先に届かなければ」 → knowledge-route |
| crates/task-worker/src/langmem.rs:84 | fallback 依頼文「いつもの `langmem` の接続先（Qwen）に届かなかったので」 | **消す**「（Qwen）」 → worker-tools |
| crates/task-worker/src/langmem_run.py:115 | 「production local Qwen OpenAI-compatible endpoint」 | **改名**: proxy（`celeris/cheap`）を既定の接続先として書く → worker-tools |
| crates/celeris/src/config/knowledge.rs:66-71 | doc 例が `base_url = "http://bnode150:18000/v1"`、`model = "qwen3.8-27b"` | **改名**: `http://127.0.0.1:18100/v1` + `celeris/cheap` → knowledge-route |
| crates/task-core/src/knowledge_run.rs:67-68、crates/task-api/src/types.rs:196-197 / 1864-1865、gui/app/celeris/types.ts:231-232 / 1314-1315、docs/api/v1/api-v1.schema.json:3987 / 19886、gui/app/lib/knowledge.ts:265-266、docs/gui/api.md:2018-2019 | `via = "langmem"`（Qwen）/`"fallback:<adapter>"`（Qwen に届かず） | 値は**互換で残す**（DB・API の既存値）。説明は「langmem の接続先」に**改名**（schema は UPDATE_SCHEMA=1 で再生成、gui は `pnpm gen:types`） → knowledge-route |
| crates/task-core/migrations/0021_knowledge_run_retry.sql:10-11 | 同上のコメント | **互換で残す**（適用済み migration は書き換えない） |
| gui/app/components/ConsoleBlockItem.tsx:935、gui/app/components/task-detail/TimelineTab.tsx:452 | コメント「Qwen に届かず」 | **改名**（表示文言「cheap のハーネスで抽出」は Qwen に触れないので残す） → gui-ui |
| crates/celeris/src/daemon/tick_loop.rs:239 | 「2 回目が Qwen で走るか cheap の汎用ハーネスで走るか」 | **改名** → knowledge-route |
| config/celeris.example.toml:292-294 | 「専用アダプタ（`langmem` = Qwen）に届かないとき」「Qwen が落ちている間の知識整理 run は失敗する」 | **改名** → config-docs |
| config/celeris.example.toml:155-159 | `[knowledge.langmem]` の欄説明が「ローカルの Qwen」「本番の Qwen トンネル」「例 qwen3.8-27b」 | **改名**: 既定例を proxy + `celeris/cheap` に → config-docs |
| docs/adr/0052-knowledge-run-fallback.md:1,12,15,43 | 題名「Qwen が使えなければ…」、専用契約 = Qwen | ADR 本文は**互換で残す**（履歴）。ADR-0131 から「Qwen 専用契約ではない」と付記で上書き → adr |
| docs/knowledge.md:257,273-311 | 「7.1 Qwen が落ちているとき」「LLM は pegasus のトンネル越しの Qwen」 | **改名**: 「langmem の接続先が落ちているとき」、proxy 構成では proxy が倒す旨 → config-docs |

### PaperQA（paperqa-qwen）

| path:line | 現状 | 処置案 |
|---|---|---|
| config/paperqa.qwen-local.example.json:2,6,8,17,21,23,44,48,50 | ファイル名・全 llm が `openai/qwen3.8-27b` | **改名**: `config/paperqa.proxy.example.json`（llm = `openai/celeris/standard`、summary/agent も celeris/<tier>）。旧ファイルは移行 1 版の間**互換で残す**か削除し参照を更新 → config-docs |
| config/celeris.research.example.toml:12,24,68,69 | `paperqa.qwen-local.example.json` を参照、`settings = ".../settings/qwen-local"` | **改名**（新ファイル名・`settings/proxy`） → config-docs |
| config/celeris.research.example.toml:16,96,111-112 | 「ローカル Qwen のような遅い LLM」「Qwen をトンネル越しに使う構成では」「`openai/qwen3.8-27b` → `qwen3.8-27b`」 | **改名**: 「遅い OpenAI 互換 LLM」、例を `celeris/cheap` に → config-docs |
| config/celeris.research.example.toml:142 | `id = "paperqa-qwen"`（model は既に `openai/celeris/standard`、tiers = standard） | **改名**: `paperqa`（例: `paperqa-proxy`）。本番 id の変更は人の移行手順に書く（stats・providers.d の id が変わる） → config-docs |
| crates/task-worker/src/paperqa.rs:78,661 | 「実機で Qwen3 は」「settings の llm は openai/qwen3.8-27b」 | **互換で残す**（実機観測の注記。実装に Qwen 制約は無い）。661 は例を `celeris/standard` に**改名**してよい → worker-tools |
| crates/task-worker/src/paperqa_acquire.py:36,382,559 | INPUT 例 `model: "qwen3.8-27b"`、`base_url :18000`、`api_key "unused"`、reasoning model 注記 | 例を proxy 値に**改名**。reasoning（`reasoning` 欄・thinking 抑止）の処理は Qwen 以外にも効くので**互換で残す**（注記を「reasoning model（Qwen3 等）」に） → worker-tools |
| crates/task-worker/src/paperqa_ask.py:58 | `"model": "qwen3.8-27b" | null` | **改名**（例を `celeris/standard`） → worker-tools |
| crates/task-worker/src/paperqa/tests.rs（23 箇所） | 試験の値に qwen3.8-27b | **互換で残す**（値は任意文字列。新試験は celeris/<tier> を使う） |

### Local Deep Research（ldr-qwen）

| path:line | 現状 | 処置案 |
|---|---|---|
| config/celeris.web-research.example.toml:148 | `id = "ldr-qwen"`（model は既に `celeris/cheap`、tiers = cheap） | **改名**: `ldr`（例 `ldr-proxy`） → config-docs |
| config/celeris.web-research.example.toml:11,111-113 | 「LLM はトンネル越しのローカル Qwen 等に向ける」「直接 Qwen を叩いていた頃」 | **改名**（proxy の `celeris/cheap` が既定） → config-docs |
| crates/task-worker/src/local_deep_research/tests.rs（5 箇所） | 試験値 | **互換で残す** |

### opencode（opencode-qwen、acp）

| path:line | 現状 | 処置案 |
|---|---|---|
| config/celeris.acp-opencode.example.toml:11,40 | `--provider opencode-qwen`、`id = "opencode-qwen"`、tiers = standard, cheap（本番は frontier も） | **改名**: `opencode`。tiers は cheap だけ（Qwen 経路を cheap に限る方針 3） → config-docs |
| config/celeris.acp-opencode.example.toml:49-55 | 既定 `model = "qwen-local/qwen3.8-27b"`（直接 Qwen）、proxy はコメントで案内 | **消す**直 Qwen 既定 → `celeris-proxy/celeris/cheap` を既定にする。直 Qwen は「cheap だけ」のコメント例として**互換で残す** → config-docs |
| config/celeris.acp-opencode.example.toml:65 | `OPENCODE_CONFIG = ".../opencode/qwen.json"` | **改名**（`proxy.json`） → config-docs |
| crates/task-worker/src/acp.rs:366 | 実機観測（opencode + Qwen3.8-27B の細切れ件数） | **互換で残す**（観測の記録） |
| crates/task-worker/src/acp/tests.rs（2 箇所） | 試験値 | **互換で残す** |
| docs/llm-source.md:183,219-221 | 「opencode-qwen の既定はまだ直接 Qwen を指すまま」 | **改名**（proxy 既定・確認手順） → config-docs |

### llm_proxy / LLM source の説明

| path:line | 現状 | 処置案 |
|---|---|---|
| crates/llm-proxy/src/naming.rs:1,13-14,22,37,66,129 | `SourceKind::Qwen`（= `openai-compatible`）、接頭辞 `qwen/<tier>` | `qwen/` 接頭辞は**互換で残す**（既存の明示指定を壊さない）。enum は `OpenAiCompatible` へ**改名**を検討（ラベル `"qwen"` は維持）。`qwen/frontier`・`qwen/standard` は (c) の方針で 422 → proxy-cheap |
| crates/llm-proxy/src/config.rs:97,177,181 | 「`qwen/<tier>` は 422」「既存の Qwen 等をそのまま中継」 | **改名**（「OpenAI 互換の中継（例 Qwen）」程度に） → proxy-cheap |
| crates/llm-proxy/src/sources/relay.rs:1、selection.rs:5,151 | 同上 | **改名** → proxy-cheap |
| crates/celerisctl/src/commands/routing.rs:6,112,129-141,192 | `celerisctl routing` の表の列 `qwen` | **互換で残す**（列名は models.qwen と対応）。frontier/standard は空欄（`-`）になることを試験で固定 → proxy-cheap |
| crates/task-core/src/quota.rs:7,56,310、crates/task-dispatch/src/dispatcher/quota_book.rs:138、docs/api/v1/api-v1.schema.json:14647 | `free` 窓 =「Qwen など」 | **互換で残す**（例示にすぎない） |
| crates/task-dispatch/src/dispatcher/cluster.rs:26、crates/celeris/src/config/cluster.rs:59、crates/celeris/src/daemon/bootstrap.rs:166、crates/task-api/src/types.rs:487、gui/app/celeris/types.ts:2469、docs/api/v1/api-v1.schema.json:2551 | `[[clusters.forwards]]`（Qwen トンネル等） | **互換で残す**（forward は汎用、Qwen は例） |
| crates/celeris/src/daemon/clusters.rs:324-325 | トンネル先 probe は Qwen 直なので bearer 不要 | **互換で残す** |
| gui/app/routes/clusters.tsx:334 | 「このトンネルは Qwen（無料の LLM source）への接続です」 | **改名**: 「OpenAI 互換の LLM source（cheap 専用の Qwen）への接続」 → gui-ui |
| config/celeris.example.toml:182-184,202-204 | proxy の説明、`[[llm_proxy.sources.openai_compatible]] id = "qwen"` | **互換で残す**（id `qwen` は source の id として妥当）。説明に「cheap 専用」を足す → config-docs |
| docs/llm-source.md:5,15,17,24,57,128,165,180-185,243 | 「Qwen は tier に関わらず qwen3.8-27b」、`paperqa-qwen`/`ldr-qwen`/`opencode-qwen` の表 | **改名**: 24 行目を「Qwen は cheap だけ」に、表の id を新名に、旧 id は移行表へ → config-docs |
| docs/adr/0053-llm-source-proxy.md:20-21,32,47-48 | D1「Qwen は qwen3.8-27b」、選択 (a) 無料の qwen を最優先 | ADR 本文は**互換で残す**。ADR-0131（ADR-0053 付記）で「Qwen は cheap だけ」と上書き → adr |
| docs/adr/0026,0027,0029,0035 の Qwen 記述 | 当時の実機構成 | **互換で残す**（履歴。必要なら ADR-0131 から参照） |

## (b) [[providers]] の各欄: LLM source に属するもの / adapter・harness に属するもの

`[[providers]]` の 1 行は「どの道具（adapter）を、どの tier の仕事に、何本まで、どのモデル・資格情報で走らせるか」を
1 行に混ぜている。claude-pool・codex-pool は資格情報（LLM source 側）を account_pool で持ち、他の行（opencode・paperqa・
ldr・langmem）は model 欄の文字列（`celeris/<tier>` など）か env/settings の中で間接的に LLM source を指している。

| 欄（定義 path:line） | 属する側 | 説明 | 処置案 |
|---|---|---|---|
| `id` crates/celeris/src/config/providers.rs:19 | 両方（行の識別子） | stats・cooldown・providers.d のファイル名の鍵 | **互換で残す**。`-qwen` の付く例 id だけ**改名**（(a) 参照） |
| `adapter` providers.rs:20 | adapter/harness | 道具の種類（claude-code / codex / acp / paperqa / local-deep-research / langmem / fake） | **互換で残す**。API・GUI で「道具」と表示 |
| `tiers` providers.rs:22 | adapter/harness（どの lane の仕事を受けるか） | ルーティングの候補条件 | **互換で残す** |
| `concurrency` providers.rs:24 | adapter/harness（同時に走る run 数） | process の枠 | **互換で残す** |
| `model` providers.rs:27 | LLM source | 具体モデル名か proxy の抽象名（`celeris/cheap` 等）。acp は `qwen-local/qwen3.8-27b` のように source を直に固定しうる | **互換で残す**＋新欄 `llm_source`（例 `"celeris-proxy"` / `"claude-oauth"` / `"codex-oauth"` / `"direct"`）を足すか、API で `model` から導出した `llm_source` を返す → provider-split |
| `tier_models` providers.rs:16、検証 providers.rs:159-163 | LLM source（tier → モデル） | claude-code / codex だけ許可。llm_proxy.models と同じ概念が別の場所にある | **互換で残す**。LLM source の画面に「claude / codex の tier 対応」としてまとめて見せる → provider-split / web-ui |
| `account_pool` providers.rs:39 | LLM source（資格情報の供給） | `[accounts]` の pool から選ぶ | **互換で残す**（LLM source = claude_oauth/codex_oauth と同じ accounts_dir を指すことを明示） |
| `account_id` providers.rs:18 | LLM source | pool の中の固定アカウント | **互換で残す** |
| `env` providers.rs:31 | 混在 | `CLAUDE_CONFIG_DIR`/`CODEX_HOME`（source の資格情報）と `OPENCODE_CONFIG`（道具の設定。その中で source を固定）と `OPENCODE_DISABLE_PROJECT_CONFIG`（道具） | **互換で残す**。API は env_keys を source 系 / 道具系に分類して返す案 → provider-split |
| `env_from_secrets` providers.rs:35 | LLM source（主に API キー） | doc 上は「管理 API は読み書きしない」（crates/task-api/src/admin.rs:224-229）が、実際は `credential_refs` が create（admin.rs:310-312）・patch（admin.rs:348-352）でここへ書かれ、view は admin.rs:253 で逆に導出する | **互換で残す** |
| `command` / `args` providers.rs:43,46 | adapter/harness | acp のみ | **互換で残す** |
| `settings` providers.rs:51 | adapter/harness（ただし中身の JSON に LLM 名を持つ） | paperqa のみ。例ファイルが Qwen 固定 | **互換で残す**。例ファイルを proxy 版に**改名**（(a)） |
| `credential_refs`（API のみ）crates/task-api/src/types.rs:306-307、admin.rs:279,328 | LLM source | 資格情報の参照（実体は `env_from_secrets`） | **互換で残す**。doc の食い違いを provider-split で直す |
| （`[knowledge.langmem]` provider/base_url/model/api_key_secret）crates/celeris/src/config/knowledge.rs:73-90 | LLM source | langmem の LLM 接続先は providers 行ではなくここにある。`[[providers]] adapter = "langmem"`（config/celeris.example.toml:484-488）は道具の行 | **互換で残す**。設定例で `base_url` = proxy、`model = celeris/cheap` を既定に → knowledge-route / config-docs |
| `[llm_proxy.sources.*]` / `[llm_proxy.models.*]` crates/llm-proxy/src/config.rs:85-99,197-205 | LLM source（正本） | claude_oauth・codex_oauth・openai_compatible | ここを LLM source の正本とする（ADR-0053 と同じ）。新しい設定の表は増やさない案を ADR-0131 で決める → adr |

## (c) llm_proxy の models.qwen と prefer_free の選択経路

| path:line | 現状 | 問題 | 処置案 |
|---|---|---|---|
| crates/llm-proxy/src/config.rs:240-248 `default_qwen_models` | frontier・standard・cheap の全部を `qwen3.8-27b` | 既定で frontier/standard が Qwen に倒れうる | **消す** frontier/standard。既定は `{cheap: qwen3.8-27b}` だけ → proxy-cheap |
| crates/llm-proxy/src/config.rs:203-204 `ModelsConfig.qwen` | 利用者が `[llm_proxy.models.qwen] frontier = …` を書けば効く | 設定で方針 3 を破れる | 読み込み時に cheap 以外を**消す**（警告して無視）か設定エラー。旧設定を読めるよう「無視＋警告」を推す（**互換で残す**） → proxy-cheap / adr |
| crates/llm-proxy/src/config.rs:22-23,73 `prefer_free`（既定 true） | 到達可能な無料 source を最優先 | 意味は残してよいが、cheap だけに効くことになる | **互換で残す**（cheap だけの規則と文書化） |
| crates/llm-proxy/src/server.rs:339-353（`SourceScope::Any`） | `prefer_free` かつ `models.qwen[tier]` があり relay が到達可能なら **relay だけ**を返す | (1) models.qwen が全 tier なので `celeris/frontier`・`celeris/standard` も Qwen。(2) relay が到達可能と判定された後に要求が失敗しても oauth へ倒れない（候補が relay だけ） | (1) は models.qwen の cheap 化で解消し、加えて `tier != Cheap` なら relay を候補にしない防御を入れる。(2) relay の後ろに oauth 候補を続ける（cheap で Qwen 落ち → Claude/GPT cheap を試験で固定） → proxy-cheap |
| crates/llm-proxy/src/server.rs:329-338（`SourceScope::Only(Qwen)`） | `qwen/<tier>` は models.qwen[tier] が無ければ候補なし（422/503） | cheap 化後 `qwen/frontier` が失敗する | 意図どおり（**互換で残す**）。応答を試験で固定 → proxy-cheap |
| crates/llm-proxy/src/server.rs:292-310（`Explicit` の `qwen:<model>`） | tier を通らず素通り | tier 制約が掛からない | 明示の素通りは**互換で残す**（tier の抽象ではないため）。ADR-0131 に明記 → adr |
| crates/llm-proxy/src/server.rs:740-750 `/v1/models` | openai_compatible があれば `qwen/frontier`・`qwen/standard`・`qwen/cheap` を並べる | 使えない名前を広告する | `qwen/cheap` だけにする（models.qwen にある tier だけ） → proxy-cheap |
| crates/llm-proxy/src/server.rs:371-382 `resolves_tier` | `attempts_for` をなぞる表示用 | GUI の「frontier → openai-compatible:qwen」表示が消えるはず | 変更不要（attempts_for に従う）。試験で frontier/standard が qwen に解決しないことを固定 → proxy-cheap |
| crates/llm-proxy/src/selection.rs:3,151-160 `rank_relays` | 設定順＋到達性 | 問題なし | **互換で残す** |
| crates/llm-proxy/tests/proxy_integration.rs:1115-1166 / 1167- / 1453- | prefer_free の試験は `celeris/cheap` だけを見ている | frontier/standard が Qwen に倒れないことは未固定 | 試験を足す: frontier/standard + 生きた relay → oauth、cheap + 生きた relay → relay、cheap + 落ちた relay → oauth cheap → proxy-cheap |
| crates/celerisctl/src/commands/routing.rs:129-141,192 | routing 表の qwen 列（試験は qwen3.8-27b の存在だけ） | frontier/standard の qwen 欄が空になる | 表示は**互換で残す**、試験に「cheap 以外は空」を足す → proxy-cheap |
| config/celeris.example.toml:220-223 | `[llm_proxy.models.qwen]` の例が全 tier qwen | 方針 3 と矛盾 | **消す** frontier/standard → config-docs |
| gui/app/lib/llm-sources.ts:52,78,80、gui/app/routes/accounts.tsx:607-609 | 「無料の Qwen が使えるため優先しています」「Qwen が届かないため Claude / GPT に倒れています」「celeris/<tier> はそちらへ倒れます」 | 全 tier で Qwen を示唆 | **改名**: cheap だけの説明に（frontier/standard の理由語に free-first が出ないこと） → gui-ui |

## (d) providers API・schema・web/gui の providers 画面の現状

| path:line | 現状 | 処置案 |
|---|---|---|
| crates/task-api/src/handlers.rs:100-105 | `GET/POST /api/v1/providers`、`PATCH/DELETE /api/v1/providers/{id}`、`POST …/{id}/check` | **互換で残す**（道具の行の API） |
| crates/task-api/src/handlers.rs:182-183、crates/task-api/src/llm_sources.rs:16-25 | `GET /api/v1/llm/sources`（`[llm_proxy]` 無効なら 409） | LLM source の正本の API として**互換で残す**。providers の行から LLM source を参照できるよう、ProviderView に `llm_source`（導出値）を足す → provider-split |
| crates/task-api/src/types.rs:304-331 `ProviderView`、:443-460 `ProviderConfigView` | adapter と LLM source の欄（tier_models・account_id・account_pool・credential_refs・model）が 1 つの型に並ぶ。区別の欄が無い | 新欄（例 `kind: "harness"`、`llm_source: {kind, id} | null`）を足す。既存欄は**互換で残す** → provider-split |
| crates/task-api/src/admin.rs:210-240 `ProviderConfigFile`、:277 `ProviderCreateBody`、:323-345 `ProviderPatchBody` | providers.d/<id>.toml の形（celeris の ProviderConfig と同形を別に持つ） | 新欄を足すなら celeris/config/providers.rs:14-51 と同時に足す（deny_unknown_fields のため片方だけだと読めない） → provider-split |
| crates/task-api/src/handlers/providers.rs:26,45,373-405 | account_pool の adapter 検査、command/args の拒否、tier_models の検査、credential 移行 | **互換で残す** |
| docs/api/v1/api-v1.schema.json:14175 `ProviderConfigView`、:14422 `ProviderView`、:10319-10432 `LlmSource*View` | schema（UPDATE_SCHEMA=1 で再生成） | 型を変えたら再生成 → provider-split |
| web/features/ops/providers-screen.tsx:54-57,145-174,193-198、web/features/ops/providers-form.ts:4 | 1 つの一覧「プロバイダ一覧」。各行は `adapter / concurrency / tiers` だけ。ADAPTERS に `langmem` が無い | 「道具（adapter/harness）」と「LLM source」を分けた節にする。LLM source の表示は `/api/llm/sources` から（現状は secrets-section.tsx:66 にだけある） → web-ui |
| web/features/ops/secrets-section.tsx:66-70 | LLM source を「secret と LLM source」節に表示 | providers 画面の LLM source 節へ移すか参照を置く → web-ui |
| gui/app/routes/providers.tsx:94-97 `ADAPTER_OPTIONS`、:250-253、:505-511 | adapter の選択肢（langmem なし）、行の説明は codex→「GPT (Codex)」・claude-code→「Claude」、それ以外は adapter 名。tier 対応欄は claude/codex だけ | 行に「道具」と「LLM source（celeris/<tier> → proxy、claude/codex pool、直指定）」を分けて出す → gui-ui |
| gui/app/routes/accounts.tsx:600-610、gui/app/lib/llm-sources.ts:40-90 | `/accounts` の「LLM source」節（tier ごとの解決先と理由） | 文言を cheap 専用 Qwen に揃える（(c)） → gui-ui |

## 移行で気を付けること（後の葉への引き継ぎ）

- 本番の provider id（`opencode-qwen`・`ldr-qwen`・`paperqa-qwen`）を変えると stats・providers.d のファイル名・cooldown の鍵が変わる。
  id の変更は人の移行手順（config-docs）に入れ、コードは旧 id をそのまま読む（id に意味を持たせていないので互換は自動）。
- `[llm_proxy.models.qwen] frontier/standard` を書いた既存の設定（本番を含む）は、無視＋警告にすれば読み込みは壊れない。設定エラーにすると本番の再起動が止まるので推さない。
- `qwen/<tier>` と `qwen:<model>` の名前は proxy の外（opencode の JSON・PaperQA の settings）に書かれている可能性がある。接頭辞自体は残す。
