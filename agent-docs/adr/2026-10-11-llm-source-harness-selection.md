---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
title: LLM source と harness の選択
---
# LLM source と harness の選択

- 状態: 決定（実装待ち）

この ADR は、実行器の選択と供給元・model の選択を独立させ、dispatcher が明示された情報だけで決定的に実行条件を確定する設計を定める。現行の `crates/celeris/src/config/providers.rs` と `crates/task-dispatch/src/dispatcher/provider_select.rs` では adapter、model、source、account pool、容量が同じ provider 行と選択処理に結び付いている。`crates/celeris/src/config/harness.rs` には harness/genre の宣言が既にあり、task の genre や `worker_hint.adapter` も存在する。以下ではこれらの責務を分ける。

## provider の廃止

`[[providers]]`、`providers.d`、provider ID を設定・選択・API の概念から廃止する。設定ローダーは provider テーブルまたは `providers.d` の存在を検出したら、読み込みエラーとして新しい `[llm_sources.<id>]` と `[harnesses.<id>]` の書き方を案内する。黙って無視する経路、派生値へのフォールバック、警告だけで続行する経路、旧新を切り替える flag は設けない。provider ID から source や harness を推測しない。

```toml
# 旧 [[providers]] / providers.d ではなく、独立した宣言を使う
[llm_sources.qwen]
kind = "openai_compatible"
endpoint = "http://127.0.0.1:17942/v1"

[harnesses.pi]
kind = "pi"
api_formats = ["openai_compatible"]
```

設定例・設定 loader と validation の変更対象は `crates/celeris/src/config/providers.rs` および config の読込・validation 経路である。残存した provider ID を暗黙 account の識別子や cooldown key として新たに使わない。

## LLM source

LLM source は接続先・認証・供給側制約を持つ宣言である。`kind` は認証/接続方式を識別し、account、同時実行上限、quota、利用可能 model を source の下に置く。model は source が提供する model ID として宣言し、source をまたいで同じ表示名でも別候補として扱う。秘密値は設定本文に埋めず、既存の credential 経路で参照する。

```toml
[llm_sources.claude_main]
kind = "claude_oauth"
accounts = ["claude-a", "claude-b"]
max_runs = 4
quota = { five_hour = 0.8, seven_day = 0.9 }
models = ["claude-sonnet-4-5", "claude-opus-4-1"]

[llm_sources.qwen]
kind = "openai_compatible"
endpoint = "http://127.0.0.1:17942/v1"
credential_ref = "qwen-api-key"
accounts = ["shared-qwen"]
max_runs = 2
models = ["Qwen3-Coder"]
```

既存 account CRUD と `account_limits` 永続化は活かす。現行 `crates/task-core/src/store/account_limits.rs` の adapter key は、source ID と account ID の組に移行し、`max_runs` の読み書きを維持する。source に複数 account がある場合、各 account の明示上限と source 全体の上限の両方を満たすよう admission を行う。quota・cooldown・利用中数は account/source の状態として扱い、harness や task lane に所属させない。暗黙 account を必要とする source は source ID を安定した所有者として明示的に作成し、`unknown` の共有枠は認めない。

## harness

Harness は task を実行するプログラムとその起動契約である。宣言は種別、受け付ける API 形式、利用できる source の種類、能力、必要な command/args を含む。API 形式（例: OpenAI-compatible、Anthropic、CLI 固有 protocol）と source の認証方式は別属性にする。受け付ける組み合わせが実測されていない場合は自動で互換と見なさず、明示的に追加されるまで候補にしない。

```toml
[harnesses.claude_code]
kind = "claude_code"
api_formats = ["claude_cli"]
source_kinds = ["claude_oauth"]
capabilities = ["tools", "repo", "skills", "browser_context"]
command = "claude"
args = ["--print"]

[harnesses.opencode]
kind = "acp"
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible", "opencode_go"]
capabilities = ["tools", "repo", "skills"]
command = "opencode"
args = ["acp"]
```

既存 `[adapters.*]` の process 固有設定と `command`/`args` は対応する harness 宣言へ移す。能力は harness が提供する機能を記述し、task の browser authorization や credential grant の判定を置き換えない。browser specialist 等の補助機能は、それ自体が LLM API harness でない限り source を持たず、選択済み worker harness の文脈機能として扱う。

## harness の選択

Harness は供給元や model を見る前に task 情報から決定的に選ぶ。優先順は (1) task に明示された harness ID、(2) task の `worker_hint.adapter` を一意の harness ID に解決した値、(3) `genre` と `role` に対応する設定済み harness、(4) repo 作業等の明示された execution class に対する設定済み既定、とする。同じ優先段で候補が複数なら設定順で選ばず、設定エラーまたは未割当として扱う。利用可能な既定が無ければ task を未割当で止める。

