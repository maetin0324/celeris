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

CoS chat の harness は `claude_code` に固定する。source/model は `[cos.route_candidates]` に明示した候補から、通常 task と同じ適格性確認・account admission で選ぶ。候補は `claude_oauth / claude-opus-5-5` の 1 件だけとする。Codex を CoS の候補に置かないのは、`claude_code` が proxy endpoint を宣言できず、Codex OAuth への経路を持たないためである。CoS 設定に provider ID は置かない。

```toml
[cos]
harness = "claude_code"
route_candidates = [
  { source = "claude_oauth", model = "claude-opus-5-5" },
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

CoS は harness を `claude_code` に固定し、standard の `claude-opus-5-5` の 1 件だけを候補にする（Codex へは落とさない）。Codex→Claude Code の候補は
`llm_routes` に API 変換を明記する。planner/reviewer は `claude_code`、`codex` の順で
admission し、どちらも空いていない場合 defer する。Qwen の軽量仕事は `local-qwen` role
から ACP/OpenCode harness を選ぶ。

旧 proxy tier alias を撤去し、LDR/LangMem の cheap は利用可能な Qwen を先頭に Claude/Codex
cheap fallback を明記する。PaperQA の standard は Claude Opus、次に Codex 6.1 とする。
Runbook TOML を `crates/celeris/src/config/tests.rs::migration_runbook_toml_loads` が読み、
harness ごとの先頭候補、CoS の候補（Claude の 1 件）、planner の harness 順と進捗内 TOML との一致を検証する。
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

## 2026-10-13 付記（本番投入の不具合）

provider 廃止版の本番 verify で見つかった 5 件を、次の契約で修正する。既存の provider 廃止・source/harness 分離の決定は変えない。proxy に渡す wire model の source 接頭辞と OpenCode の provider namespace は別の名前空間である。前者は `llm_routes.via`、後者は `[harnesses.opencode].model_namespace` だけから決める。

### 1. 旧 `routing_decided` event の読み込み — `legacy-events`

**決定**: `RoutingMode::Legacy` を durable event の deserialize のみで受ける互換 variant として戻す。新しい設定・policy・dispatch・event serialize 経路からは選べないようにし、新規 event は引き続き `shadow` / `enforce` のみを書く。独立 parser や event migration は作らず、通常の `Event` deserialize で読む。

**理由**: `RoutingDecided.record.optimizer.mode = "legacy"` は旧 selector が残した監査値で、現行設定の選択肢ではない。`tasks_with_events_impl` の `serde_json::from_str::<Event>` が `RoutingMode` の未知 variant で失敗し、store から全 task/event を読む daemon bootstrap に伝播して起動を止める。

provider 削除履歴を調べるときは、provider 関連 commit の名前だけで互換対象とせず、各削除 diff の serde `Deserialize` 型を Event variant/payload から包含関係で追い、保存済み event の実例、append site と現行 reader を照合する。`WorkerHint.adapter` は `Created.task` 内に残り得るが未知 struct field は無視される。runtime config/catalog 型の削除は event payload 型でない。したがって現時点で追加互換が必要なのは `RoutingTraceV1.mode` の `legacy` である。

**変更箇所（file:関数）**: `crates/task-core/src/model_router/policy.rs:RoutingMode`; `crates/task-core/src/model_router/trace.rs:RoutingTraceV1`; `crates/task-core/src/store/events.rs:events_for_impl`・`tasks_with_events_impl`（reader の回帰固定）。daemon bootstrap からの伝播も確認する。

**実装 leaf と試験名**: `legacy-events` — `task_core::model_router::tests::legacy_routing_event_deserializes_without_becoming_writable`; `task_core::store::tests::legacy_routing_decided_event_loads_from_store`; `celeris::legacy_event_startup::startup_loads_legacy_routing_decided_event`（実体: `crates/celeris/tests/legacy_event_startup.rs`）。

### 2. Pi の実行宣言 — `pi-harness`

**決定**: `[harnesses.pi]` に `extensions: Vec<PathBuf>`、`tools: Vec<String>`、`env: HashMap<String, String>` を持たせる。extensions は設定ファイルからの相対 path とし、extensions/tools が空なら設定を拒否する。旧 `pi-*` provider 宣言の値を移す。env は既存の secret reference 解決を通して Pi process に渡し、route が足す account directory・credential・endpoint env は宣言 env を保持して合成する。値が本番 backup にしかない項目は推測せず、運用者が既存値を照合する。

**理由**: provider 行を廃止した後も Pi adapter は `-e` extensions、`--tools` allowlist と process env を要する。現 harness builder が空値を渡し、Pi 側 validation が起動を拒否する状態を解消する。

**変更箇所（file:関数）**: `crates/celeris/src/config/harness.rs:HarnessConfig`・検証/path 解決; `crates/celeris/src/daemon/adapters.rs:build_harness_adapters`; `crates/task-worker/src/route.rs:route_adapter`; `crates/task-worker/src/pi.rs` の起動 env/args。

**実装 leaf と試験名**: `pi-harness` — `celeris::config::tests::pi_harness_declaration_deserializes_extensions_tools_and_env`; `celeris::daemon::adapters::harness_adapter_tests::pi_harness_declaration_reaches_pi_config`; `task_worker::pi::tests::pi_adapter_args_tools_isolation_cwd_and_large_prompt`（run 側で `-e` extension と `--tools` が渡ることを確かめる）; `celeris::daemon::adapters::harness_adapter_tests::pi_harness_declaration_reaches_pi_process`（daemon の `build_harness_adapters` で得た pi adapter に `run` を通し、stub の pi が受け取った argv に宣言の `-e`・`--tools`、環境に宣言 `env` と `env_from_secrets` の値が入ることを確かめる。stub は一時 directory に置き、外部ネットワークと本物の pi は使わない）。**未作成（実装の食い違い）**: 相対 extension path の TOML 基準解決（`config/mod.rs` の extensions 解決）を名前付きで確かめる試験は、まだ無い。試験の追加は次の WU の範囲。

### 3. proxy harness の model wire 名 — `model-names`

**決定**: `via = "llm_proxy"` の明示 route では harness に `<source_id>/<model_id>` を渡す。直接接続は model ID のまま。変換は daemon の route 選択時に一度だけ行い、選択結果の wire model を adapter 起動と `SelectedLlmRoute.model_id` の双方で使う。event/run の canonical model は接頭辞なしの source model ID のまま保つ。proxy route の endpoint は source endpoint を渡さず `None` とし、proxy URL と bearer secret は各 harness の接続設定に置く。

LDR は proxy 経由のまま維持する。Claude/Codex source へ fallback しても route source に応じて wire name だけを替える。PaperQA は `settings.llm`、`settings.summary_llm`、`settings.agent.agent_llm` の3欄を wire name で上書きし、検索語 LLM にも同じ名前を渡す。LangMem は `with_model` で route model を使う。LDR は既存の route model override を使う。

**理由**: proxy parser は最初の `/` までを source ID として解決する。現在は選択 model が裸のまま渡り、proxy 経由と宣言しても 422 になる。PaperQA の `strip_provider_prefix` は wire 名の source 部分を落とし、要約/agent LLM は settings file の古い model を残していた。LangMem も route の model を無視していた。registry を持つ daemon で変換すれば worker と daemon の二重変換を避けられる。

**変更箇所（file:関数）**: `crates/task-core/src/llm_route.rs:wire_model_id`・`route_via`; `crates/task-dispatch/src/dispatcher/route_select.rs:route_select_one`; `crates/task-dispatch/src/dispatcher/route_launch.rs:Dispatcher::route_adapter`・`selected_llm_route`; `crates/celeris/src/config/llm_route.rs:validate_proxy_routes`; `crates/task-worker/src/paperqa.rs:query_llm_model`・`strip_provider_prefix`; `crates/task-worker/src/paperqa_ask.py` の settings 上書き; `crates/task-worker/src/langmem.rs:LangMemAdapter::with_model`。

**実装 leaf と試験名**: `model-names` — `task_core::llm_route::tests::wire_model_id_qualifies_only_routes_declared_via_llm_proxy`; `task_core::llm_route::tests::wire_model_id_keeps_a_model_that_already_contains_a_slash`; `task_dispatch::dispatcher::tests::route_select::proxy_route_pick_carries_the_qualified_wire_model`; `task_dispatch::dispatcher::tests::route_select::direct_route_pick_keeps_the_bare_model`; `task_dispatch::dispatcher::tests::route_launch::route_adapter_launches_the_wire_model`; `celeris::config::tests::proxy_bound_harness_candidates_all_need_a_via_llm_proxy_route`; `task_worker::paperqa::tests::route_model_overrides_llm_summary_llm_and_agent_llm`; `task_worker::paperqa::tests::query_llm_model_keeps_the_source_qualified_name`; `task_worker::langmem::tests::with_model_replaces_the_configured_model`; `task_worker::local_deep_research::tests::route_model_overrides_settings_llm_model`。

**本番への移行**: `docs/ops/provider-llm-source-migration.md` の実装 leaf で、LDR/PaperQA/LangMem の proxy URL・token secret reference・接頭辞付き既定 model、明示 `via = "llm_proxy"` route、稼働中の config backup に基づく Pi 値を反映する。`claude_code` は proxy endpoint を宣言できないため Codex OAuth proxy fallback を候補から外し、CoS は Claude source のみとする。設定検証、`celerisctl routing show`、proxy の接頭辞付き成功/裸 model の 422 を確認してから人が reload する。daemon 本番設定はこの付記作業では変更しない。

### 4. OpenCode ACP model ID — `model-names`

**決定**: `[harnesses.opencode]` に任意の `model_namespace` を宣言し、OpenCode ACP だけ `model_namespace/model_id` を adapter に渡す。今回は namespace を `qwen-local`、route model を `qwen3.8-27b` とし、OpenCode が要求する `qwen-local/qwen3.8-27b` にする。namespace 未指定なら従来どおり route model を渡し、空値または `/` を含む値は拒否する。proxy source prefix から namespace を推測しない。

**理由**: OpenCode config の provider namespace `qwen-local` と LLM source ID `qwen` は異なる識別子である。`<source>/<model>` は proxy wire protocol のための名前であり、OpenCode の model catalog ID を表さないため、設定で明示する。

**変更箇所（file:関数）**: `crates/celeris/src/config/harness.rs:HarnessConfig`・検証; `crates/celeris/src/daemon/adapters.rs:build_harness_adapters`; `crates/task-worker/src/route.rs:route_adapter`; `crates/task-worker/src/acp.rs` の `with_model` から session model option への引き渡し。

**実装 leaf と試験名**: `model-names` — `celeris::config::harness::tests::opencode_model_namespace_deserializes_and_validates`; `celeris::daemon::adapters::tests::opencode_harness_passes_model_namespace`; `task_worker::route::tests::opencode_model_id_uses_declared_namespace`; `task_worker::route::tests::non_opencode_model_id_remains_route_model`; `task_worker::acp::tests::opencode_receives_qualified_route_model_id`。

**本番への移行**: 同じ runbook 反映で `[harnesses.opencode].model_namespace = "qwen-local"` を宣言し、route の canonical model は `qwen3.8-27b` のままにする。既存 OpenCode config の provider ID と一致することを確認する。

### 5. execution class ごとの route 候補 — `class-routes`

**決定**: `[route_defaults.<harness>].classes.<execution_class>.candidates` を追加し、`coding` class だけ `claude-opus-5-5` を候補先頭にする。`[route_defaults.claude_code].candidates` の通常既定は `claude-sonnet-5-5` を先頭にする。class 候補が宣言された場合はその候補列で置き換え、全候補不適格なら `NoEligible` として fail closed する。lane/tier と `RoutePolicy` は使わず、後続 task 01M4M8D4AQ の policy 境界を変更しない。class 名は `coding` / `planner` / `review` に限定し、route 候補と既定候補で同じ適格性検証を行う。

**理由**: 本番要件は coding のみ Opus、通常の Claude Code は Sonnet。harness の execution class 選択と source/model 候補選択は別段階なので、既存 `route_defaults` に class 別の候補表を持たせ、route 選択時に class を保持するのが最小の変更となる。class 候補から既定への暗黙 fallback は宣言順の意味を損なう。

**変更箇所（file:関数）**: `crates/celeris/src/config/llm_route.rs:RouteDefaultConfig`・`validate_registry`; `crates/task-core/src/llm_route.rs:Registry`・`route_priority`; `crates/task-dispatch/src/dispatcher/route_select.rs:route_select`・`route_select_one`（harness loop で class を落とさない）; `crates/celeris/src/config/harness.rs:apply_verify_smoke`; `crates/celerisctl/src/commands/routing.rs` の `routing show`。

**実装 leaf と試験名**: `class-routes` — `task_core::llm_route::tests::route_priority_prefers_the_execution_class_list`; `task_core::llm_route::tests::route_priority_ignores_the_class_for_cos`; `task_dispatch::dispatcher::tests::class_route::coding_class_picks_the_class_candidate_first`; `task_dispatch::dispatcher::tests::class_route::class_list_does_not_fall_back_to_the_harness_defaults`; `task_dispatch::dispatcher::tests::route_select::class_candidates_survive_the_harness_loop`; `celeris::config::tests::route_defaults_classes_build_the_registry`; `celeris::config::tests::class_candidates_are_validated_like_harness_candidates`; `celeris::config::tests::empty_class_candidates_are_a_config_error`; `celeris::config::tests::unknown_execution_class_is_a_config_error`; `celeris::config::tests::apply_verify_smoke_drops_smoke_from_class_candidates`; `celerisctl::commands::routing::tests::show_prints_class_candidates`。

**本番への移行**: 同 runbook の `route_defaults.claude_code.candidates` は Sonnet、`route_defaults.claude_code.classes.coding.candidates` は Opus を先頭にする。planner/review は通常既定を使い、CoS は独立した候補表を維持する。reload 前に `celerisctl routing show` で両候補列を確認する。

### 残る別工程

`claude_code` / `codex` harness 自体を llm-proxy 経由にする機能は今回の決定に含めない。Claude Code の endpoint/auth 注入と daemon 起動経路への適用、proxy bearer secret の引き渡しが揃うまで、該当 fallback は設定しない。必要になった場合は別の設計・実装 task とする。

### 2026-10-13 付記（CoS の Codex fallback 撤去 — 決定 cos-codex）

人の決定 `cos-codex`（2026-10-13）: **(a) CoS は Claude のみ**。CoS の候補に Codex OAuth（`codex_oauth` / `gpt-6-astra`）を fallback として置かない。CoS の候補は `claude_oauth / claude-opus-5-5` の 1 件とする。

1. **置き換え**: 本文（CoS の経路・runbook 付記）を本付記に合わせて書き換えた。書き換えた箇所は次のとおり。
   - 「## CoS の経路」: CoS の候補は `claude_oauth / claude-opus-5-5` の 1 件とし、Codex を候補に置かない理由を書いた。
   - 「実装突き合わせ付記（2026-10-12、本番移行 runbook）」: CoS の文と候補順の検証の記述を Claude の 1 件に合わせた。
   - 本番移行 runbook `docs/ops/provider-llm-source-migration.md`、進捗 TOML、選択結果の表の CoS 行と確認手順: Claude の 1 件に揃えた。
   - 2026-10-13 付記 §3「本番への移行」の「CoS は Claude source のみとする」は本決定と同じ内容で、そのまま有効。

2. **揃える対象**: runbook、進捗 TOML（`agent-docs/progress/2026-10-11-provider-layer-redesign.md` の `[cos]` 区間）、試験 `celeris::config::tests::migration_runbook_toml_loads` の CoS 候補数 assert を Claude のみに揃える。試験は候補数 1 件と、その source / model を `assert` する。

3. **戻す条件**: Codex OAuth を CoS の fallback に戻すには、`claude_code` harness を llm-proxy 経由にする接続が要る（Claude Code の endpoint・auth の注入、daemon 起動経路への適用、proxy bearer secret の引き渡し）。これは別 task の設計・実装とし、本付記の範囲外とする。それまで `claude_code` は proxy endpoint を宣言できないため、Codex の CoS 候補は宣言しない（宣言すると実行時に失敗する候補になる）。

4. **変えないこと**: planner・reviewer・coding の Codex 候補、PaperQA・LDR の Codex 候補は本決定の対象外で、そのまま残る。コードの挙動は変えない（runbook・進捗 TOML・試験の宣言の一致のみ）。

### 2026-10-22 付記（harness の試行順）

2026-10-12 付記の「候補 harness を先頭から順に試す」は、`[route_selection] harness_balance_classes` で有効化したクラスに限り、残量スコアによる並べ替えに置き換わる（[2026-10-22-harness-balance-and-lane-first-routing](2026-10-22-harness-balance-and-lane-first-routing.md) D1。既定は無効で、無効なクラスは本付記までと同じ宣言順）。frontier lane の model 選択も同 ADR D3 が上書きする。
