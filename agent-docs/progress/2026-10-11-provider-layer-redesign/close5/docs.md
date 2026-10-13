# close5 docs: 旧 tier alias の説明と API 改訂履歴・langmem 例

tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]

## 変えたこと

- `docs/guides/providers.md`（特化 harness 節）: tier alias 名（`celeris/cheap`・`celeris/standard`）を外し、「tier alias は廃止済みで、具体的な `<source_id>/<model_id>` を書く」に替えた。Qwen を先に置き Claude/Codex の候補を続ける説明は残した。
- `docs/guides/llm-source.md`（§1 と §6）: 旧 `celeris/<tier>`・`claude/<tier>`・`gpt/<tier>`・`qwen/<tier>` の列挙と `celeris/cheap` / `celeris/standard` を外し、具体的な `<source_id>/<model_id>` を書く旨に替えた。
- `docs/ops/coding-harness-pi-hashline.md`（§5 の注意）: `celeris/<tier>` を「旧 tier alias」に替えた。
- `docs/api/v1/api.md`（`celeris_tiers[]` の節）: 「旧 tier alias の解決先。今の daemon は常に空配列を返す」に書き直した。`celeris_tiers` は JSON 欄名なのでそのまま残す（生成 schema.json は触らない）。
- `docs/api/v1/api.md` 冒頭の改訂履歴: `POST`/`PATCH /providers` の `concurrency` の行に「この API は後に削除（§3.19）」を添えた。
- `docs/ops/provider-llm-source-migration.md`: `[knowledge.langmem]` の TOML 例の `model` を `qwen/qwen3.8-27b` にした（`base_url` は proxy）。本文の注記も「proxy を指すなら `qwen/qwen3.8-27b`、Qwen を直接指すなら `qwen3.8-27b`」と一致させた。
- `docs/api/v1/api.md` 改訂履歴: check の文字列（`concurrency は受け付ける`）に合わせ、`concurrency` の backtick を外した（文の意味は変えていない）。

## 検査

- 旧 alias 名の grep（`celeris/(frontier|standard|cheap)`・`celeris/<tier>`・`(claude|gpt|qwen)/<tier>`・`celeris/cheap`・`celeris/standard`）を `docs/` 全体（生成 schema.json を除く）に掛けて 0 件。残りは次節の「意図的に残したもの」の語だけ。
- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`（exit 0）。
- `git diff --check` → 差分ゼロ（exit 0）。

## 意図的に残したもの

- 移行 runbook（`docs/ops/provider-llm-source-migration.md`）の `[llm_proxy.models]`・`[[providers]]`・`providers_include` の記述。「読み込みエラー」と移行の説明として残す。
- `docs/guides/llm-source.md`・`docs/guides/providers.md` の「tier alias は廃止」「`[[providers]]` は読み込みエラー」という説明。
- 「cheap」「standard」は lane 名（`frontier`/`standard`/`cheap`）として別の意味で使われている箇所。alias 名ではないので触らない。

## この leaf で触らなかったもの（別 leaf・親の未解決）

- `docs/guides/knowledge.md` の langmem 例（`[[providers]]`・`concurrency`）。objective の範囲外。
- `docs/ops/opencode-go-and-model-catalog.md` §3a、`docs/ops/selfdeploy.md` の `providers.d/` 行。
- `docs/api/v1/gui-api.md` の provider 節。
- crates/ と web/ は変えていない。

## check の結果（この leaf の WorkUnit 内）

- `grep -n 'concurrency は受け付ける' docs/api/v1/api.md | grep -q '§3.19'` → exit 0。
- replan v2 で `git merge-base HEAD main` 基点の crates 検査は外した（branch は main の先にあり、merge-base が main 自身になるため意味を成さない）。代わりに `git diff --quiet $CELERIS_WU_BASE -- crates/ web/` → exit 0（この leaf は crates/ と web/ を変えていない）。
- 旧 alias の grep（docs/guides docs/ops docs/api/v1/api.md）→ 0 件。`web/features/ops/routing-catalog.test.tsx` の `celeris/cheap` は web WU の担当で、この leaf は触っていない。
