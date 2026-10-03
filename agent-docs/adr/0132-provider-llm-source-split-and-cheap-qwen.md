---
tasks: [01M3YF3NSR46FM314VHG14T3N6]
---
# ADR-0132: provider の LLM source と adapter を分け、Qwen は cheap に限る（ADR-0053 付記）

- 日付: 2026-10-02
- 状態: **実装済み**（2026-10-02。D1〜D7 を provider-split・proxy-cheap・config-docs・gui-ui・web-ui・knowledge-route・worker-tools の各 WorkUnit で実装し、`cargo fmt --all -- --check` / `cargo clippy --workspace -- -D warnings` / `cargo test --workspace --no-run` / `cargo test -p llm-proxy` / `cargo test -p celeris config::` / `cargo test -p task-api --test providers_admin --test llm_sources` / `cargo test -p task-dispatch --lib` で確認した。詳細は [進捗ファイル](../progress/2026-10-02-provider-llm-source.md) の節「provider の LLM source / adapter 分離と cheap 専用 Qwen」。人の方針: paperqa・langmem・local-deep-research は Qwen 専用でなくてよく、GPT 系へのデータ送信を許す。LLM source と adapter / harness を設定・API・GUI で区別し、既存設定を読めるようにする。Qwen3.8-27B は cheap lane だけで使う）
- 関連: [ADR-0053](0053-llm-source-proxy.md) D1・D2（供給元の抽象と proxy）、[ADR-0052](0052-knowledge-run-fallback.md) D2（知識整理の汎用ハーネスへの fallback）、[棚卸し](../progress/2026-10-02-provider-llm-source/inventory.md)
- 番号: 2026-10-02 に `main` と全 `celeris/*` ブランチの `docs/adr` を確認した。0131 は `celeris/01M3YF3NS2EGTZD2BBWNPG1K28` の cron jobs で使用済み、0132 は空き。

## 文脈

2026-10-02 の本番設定では、`claude-pool` と `codex-pool` はアカウントを持つ CLI の行であり、proxy は別に `claude_oauth`・`codex_oauth`・`openai_compatible` を LLM source として持つ。一方、`opencode-qwen`・`ldr-qwen`・`paperqa-qwen`・`langmem-main` は `[[providers]]` に並び、道具の種類とモデルの供給元が同じ「provider」に見える。後の 3 つは proxy の `celeris/<tier>` を使っており、名前の Qwen と実際の供給元も一致しない。さらに `[llm_proxy.models.qwen]` の全 tier に Qwen があるため、`prefer_free = true` では frontier / standard も Qwen を選びうる。

この付記は上記の人の方針を ADR-0053 D1・D2 のうち Qwen を全 tier の候補とする記述、道具を供給元と同一視する記述より優先する。ADR-0053 の当時の実装記録は履歴として残す。

## 決定

### D1. 用語と設定の境界

- **LLM source** はモデルと資格情報を供給するもの。正本は `[llm_proxy.sources.claude_oauth]`、`[llm_proxy.sources.codex_oauth]`、`[[llm_proxy.sources.openai_compatible]]`（Qwen はその一例）、およびその `[llm_proxy.models]` tier 写像。`celeris/<tier>` は source 名ではなく proxy が source を選ぶ抽象モデル名。
- **adapter / harness** は仕事を実行する道具と実行契約。`claude-code`、`codex`、`acp` / opencode、`paperqa`、`langmem`、`local-deep-research` がこの側に属する。`[[providers]]` は道具の実行枠であり、source 自体の登録表ではない。既存の `adapter`、`tiers`、`concurrency`、`command`、`args`、`settings` は道具の欄として維持する。
- provider 行の性質は `kind = "adapter"` と明示する。旧行で `kind` が無ければ `adapter` と解釈する。LLM source の行を `[[providers]]` に増やさず、source の登録は既存の `[llm_proxy.sources.*]` に置く。
- provider の `llm_source` は**参照**であり、資格情報や実モデルの複製ではない。`"celeris"` は proxy の動的選択、`"claude_oauth"` / `"codex_oauth"` は各 CLI がそのアカウントで直接使う source、`"openai_compatible:<id>"` は明示的な直結、`"none"` は fake 等の非 LLM 道具を表す。`openai_compatible:<id>` の `<id>` は既存の `[[llm_proxy.sources.openai_compatible]].id` を参照する。`celeris` の実際の解決先は run 時に決まり、provider 行の固定情報として扱わない。
- `model` / `tier_models` は要求モデルの指定として残す。proxy を使う道具は `llm_source = "celeris"` と `model = "celeris/<tier>"`（道具固有の `openai/celeris/<tier>` 表記を含む）を組み合わせる。Claude Code / Codex は従来の `account_pool`・`account_id`・`tier_models` を保ち、それぞれ `llm_source = "claude_oauth"` / `"codex_oauth"` を付ける。`account_pool` と資格情報参照は source 側の属性として画面に分けて示す。

