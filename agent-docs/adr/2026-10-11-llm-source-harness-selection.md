---
tasks: [01M4M8D4AP4YY4F9TV09HG30SJ]
title: LLM source と harness の選択
---
# LLM source と harness の選択

- 状態: 実装済み（決定 2026-10-11。実装の closeout は 2026-10-12、進捗 `agent-docs/progress/2026-10-11-provider-layer-redesign.md`）

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
models = ["claude-sonnet-5-5", "claude-opus-5-5"]

[llm_sources.qwen]
kind = "openai_compatible"
endpoint = "http://127.0.0.1:17942/v1"
credential_ref = "qwen-api-key"
accounts = ["shared-qwen"]
max_runs = 2
models = ["qwen3.8-27b"]
```

既存 account CRUD と `account_limits` 永続化は活かす。現行 `crates/task-core/src/store/account_limits.rs` の adapter key は、source ID と account ID の組に移行し、`max_runs` の読み書きを維持する。source に複数 account がある場合、各 account の明示上限と source 全体の上限の両方を満たすよう admission を行う。quota・cooldown・利用中数は account/source の状態として扱い、harness や task lane に所属させない。暗黙 account を必要とする source は source ID を安定した所有者として明示的に作成し、`unknown` の共有枠は認めない。

## harness

Harness は task を実行するプログラムとその起動契約である。宣言は種別、受け付ける API 形式、利用できる source の種類、能力、必要な command/args を含む。API 形式（例: OpenAI-compatible、Anthropic、CLI 固有 protocol）と source の認証方式は別属性にする。受け付ける組み合わせが実測されていない場合は自動で互換と見なさず、明示的に追加されるまで候補にしない。

### 宣言の形（harness）

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

Harness は source/model の選択とは独立に、task の構造化情報から決定する。人が指定する場合は task の `harness` 欄に登録済み harness ID を記す。未指定なら `genre`、`requirements`、harness の宣言能力、repo の有無、明示された execution class に対する設定済み選択規則を使う。規則で一意に決まらない場合や適格な harness が無い場合は未割当で fail closed とする。文面を解析して推測せず、LLM も呼び出さない。

旧 task の `worker_hint.adapter` は harness 選択で読み取らない。schema migration はその値を task の `harness` 欄へ移し、登録済み harness ID として解決できない値は `harness = null` にして `harness_migration_unresolved` event を追記する。以後の選択経路は task の `harness` 欄のみを読む。

```toml
[harness_selection]
# genre/requirements/capabilities/repo の適格性に基づく規則を宣言する。
# 一意に決まらない場合は fail closed。
```

`adapter_policy = model_family` のように model family から harness を逆算する規則は廃止する。

## source と model の選択

Harness 決定後、適格性・quota・cooldown・同時実行数を確認した source/model 候補を、設定に明示した優先順で選ぶ。通常 task の既定は `[route_defaults.<harness_id>]` の `candidates` 配列であり、必要なら execution class ごとに同じ形式を定義できる。`default_route_policy` はこの配列順だけを使い、適格な候補の最初を選ぶ。候補宣言の無い harness/class、または全候補が不適格なら fail closed とする。設定順や lane/Tier から候補の優先度を作らない。

```toml
[route_defaults.claude_code]
candidates = [
  { source = "claude_oauth", model = "claude-sonnet-5-5" },
  { source = "codex_oauth", model = "gpt-6.1-sol" },
]

[route_defaults.codex]
candidates = [
  { source = "codex_oauth", model = "gpt-6.1-sol" },
  { source = "claude_oauth", model = "claude-sonnet-5-5" },
]