```toml
[harness_selection]
default_by_execution_class = { coding = "codex", conversation = "claude_code" }
by_genre = { research = "local_deep_research" }
```

人が指定する場合は task の `harness` 欄に登録済み ID を記す。`worker_hint.adapter` は既存 task との移行中に同じ ID へ解決する入力として残す。genre の `capabilities` は説明・適格性条件に利用できるが、文面を解析して harness を推測しない。dispatcher は設定済みの優先規則と task の構造化欄だけを使い、LLM を呼び出さない。`adapter_policy = model_family` のように model family から harness を逆算する規則は廃止する。

## source と model の選択

Harness 決定後、候補を `source × model` の組として列挙する。候補は source の宣言済み model だけから作り、harness が受け付ける source kind と API format の条件を満たすものに限る。制約は account availability/cooldown、account と source の同時実行数、quota、task に明示された model、harness との組み合わせである。各除外理由を routing trace に残せる形にする。

```toml
[llm_sources.qwen]
kind = "openai_compatible"
models = ["Qwen3-Coder", "Qwen3-Reasoner"]

[harnesses.pi]
kind = "pi"
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible"]
```

選択根拠となる cost、latency、品質の観測が存在しない間は、適格候補を source ID、model ID の辞書順で整列し先頭を選ぶ。この順序は再現可能な暫定既定であり、設定順・ランダム値・暗黙の lane から優先度を作らない。task が model ID を指定した場合は一致する候補だけを残し、該当なしなら失敗として明示する。`frontier`/`standard`/`cheap` は task の予算・方針ラベルとして残せるが、特定 source や model を暗黙に意味しない。

## 結合

Harness 宣言が受け付ける source kind と API format を source 宣言と照合し、source/model 候補を絞る。直接接続できる場合は選択済み model ID と source credential を harness に渡す。source と harness の形式が異なる場合、`llm-proxy` に変換経路が明示登録されている組だけを許可する。proxy は source 側の接続・認証・quota を保持し、上流の要求/応答形式を harness の API format に変換する。変換不能、未登録、未実測の組み合わせは dispatch 前に拒否し、別 source へ勝手にフォールバックしない。

```toml
[[llm_routes]]
harness = "local_deep_research"
source = "qwen"
model = "Qwen3-Coder"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "openai_compatible"
```

`crates/llm-proxy/src/config.rs` の source 定義、model alias、`selection.rs` の選択・変換を、harness から要求された形式を満たす明示経路として整理する。`celeris/<tier>` 等の alias は source/model ID の代用にせず、alias を使う場合も解決後の source/model を trace に記録する。API 形式の一致だけで認証方式や実接続可能性を推測してはならない。

## 削除する欄と仕組み

| 現行欄・仕組み | 決定 | 新しい所属・扱い |
|---|---|---|
| `[[providers]]`、`providers.d`、provider ID | 削除 | source ID と harness ID を個別に宣言 |
| `adapter` | provider から削除 | harness の `kind` / ID |
| `llm_source`、provider `model` | provider から削除 | source/model 候補、必要なら明示 route |
| `tiers`、`tier_models` | provider 設定から削除 | lane は task 方針に残し、model 対応は明示候補・route に置く |
| `lane` / `Tier` | 廃止しない | task 予算・routing policy のラベル。source/model/harness の別名にはしない |
| `worker_hint.adapter` | 互換入力として限定的に残す | 登録 harness ID への決定的な手動制約。新 task は `harness` 欄を使う |
| `account_pool` | provider から削除 | source の `accounts` |
| provider `concurrency`、API `in_use_cos` | 削除 | source/account の admission と観測へ移す |
| `account_limits` | 維持・key を更新 | `(source_id, account_id)` ごとの上限永続化 |
| `env_keys`、`credential_refs` | provider から削除 | source credential 参照と harness process env を分離 |
| `command`、`args`、adapter `settings` | provider から削除 | harness 定義に置く |
| `SourceOrigin::Derived`、`Unknown`、source derive | 削除 | 登録済み source のみ許可し、不明値は設定/dispatch error |
| `[llm_proxy.models]` の tier alias | proxy から直ちには削除しない | 移行中の名前解決に限り利用可能。選択 trace は具体的 source/model を記録 |

既存 config に削除対象の provider 欄が残る場合は設定エラーにし、読み替えや警告だけの互換動作を実装しない。account 上限の永続データは source/account の識別子が一意に対応するものだけを移し、対応不明な値を推測で割り当てない。