### D2. 旧設定の互換読み込みと警告

- 2026-10-02 の本番形、すなわち `opencode-qwen`（acp、frontier / standard / cheap、`OPENCODE_CONFIG` で Qwen 固定）、`ldr-qwen`（local-deep-research、cheap、実体は `celeris/cheap`）、`paperqa-qwen`（paperqa、全 tier、実体は `celeris/standard`）、`langmem-main`（langmem、cheap / standard）、および `[llm_proxy.models.qwen]` の全 tier 対応は**設定ファイルとして読み込める**。旧 `id`、`adapter`、`model`、`tier_models`、`account_pool`、`env`、`settings`、`providers_include` の意味と API の既存フィールドは維持する。
- `kind` / `llm_source` の無い旧行は実行経路から推定する。Claude Code / Codex はそれぞれの OAuth、proxy の `celeris/<tier>` を使う道具は `celeris`、Qwen 固定の opencode は `openai_compatible:qwen` と表示する。settings / env の中身から確定できないものは `unknown` と表示し、黙って `qwen` と決めつけない。読込時に非推奨の `-qwen` id・Qwen 直指定・推定不能な参照を警告するが、認証情報の値は出さない。
- `kind` / `llm_source` を明示した行では、モデル名・adapter・参照先の整合を検証する。旧行から導出した参照は表示用として扱い、移行前に実行経路を勝手に切り替えない。旧 provider id は stats・cooldown・`providers.d` の鍵なので自動改名しない。

### D3. Qwen の tier 写像は cheap だけ

- `[llm_proxy.models.qwen]` の既定と移行後の設定は `cheap = "qwen3.8-27b"` のみとする。旧 `frontier` / `standard` キーは構文エラーにせず、警告して**無視**する。`prefer_free` は cheap でだけ Qwen 優先に作用する。
- `celeris/frontier` / `celeris/standard` は Qwen 候補を作らず、Claude / Codex の対応 tier のみを選ぶ。`qwen/frontier` / `qwen/standard` は使えないモデルとして明確なエラーにし、`/v1/models` に広告しない。`qwen/cheap` は有効。cheap では生きた Qwen を先に試し、失敗・到達不能なら同じ要求を stream 開始前に Claude / GPT の cheap 候補へ倒す。proxy の偽上流試験でこの 3 経路を固定する。
- `qwen:<concrete-model>` の明示的な素通りは tier 抽象を通らない既存 API として残す。ただし task の frontier / standard の provider 設定には使わせない。Qwen 直結の実行枠は cheap のみである。
- ACP 行の実効 source が Qwen（`model` の Qwen 直指定、`llm_source = "openai_compatible:qwen"`、または Qwen 固定の `OPENCODE_CONFIG`）なら、設定に非 cheap tier が残っても dispatch 候補は cheap のみとする。

### D4. PaperQA・LangMem・LDR は普通の道具