[route_defaults.opencode]
candidates = [
  { source = "qwen", model = "qwen3.8-27b" },
]
```

この例は本番の使い分けを表す。coding task は claude と codex をこの順で選択でき、軽い仕事は Qwen を使う。必要な場合は execution class ごとに candidates を分ける。langmem、local deep research（LDR）、paperqa は tier alias ではなく、各設定で具体的な source と model を指定する。task が model ID を明示した場合は候補をその ID に絞り、該当が無ければ明示的に失敗させる。

`lane` / `Tier` は provider/model の対応付けに使わない。この値を source/model 選択入力に使うかどうかは後続 task 01M4M8D4AQ が決める。この ADR の選択では入力にしない。`RouteRequest` に `lane` は保持し、後続の policy が差し替えられる境界とする。

## 結合

Harness 宣言が受け付ける source kind と API format を source 宣言と照合し、source/model 候補を絞る。直接接続できる場合は選択済み model ID と source credential を harness に渡す。source と harness の形式が異なる場合、`llm-proxy` に変換経路が明示登録されている組だけを許可する。proxy は source 側の接続・認証・quota を保持し、上流の要求/応答形式を harness の API format に変換する。変換不能、未登録、未実測の組み合わせは dispatch 前に拒否し、別 source へ勝手にフォールバックしない。

```toml
[[llm_routes]]
harness = "local_deep_research"
source = "qwen"
model = "qwen3.8-27b"
via = "llm_proxy"
from_api = "openai_compatible"
to_api = "openai_compatible"
```

`crates/llm-proxy/src/config.rs` の source 定義、model alias、`selection.rs` の選択・変換を、harness から要求された形式を満たす明示経路として整理する。API 形式の一致だけで認証方式や実接続可能性を推測してはならない。

## 削除する欄と仕組み

| 現行欄・仕組み | 決定 | 新しい所属・扱い |
|---|---|---|
| `[[providers]]`、`providers.d`、provider ID | 削除 | source ID と harness ID を個別に宣言 |
| `adapter` | provider から削除 | harness の `kind` / ID |
| `llm_source`、provider `model` | provider から削除 | source/model 候補、必要なら明示 route |
| `tiers`、`tier_models` | provider 設定から削除 | lane は task 方針に残し、model 対応は明示候補・route に置く |
| `lane` / `Tier` | 廃止しない | task 予算・routing policy のラベル。source/model/harness の別名にはしない |
| `worker_hint.adapter` | 削除 | migration で `harness` 欄へ移し、解決不能なら null と `harness_migration_unresolved` event。以後は読まない |
| `account_pool` | provider から削除 | source の `accounts` |
| provider `concurrency`、API `in_use_cos` | 削除 | source/account の admission と観測へ移す |
| `account_limits` | 維持・key を更新 | `(source_id, account_id)` ごとの上限永続化 |
| `env_keys`、`credential_refs` | provider から削除 | source credential 参照と harness process env を分離 |
| `command`、`args`、adapter `settings` | provider から削除 | harness 定義に置く |
| `SourceOrigin::Derived`、`Unknown`、source derive | 削除 | 登録済み source のみ許可し、不明値は設定/dispatch error |
| `[llm_proxy.models]` の tier alias | 即時削除 | langmem・LDR・paperqa を含む利用箇所は設定で具体的 source/model を指定 |

既存 config に削除対象の provider 欄が残る場合は設定エラーにし、読み替えや警告だけの互換動作を実装しない。account 上限の永続データは source/account の識別子が一意に対応するものだけを移し、対応不明な値を推測で割り当てない。

## CoS の経路

CoS chat の harness は `claude_code` に固定する。source/model は `[cos.route_candidates]` に明示した優先順の候補から、通常 task と同じ適格性確認・account admission で選ぶ。代替 source は task 01M4E0WA5Z で定めた fallback を候補に含めて維持する。例えば次の候補は Claude OAuth を優先し、Codex OAuth を fallback にする。CoS 設定に provider ID は置かない。

```toml
[cos]
harness = "claude_code"
route_candidates = [
  { source = "claude_oauth", model = "claude-opus-5-5" },
  { source = "codex_oauth", model = "gpt-6-astra" },
]
```

CoS の account 選択も source/account の quota・cooldown・同時実行数を使い、provider snapshot や `in_use_cos` を別の容量台帳として残さない。CoS 候補が空またはすべて不適格なら fail closed とする。

## llm-proxy の位置

`llm-proxy` は harness ではなく、source に対する接続・認証・quota の実行面と、source API 形式から harness が必要とする API 形式への変換器である。proxy 自身を LLM source として数えない。source ID は上流の実体（例 `qwen`、`claude_oauth`）、model ID はその source の model とする。`[llm_proxy.models]` の tier alias（例 `celeris/cheap`）は移行期間を設けず削除する。langmem・LDR・paperqa の設定には具体的な source/model を書く。

`llm-proxy` 内の `[sources.*]` 接続情報は proxy の接続設定に属する。Celeris の canonical source registry は source kind、account、quota、同時実行上限、利用可能 model を持ち、proxy はその source ID/model ID を参照する。両者の重複設定を避け、proxy に tier alias を残さない。変換 route は harness の入力形式と source の出力形式の対応を宣言し、認証・account 使用量・cooldown は常に上流 source/account に計上する。

## API と画面

`/api/v1/providers` と provider CRUD/check API、`/providers` web route・画面、および旧 `gui/` の同じ provider 画面を削除する。provider ID を含む API 型・OpenAPI・リンク・ヘルプ文も同時に消す。新 API は責務別に置く。source CRUD/catalog/health は `/api/v1/llm/sources`、account CRUD・quota・cooldown・上限は `/api/v1/accounts`、harness 宣言と能力は `/api/v1/harnesses` とし、実行時の選択結果は run detail/event の読み取り API で参照する。source と harness の編集画面は別にし、task の実行条件表示では選ばれた harness と source/model を並べて示す。

`model_assignments` は provider/harness の代替選択表にしない。source×model の利用可否・人が設定した model 制約は source/model catalog として表現し、harness 適格性は harness 宣言で判定する。既存 assignment API/画面はこの二つの正本へ分解して削除する。routing shadow は実行選択を変更せず候補・適格性・除外理由・仮決定を観測する読み取り専用機能として維持できるが、shadow のキーは provider ID でなく task/run・harness・source/model とする。旧 provider に紐づく shadow 設定と UI は移行時に削除する。

## 選択の記録

run の永続レコードに `selected_harness_id`、`selected_source_id`、`selected_model_id`、選択方式/根拠の versioned metadata を記録する。source/account の実際の admission 結果は account ID も記録する。dispatcher は選択時に `harness_selected` と `llm_route_selected` event を追記し、候補・制約適用後の決定・選択理由を構造化 payload として残す。失敗した場合も拒否理由を event に残す。events は追記専用であり既存 event payload を書き換えない。

run 欄の追加と既存データの backfill は schema migration で行う。既存 run の provider ID から harness/source/model を推測して埋めず、旧 run は選択詳細を `legacy_unresolved` として表示する。events も削除・再生成しない。新規実行の migration 後に新しい欄と event を必須にする。account limit の複合 key は `(source_id, account_id)`、quota/cooldown の key は `(source_id, account_id)`（source 全体の制限は account ID を持たない source key）、同時実行 reservation も同じ所有者単位とし、provider ID に依存する stats/cooldown cache key を廃止する。

## routing の境界

後続 task 01M4M8D4AQ は評価に基づく policy を差し替える。dispatcher との契約は次の純粋な境界とし、dispatcher に LLM 呼び出しを入れない。

```rust
struct RouteRequest { task_id: TaskId, harness: HarnessId, model_hint: Option<ModelId>, lane: Option<Lane> }
struct Candidate { source: SourceId, model: ModelId, api_format: ApiFormat }
struct CandidateRejection { candidate: Candidate, reason: ExclusionReason }
fn enumerate_candidates(req: &RouteRequest, registry: &Registry) -> Vec<Candidate>;
fn constrain_for_harness(candidates: Vec<Candidate>, harness: &HarnessSpec) -> Vec<CandidateRejection>;
fn admit_source(candidate: &Candidate, accounts: &AccountSnapshot, quota: &QuotaSnapshot,
                reservations: &ReservationSnapshot) -> Admission;
