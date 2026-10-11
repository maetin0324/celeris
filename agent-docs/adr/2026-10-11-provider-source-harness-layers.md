---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
---
# ADR 2026-10-11: provider を LLM source と harness の関係にする

- 日付: 2026-10-11
- 状態: 決定（実装待ち）
- 関連: [provider 層の再設計・棚卸し](../progress/2026-10-11-provider-layer-redesign.md)、[ADR-0132](0132-provider-llm-source-split-and-cheap-qwen.md)、[アカウント同時実行数](2026-10-08-account-concurrency-limits.md)、[破壊的変更を既定とする ADR](2026-10-11-breaking-changes-by-default.md)

## 背景

現行の `[[providers]]` は、LLM の供給元、実行アダプタ、モデル対応、account pool、同時実行数を一つの行に持つ。source を adapter や model 名から推定する経路があり、推定できない provider は `unknown` として残る。provider は実行手段そのものではなく、実在する LLM source と harness の対応および利用可能 model の宣言にする。

この決定は ADR-0132 と 2026-10-08 の account concurrency ADR のうち、derived source、旧 tiers/tier_models、provider concurrency、暗黙的な source/account key に依存する箇所を置き換える。過去の実装記録は履歴として残す。旧欄を読んで警告する互換分岐や切替 flag は設けず、削除した欄が設定に残れば読み込み時にエラーとする。これは「破壊的変更を既定とする」ADR に従う。

## 3 層の宣言

設定を `[[llm_sources]]`、`[[harnesses]]`、`[[providers]]` の3層に分ける。ID は各層で一意とし、参照は文字列 ID で明示する。LLM source は接続と account の正本、harness は adapter の実行契約、provider は source と harness の組み合わせおよび model 対応だけを表す。

### LLM source

`[[llm_sources]]` は `id`、`kind`、認証/account pool、同時実行数、quota、利用可能 model を持つ。`kind` は `claude_oauth`、`codex_oauth`、`opencode_go`、`openai_compatible:<id>`、`celeris` のいずれか。`celeris` は Celeris proxy を通る source であり、その上流選択は proxy の責務とする。認証情報の値は config に直接書かない。

```toml
[[llm_sources]]
id = "claude-main"
kind = "claude_oauth"
account_pool = "claude-code"
max_concurrency = 3
quota = "subscription"
models = ["claude-opus", "claude-sonnet"]

[[llm_sources]]
id = "qwen"
kind = "openai_compatible:qwen"
account_pool = "qwen-account"
max_concurrency = 1
quota = "local-gpu"
models = ["qwen3.8-27b"]
```

account pool の所属・実アカウント・暗黙 account の上限は source に結び付ける。quota は source の利用制約として記録し、harness/provider に重複させない。

### harness

`[[harnesses]]` は `id`、adapter 種別、話せる source 種類、能力、`command`、`args`、`settings` を持つ。能力は `tool`、`browser`、`credential` などを宣言する。source 種類の制約は接続可能性を示し、個別 source ID や利用可能 model を推定するものではない。

```toml
[[harnesses]]
id = "claude-code"
adapter = "claude-code"
source_kinds = ["claude_oauth"]
capabilities = ["tool", "browser", "credential"]
command = "claude"
args = ["--print"]

[harnesses.settings]
permission_mode = "acceptEdits"
```

### provider

`[[providers]]` は source と harness の明示参照、およびその組で使える model 対応だけを持つ。上限・認証・account pool は持たない。`llm_source` は必須の source ID 参照であり、adapter/model から導出しない。

```toml
[[providers]]
id = "claude-opus"
llm_source = "claude-main"
harness = "claude-code"
models = ["claude-opus"]

[[providers]]
id = "qwen-opencode"
llm_source = "qwen"
harness = "opencode"
models = ["qwen3.8-27b"]
```

provider の候補は、この明示された関係と model 対応から作る。`derived` source の状態は廃止する。参照のない provider を暗黙 source に束ねることも認めない。

## 削除する欄

以下は新しい3層へ移し、旧欄を受け入れる型・API field・画面を削除する。config に旧欄が残れば parse/validation error とし、新しい欄への移行を促す。読み込み互換、警告だけでの継続、切替 flag は置かない。

| 旧欄・旧概念 | 削除・移動先 |
|---|---|
| `[[providers]].concurrency` | 削除。上限は LLM source の account ごとの同時実行数へ移す |
| `[[providers]].kind`（`adapter` 判定） | 削除。実行 adapter 種別は `[[harnesses]].adapter` へ移す |
| provider の `derived` / `SourceOrigin::Derived` | 削除。全 provider が明示的な `llm_source` を参照する |
| `tiers` / `tier_models` | 旧 tier 欄を削除。provider の model 対応を `models` として明示し、tier routing は後続の routing 設計で定義する |
| `account_pool` on provider | provider 欄から削除し、LLM source/account の所有関係へ移す |
| provider API の `concurrency`、`in_use_cos` 等 provider 単位の上限・稼働数 field | provider API から削除。上限と利用状況は source/account の境界で扱う |
| provider に埋め込まれた command/args/settings | `[[harnesses]]` へ移す |