- 新しい設定例と provider id は `paperqa`、`langmem`、`ldr` のように source を名前に含めない。各道具のモデルは proxy の `celeris/<tier>` とし、PaperQA の設定 JSON、LangMem の `[knowledge.langmem]`、LDR の設定もその抽象名を参照する。どの道具も Qwen を必須条件にしない。特に `celeris/standard` の PaperQA は Qwen に倒れない。
- ADR-0052 D1 の `/models` probe は **LangMem の設定された接続先**の到達性検査として残す。proxy 構成では bearer を付けて proxy を probe し、個別の Qwen トンネルは probe しない。proxy に到達できるが Qwen が落ちた場合は D3 の proxy 内 fallback が働く。proxy 自体へ到達不能なら ADR-0052 D2 の cheap 汎用ハーネス fallback を使う。旧 `via = "langmem" | "fallback:<adapter>"` は DB / API 互換のため維持する。

### D5. opencode の Qwen 経路は cheap のみ

- Qwen 直指定の旧 `opencode-qwen` と、新しい opencode の Qwen 専用実行枠は cheap だけを受ける。`tiers` に frontier / standard が残る旧行は読み込めても、その tier では Qwen 直指定の候補にしない。警告に移行先を示す。
- 管理 API の POST/PATCH は該当 ACP 行で `tiers` 省略なら `[cheap]` を保存・返却し、frontier / standard の明示指定は 400 で拒否する。proxy の `celeris/<tier>` を使う ACP 行は全 tier を使える。
- frontier / standard で opencode を使うなら、Qwen 固定の `OPENCODE_CONFIG` を外して proxy の `celeris/<tier>` を選ぶ別の道具の行にする。cheap の opencode も proxy の `celeris/cheap` を既定とし、Qwen が落ちたときの D3 の fallback を利用する。

### D6. API と providers 画面

- 既存の `GET/POST/PATCH /api/v1/providers` は adapter 実行枠の API として維持する。provider view / 設定 API に `kind` と `llm_source` を加え、旧フィールドは維持する。`llm_source` は D1 の参照と、導出値か明示値かを判別できる形で返す。LLM source の到達性・残量・tier の解決先は既存の `GET /api/v1/llm/sources` を正本にする。schema と生成型を同時に更新する。
- `web/` と `gui/` の providers 画面は「adapter / harness の実行枠」と「LLM source」を別の節にする。各実行枠には adapter・受ける tier・要求モデル・`llm_source` 参照を、source 節には種類・ID・到達性・tier 写像を表示する。`celeris` 参照は実行時に選ばれることを示し、固定の Qwen と誤表示しない。accounts 画面の Qwen 優先・fallback の説明は cheap にだけ出す。旧 id は移行までそのまま識別子として表示する。

### D7. 移行と本番操作

- [移行手順](../../docs/ops/provider-llm-source-migration.md) に、現行 config の控え、旧 ID と `providers.d` の対応、`models.qwen` の非 cheap キー削除、proxy 経由の `celeris/<tier>` への切替、Qwen 固定 opencode の cheap 制限、設定検証、画面と応答ヘッダの確認、戻し方を書く。新しい設定例は source と道具を分けて示す。旧 id の変更で履歴や cooldown の鍵が変わることも明記する。
- 本番の `~/.config/celeris`、service、daemon、DB はこの仕事から変更しない。本番 config の適用と daemon 再起動、必要な TOTP 操作は人が手順に従って実行する。認証情報の値を手順・ログ・証跡に載せない。

## 検証条件

- 旧本番形の読み込みと警告、`kind` / `llm_source` の検証、API・schema の互換を試験する。新しい Rust 試験名には `provider_kind` または `cheap_only` を含める。
- 偽上流で `celeris/frontier` / `celeris/standard` が Qwen に向かわず、cheap が Qwen 生存時は Qwen、失敗時は Claude / GPT cheap に倒れることを試験する。`qwen/frontier` / `qwen/standard` のエラーとモデル一覧も固定する。
- 設定例・web/・gui/ の providers 画面、knowledge probe と ADR-0052 D2 の fallback を整合させ、最終検証で `cargo fmt --all -- --check`、clippy、`llm-proxy` と `task-dispatch` の試験を通す。