trait RoutePolicy { fn choose(&self, eligible: &[Candidate]) -> Result<Candidate, NoEligibleCandidate>; }
fn default_route_policy(eligible: &[Candidate], priority: &[CandidateKey]) -> Result<Candidate, NoEligibleCandidate>;
```

Enumeration yields only declared source×model pairs. Harness compatibility is applied before account admission; admission checks account/source quota, cooldown and simultaneous execution limits and returns a reservation owner. `RoutePolicy` is the sole replaceable decision point for task 01M4M8D4AQ. The default route policy preserves the order declared in `route_defaults.<harness_id>.candidates`; it chooses the first eligible candidate and fails closed when no declared candidate is eligible. The harness selection result is an input and is not changed by source routing. Explicit task model hints filter the set before policy choice. `lane` remains in `RouteRequest` but does not influence this source/model choice; task 01M4M8D4AQ decides whether a later policy consumes it.

## 本番 config の移行

本番設定の直接読み取り・変更はこの ADR 作成では行わない。移行はバックアップを取った上で一回の設定更新として実施し、新 loader で検証してから daemon を reload する。旧 provider 行、`providers.d`、provider API の値を残したまま新形式へ読み替える互換期間は設けない。DB/events/account credential は設定更新に含めず、migration は transaction で run の新欄と account-limit key を追加する。credential ファイル・account login state・既存 event は移動・削除・再発行しない。

| 現行行 | 新しい source/model 表現 | harness 表現・task 選択 | 継続する用途 |
|---|---|---|---|
| `claude-pool` | `claude_oauth` source、既存 Claude accounts と各 account quota/上限、利用可能な Claude models | `claude_code` harness。coding execution class の候補 | Claude による coding |
| `codex-pool` | `codex_oauth` source、既存 Codex accounts と models | `codex` harness。coding execution class の候補 | Codex による coding |
| `opencode-qwen` | `qwen` OpenAI-compatible source と実 model ID | `opencode`/ACP harness。適格性を宣言 | Qwen の軽量仕事 |
| `pi-qwen` | 同じ `qwen` source/model を共有 | `pi` harness。適格性を宣言 | Qwen の軽量仕事 |
| `pi-opencode-go` | `opencode_go` source とその catalog model IDs | `pi` harness。適格性を宣言 | OpenCode Go を使う仕事 |
| `ldr-qwen` | LDR 設定に具体的な source/model を明記 | `local_deep_research` harness を research genre/capability から選択 | Web 調査 |
| `paperqa-qwen` | paperqa 設定に具体的な source/model を明記 | `paperqa` harness を literature/research 文脈から選択 | 学術文献調査 |
| `langmem-main` | langmem 設定に具体的な source/model を明記 | `langmem` harness を記憶整理 task から選択 | LangMem の記憶整理 |

この対応は harness ごとの固定 provider 行を作り直すものではない。同一 `qwen` source は複数 harness から共有でき、候補適格性は harness 宣言で制約する。本番で現在使用中の model ID/account 名/quota は移行時に既存 config と DB から照合し、source/account の stable ID を保つ。設定検証が通り、CoS が `claude_oauth` で起動できることと各用途の route が適格であることを確認後に reload する。失敗時は保存した config backup を戻して旧版 daemon を再起動する手順を運用 runbook に記録する。

## 2026-10-11 付記: runtime 実装との突き合わせ

runtime の決定は実装済み。closeout HEAD `a3c63b8a` で以下を確認した。

| 決定 | 実装の状態 | 確認箇所・証拠 |
|---|---|---|
| provider 選択を廃止し、task の harness と source×model route を別々に選ぶ | 実装済み。通常 run と reviewer run は `select_route` を使い、旧 provider selector / policy と adapter hint を撤去 | `dispatch-select.md`、`run-switch.md`、`review-switch.md`、`prune-select.md`、`prune-policy.md`、`hint-drop.md`。workspace test-parallel pass |
| 選択結果を run と event に記録する | 実装済み。task-core migration と dispatcher の route 選択・記録を追加 | `store-migrate.md`、`dispatch-select.md`。schema_matches pass |
| account 上限・quota・cooldown・同時実行を source/account 所有にする | 実装済み。CoS と暗黙 account も AccountBook の同じ admission に統合 | `accounts-source.md`、`cos-route.md`、`e2e-api.md` |
| config の provider 宣言を拒否し、新宣言例を使う | 実装済み。`[[providers]]` / `providers.d` の拒否、examples 読み込み試験を含む | `config-drop.md`、`examples.md` |
| API・snapshot・catalog の provider 概念と proxy の tier alias を除去する | 実装済み | `api-unprovider.md`、`snapshot-unprovider.md`、`catalog-unprovider.md`、`proxy-concrete.md` |
| task-worker と celerisctl が選択済み harness/source/model を受ける | 実装済み。worker protocol schema も更新 | `worker-harness.md`、`remove-integration-fix.md`。schema_matches pass |
| lane/Tier を provider/model の対応付けに使わない（legacy tier の焼き付けを除去） | 実装済み。llm-proxy の tier alias に続き、task-worker の tier→model 束縛（`TieredAdapter`・`WorkerAdapter` の tier 別 model / effort の口）と task-core の束縛型・解決・割り当ての束縛への重ね合わせを撤去。harness は route の model だけを `with_model` で受ける | `progress/2026-10-11-llm-source-harness-runtime/tier-drop-operator.md`。workspace test-parallel pass |

最終確認（closeout、HEAD `a3c63b8a`）: `TMPDIR=/local/x bash scripts/dev/test-parallel.sh` は exit 0。4900 passed、0 failed、14 ignored。`cargo clippy --workspace -- -D warnings` は exit 0。TMPDIR を長くすると unix socket の SUN_LEN 上限で試験が落ちるため、短い TMPDIR を使った（環境の注意で、branch の問題ではない）。これに伴い状態を「決定（実装済み: runtime）」にした。詳細は progress の closeout 節。

### 残る別工程

- 画面側の provider UI / 旧型追随は ui-v2 の対象。
- 新しい source/account/harness API は api-v2 の対象。
  - 2026-10-12 付記（api-v2 の状態）: 実装済みは `GET /api/v1/harnesses`（一覧・詳細）、`GET /api/v1/llm/sources/{source_id}`、run detail の `selection`（旧 run は `legacy_unresolved`）、model role assignments の削除（migration 0069）、web・gui の生成型の再生成。accounts の HTTP 路の `(source_id, account_id)` 化は 2026-10-12 accounts-rekey で実装済み（付記は下の「2026-10-12 付記（accounts-rekey）」）。`llm/sources` の catalog 欄（model 一覧）は未実装で、この表の外の残りとして扱う（`llm/sources` の health は既存の `reachable`・`deployments` 欄で一部だけ満たす）。詳細と次の unit の提案は `agent-docs/progress/2026-10-11-llm-source-harness-api.md`。
- reasoning effort は selected route に載っていない。tier 束縛の撤去で、codex へ effort を渡す経路（`-c model_reasoning_effort`）も消え、routing 監査の `reasoning_effort` は新しい run では常に空になる。route に effort を持たせるかは別の決定として扱う（`tier-drop-operator.md` の提案）。
- 本番 config の `[[providers]]`・`providers.d`・`[llm_proxy.models]` を新形へ書き換え、account_limits の移送残行と `harness_migration_unresolved` event を確認する作業は人が実行する。本番 host・DB の変更はこの closeout では行っていない。


### 2026-10-12 付記（Legacy routing の撤去）

実行時の source・model 選択は `task_core::llm_route::route_select` だけが担う。`model_routing` は shadow による観測、または後続 task `01M4M8D4AQ` の入力として扱い、実行選択の互換経路にはしない。`RoutingMode::Legacy` は存在しない。`[model_routing]` を設定する場合は `mode = "shadow"` または `mode = "enforce"` を明示する。設定表そのものを省略した場合は routing 無効として従来どおり読み込める。


### 2026-10-12 付記（accounts-rekey）

Accounts HTTP API は source と account を別々の path key にする。全 source の一覧は `GET /api/v1/llm/accounts`、作成は `POST /api/v1/llm/sources/{source_id}/accounts`、個別操作は `/api/v1/llm/sources/{source_id}/accounts/{account_id}` とその `check`・`login`・`login/code` suffix を使う。旧 `/api/v1/accounts` 路および `?adapter=` は残さない。

managed account pool を持つ `claude_oauth` / `codex_oauth` / `opencode_go` source は、それぞれ内部の ClaudeCode / Codex / OpencodeGo adapter に固定対応する。内部 effect・store key は従来の adapter key を保つ。暗黙 account の公開 `account_id` は `source_id` そのものとし、上限の内部 key は `implicit` と source ID の組を保つ。

この付記により本文 177 行の旧 `/api/v1/accounts` 記述を更新する。


### 2026-10-12 付記（planner・reviewer の選択）

計画 run（planner）と reviewer run は、固定の adapter ではなく execution class の候補列で harness を決める。実装は `wu/class-select`（0a3bf4c5）と `wu/adapter-purge`（64789ef9）。

**class の付け方。** `dispatch_run.rs` は planner 起動の `RouteInput` の harness 要求に `execution_class = "planner"` を付け、`review_spawn.rs` は reviewer の要求に `"review"` を付ける。それ以外の run は class を持たない。

**harness の候補。** `task_core::llm_route::harness_candidates` が候補列を返す。
- `[harness_selection.execution_class_defaults]` に class の宣言がある場合: その列を宣言順のまま返す（登録済みの harness ID だけ残す）。task の `harness` 欄と genre・role の規則は見ない。
- 宣言が無い場合: 従来どおり `select_harness`（task の harness 欄、genre・role の規則）が 1 件を返す。組み込みの adapter 既定は置かない。

**admission。** `route_select::route_select` は候補 harness を先頭から順に試す。各 harness について execution class を外し、`route_select_one` を呼ぶ。`route_select_one` は次の順で判定する。
1. `route_defaults.<harness>` が無ければ `NoDeclaredDefault` で、その harness は落ちる。
2. task が model を指定していて候補に無ければ `ModelHintUnmatched` で落ちる。
3. `route_defaults.<harness>.candidates` の source×model を enumerate し、account admission（quota・cooldown・同時実行数）を通した最初の候補を `RoutePolicy`（既定は宣言順）で選ぶ。全候補が不適格なら `NoEligible`。

最初に成功した harness の (harness, source, model, account) で起動する。

全候補の harness が落ちた場合の規則:
- 候補のどれかが `NoEligible` なら、その `NoEligible` を返す。末尾の候補が `NoDeclaredDefault` などでも、先に `NoEligible` の候補があれば defer になる（2026-10-12 `wu/class-tests`（db30c4fe）で直した。直す前は末尾の失敗理由が返り、defer にならなかった）。
- `NoEligible` は「今は埋まっている」なので run は起動せず defer する（失敗にしない）。
- どの候補も `NoEligible` でなければ、最後に試した harness の失敗を返す。`NoDeclaredDefault`・`ModelHintUnmatched`・未登録の harness は設定を直すまで解けないものとして、既存どおり unroutable に数える。

**記録。** 選択の結果は次の 3 箇所に残す。
- `harness_selected` event は、成功時は選ばれた harness、失敗時は最後に試した harness を記録する。
- `llm_route_selected` event と `runs.selection_metadata_json` は、採用された (harness, source, model, account) と `metadata`（`harness_rejections` を含む）を記録する。
- 失敗時は `worker_progress`（`routing`）の `route refused` に理由と metadata を残す。

`harness_rejections` は試した harness ごとに `{harness, reason, rejections}` を持つ。`rejections` は、その harness の各 source×model 候補が落ちた理由（quota・cooldown・同時実行など）。どの harness が、どの候補の理由で落ちたかを後から追える。

**config。** `[execution.planner] adapter` は撤去した。この欄を書いた config は読み込みエラーになり、`[harness_selection.execution_class_defaults] planner = [...]` の書き方を案内する（`crates/celeris/src/config/mod.rs`）。読み替えや警告だけの経路は無い。`PlannerConfig.adapter` と `default_planner_adapter` も消した。

```toml
# 削除: [execution.planner] adapter = "claude-code"
[harness_selection]
rules = [
  { genre = "coding", required_capabilities = [], requires_repo = true, harness = "pi" },
]
# 優先順。claude_code が埋まると pi へ回る。両方埋まれば planner run は defer する。
execution_class_defaults = { planner = ["claude_code", "pi"], review = ["claude_code"] }