## unknown source の解消

`unknown` は有効な source 値や fallback ではなくす。起動時の config 検証で、各 provider の `llm_source` が `[[llm_sources]]` に登録された ID と一致すること、参照先の種類を harness が受け付けること、provider の model が source と harness の対応可能な範囲にあることを検査する。一つでも満たさなければ設定読み込みを拒否し、provider ID と不正な参照先を示す。

source が未登録、ID の誤記、互換性のない source kind のいずれも `unknown` に変換して実行を続けない。全ての provider は実在 source を参照し、source の追加・削除も同じ registry の検証を通す。

## API と画面

API は設定の三層をそれぞれ編集・取得できる形にする。`GET/POST/PATCH/DELETE /api/v1/providers` は provider relation（`id`、`llm_source`、`harness`、`models`）を扱い、`concurrency`、`in_use`、`in_use_cos`、`kind`、`derived` など旧 provider 欄を要求・返却しない。`GET/POST/PATCH/DELETE /api/v1/llm_sources` は source の種類・接続設定・account pool・quota・利用可能 model を扱い、`GET/POST/PATCH/DELETE /api/v1/accounts` は source 所有の実 account と上限を扱う。harness 一覧（`GET /api/v1/harnesses`）は adapter、対応 source kind、capabilities、command/args/settings を返す。harness 定義の編集 API は同じ harness 単位の shape とし、provider に実行設定を戻さない。全 endpoint の入力は未知の旧欄を拒否する。

画面の正本は `web/` とし、source/account、harness、provider relation を別々に表示・編集する。provider 画面は source と harness の選択および model 対応に限り、上限や認証を置かない。旧 `gui/` は新 API と画面に追随させず、provider・source・account の重複画面を削除する。破壊的変更を既定とする ADR に従い、旧画面を互換 UI として残さない。

## 各部の使い方

- **routing** は有効な provider relation を候補として列挙し、source の account 状態・quota と harness の能力を別々に照合する。tier 名やモデル選択規則は provider に埋め込まず、routing 自身の入力として扱う。
- **CoS** は run の実行に選ばれた provider relation と harness を使う。session の実行条件・再開情報は harness/source/model を明示して照合し、provider ID だけを source/account の同一性として扱わない。
- **llm-proxy** は `celeris` source の上流接続・model routing・proxy 内 HTTP 要求の account quota を管理する。dispatcher の run admission と proxy の request admission は別の上限である。
- **dispatcher** は relation 候補を選び、実行直前に source のアカウント上限・状態を適用して harness を起動する。CoS を含む run 数の計上は account/source 側に置き、provider 行の concurrency には戻さない。
- **accounts** は source に属する認証済み account、quota、上限、稼働状況の正本である。implicit account も source ID を所有キーとし、provider 行ごとに複製しない。

## routing の境界

provider 候補の列挙は **source × harness × model の組**を単位とする。source が利用可能な model と harness が受け付ける source kind/capability の積集合から、明示登録された provider relation を候補にする。存在しない組を推定で作らず、不正な参照は config 検証で拒否する。

この境界より後の優先順位（tier/lane、task hint、sticky、reviewer、health/cooldown、quota、account の空き、fallback）は routing 作り直し task が定義する。現行の provider ID を候補の同一性や順位の根拠に流用せず、source/account の admission と harness 実行可能性を候補選択から独立して評価する。

## 本番 config の移行

本番移行は人が行う deploy 手順とし、daemon が旧 config を自動変換したり、本番 config を書き換えたりしない。手順では現行 config と provider ID の対応表を控え、新しい source・harness・relation を記述して検証してから切り替える。旧欄が残った config は読み込みエラーとなるため、旧 config を部分的に混ぜた状態で起動しない。

既存 provider ID を同じ relation の ID として保てるものは維持し、stats の過去集計を連続させる。ID を変える必要がある relation は移行表で旧 ID と新 ID の対応を明記し、DB migration で provider keyed stats/cooldown を新しい relation key に移す。複数 relation への分割など一意に移せない stats は旧キーを消さず履歴として保持し、新 key に誤配しない。cooldown は意味が一致する一対一の移行だけ引き継ぎ、対応が曖昧なら移行先を cooldown なしで開始する。孤立した旧キーを掃除する場合も migration の明示対象とする。

SQLite の既存データは migration で必要な値を新しい形へ移してから旧列・旧表を除く。events は追記専用とし、旧 event の payload を書き換えたり削除したりせず、新 event を新しい relation/source の識別子で追記する。credential と秘密は移動・再参照して保持し、消去・ログ出力・画面への露出をしない。これらは「破壊的変更を既定とする」ADR のデータ保全規則に従う。