[route_defaults.claude_code]
candidates = [{ source = "claude_oauth", model = "claude-sonnet-5-5" }]

[route_defaults.pi]
candidates = [{ source = "opencode_go", model = "opencode-go/kimi-k3" }]
```

`config/celeris.pi-hashline.example.toml` が同じ書き方の例である。

## 実装突き合わせ付記（2026-10-12、本番移行 runbook）

本番移行例は [provider/source/harness 移行手順](../../docs/ops/provider-llm-source-migration.md)
と [進捗記録](../progress/2026-10-11-provider-layer-redesign.md) に記載する。確認した本番
8 行の model ID は Claude `claude-fable-5-1` / `claude-opus-5-5` / `claude-sonnet-5-5`、
Codex `gpt-6-astra` / `gpt-6.1-sol` / `gpt-6-luna`、Qwen `qwen3.8-27b`、Pi の
OpenCode Go `opencode-go/grok-4.6` である。Pi の先頭候補は OpenCode Go、その後ろに Qwen
を置く。coding standard は Claude Opus を先頭、Codex 6.1 を次にする。

CoS は harness を `claude_code` に固定し、standard の `claude-opus-5-5` を先頭に、task
01M4E0WA5Z の Codex OAuth `gpt-6-astra` fallback を続ける。Codex→Claude Code の候補は
`llm_routes` に API 変換を明記する。planner/reviewer は `claude_code`、`codex` の順で
admission し、どちらも空いていない場合 defer する。Qwen の軽量仕事は `local-qwen` role
から ACP/OpenCode harness を選ぶ。

旧 proxy tier alias を撤去し、LDR/LangMem の cheap は利用可能な Qwen を先頭に Claude/Codex
cheap fallback を明記する。PaperQA の standard は Claude Opus、次に Codex 6.1 とする。
Runbook TOML を `crates/celeris/src/config/tests.rs::migration_runbook_toml_loads` が読み、
harness ごとの先頭候補、CoS の候補順、planner の harness 順と進捗内 TOML との一致を検証する。
本番移行後の daemon、DB、credential の変更は本 task の範囲外で、人が別途実行する。

**宣言の検証（読み込み時）。** `validate_registry`（`crates/celeris/src/config/llm_route.rs`）は execution class の各候補 harness について次を確かめ、違えば読み込みエラーにする。
- `[harnesses.<id>]` で宣言されている（未宣言は既存の検証で拒否）。
- `[route_defaults.<id>]` を持つ。持たないと admission で必ず `NoDeclaredDefault` になるため、読み込みで止める（試験 `execution_class_harness_requires_route_defaults`）。
- 同一 class 内に同じ harness を重複させない。

**試験。** `crates/task-dispatch/src/dispatcher/tests/class_route.rs` は注入した `RouteState` で `route_select` を呼ぶ（LLM・外部 network なし）。(a) claude_code が埋まり codex が空き → codex、(b) 両方埋まり → `NoEligible`（defer）、(c) review も同じ、(d) 宣言なし → genre・role の従来選択で、落ちた harness を `harness_rejections` に残す、(e) 末尾の候補が未宣言でも先頭の `NoEligible` で defer する。

**後続 task 01M4M8D4AQ との境界。** execution class の候補列は harness の選択入力である。source と model の選択は変えない。source×model の方針は `RoutePolicy` だけが持ち、今回の候補列は `RoutePolicy` に入らない。`RouteRequest` の `lane` は保持するが、この選択では使わない。後続 task が `RoutePolicy` を差し替えても、harness の候補列と admission の規則は変わらない。

## 2026-10-12 付記（ui-v2）

web/ の provider 画面を削除し、source・account・harness の画面を置いた。旧 `gui/` の `/providers` 画面と provider API の呼び出し・試験・fixture も消した。

実装した画面（すべて読み取り専用。`web/` の route）:
- `/sources`: LLM source の一覧（種別・到達可否・deployment 数・account 数）。
- `/sources/$sourceId`: source の詳細。account は `/accounts` の該当行へ、model は `/models?source=<id>` へ結ぶ。
- `/harnesses`: harness の一覧（宣言された形式・source 種別・能力・既定・適格条件）。設定の変更は config で行う旨を help に書く。
- `/tasks/$id/runs/$runId` の run ヘッダと task 詳細の run 一覧: 選ばれた harness・source（`/sources/$sourceId` へのリンク）・model・口座を並べて出す。旧 run（`selection.status = legacy_unresolved`）は `記録なし（旧 run）` と出し、adapter や model で補わない。

`/providers` は削除済み。`/accounts`・`/models`・`/sources`・`/harnesses` の 4 画面はこの ADR の「API と画面」の source・account・harness の分離に従う。

残り（この付記の範囲外。次の unit の提案は `agent-docs/progress/2026-10-12-web-source-harness-ui.md`）:
- source と harness の編集 API と画面。ADR の「source と harness の編集画面は別にし」は未実装で、現状の画面は読み取りだけ。編集は config に残る。
- `llm/sources` の catalog 欄（source が持つ model 一覧と利用可否）は未実装。`/sources/$sourceId` の model 欄は既存の deployments 欄を使い、`/models?source=` へ結ぶ。
- run の `adapter`・`model` 欄（`RunSummary`）と timeline・console の `worker_started` ラベルは、まだ adapter 由来の値を読んでいる。選択結果の `selection` だけを表示源にする整理は別 unit。

### 2026-10-12 付記（ui-v2 の差し戻し修正）

final review の差し戻し（画面に残る旧 provider 表示）を直した。`crates/` は変えていない。

旧 `gui/` の画面は、run の選択を `RunSummary.selection`（harness・source・model・account）から出すように置き換えた:
- run 一覧（`gui/app/components/task-detail/OverviewTab.tsx`）の `adapter`・`provider` 列を `harness`・`source` 列に。旧 run（`legacy_unresolved`）は「記録なし（旧 run）」。
- run 詳細（`gui/app/routes/tasks.$id.runs.$runId.tsx`）の見出しを `harness / source / model / 口座 account` に。
- routing パネル（`gui/app/components/TaskRoutingPanel.tsx`）の「選択」行を、同じ run の `selection` から出す。`run.provider` の表示は消した。
- daemon（`gui/app/routes/daemon.tsx`）の in-flight 列は `source`、cooldown 列は `source / account`。
- accounts（`gui/app/routes/accounts.tsx`）の provider 分岐と存在しない目次項目を消した。
- help（`gui/app/routes/help.tsx`）の用語を `harness`・`LLM source` の 2 項に置き換えた。
- web の routing 監査表示（`web/features/tasks/routing-audit-view.tsx`）の実行枠行は `harness / source / model / 口座`。harness が無い run は「harness 不明」。

API の欄名 `provider`（`RunSummary.provider`・`LaneResolution.provider`・`InFlight.provider`・`RunRoutingAudit.provider`）は、crates/ を変えないこの task では残した。中身は source id。画面では欄名を出さず、source の語で出す。

後続の提案（別 unit、crates を変える）:
- 上の欄名を `source` へ改名する。破壊的変更の方針に従い、改名と同じ変更で旧欄名を消す（互換の別名を残さない）。生成型の再生成（`UPDATE_SCHEMA=1`、gui・web の `gen:types`）を含める。
- `RunSummary.adapter` と `RunRoutingAudit.adapter` は harness が入った後は同じ情報の重複なので撤去する。`TimelineTab`・`console.ts` の `{adapter} / {model}` 表示も `selection` から出す。

残り（provider の語を残すが LLM provider の表示ではないもの）:
- `gui/app/components/BrowserWaitsPanel.tsx` の `credential.provider` は credential の provider で、LLM provider ではない。
- `gui/app/components/task-detail/TimelineTab.tsx` の `provider_throttled` は event 種別の一覧（生成型の Event enum に残る）。
