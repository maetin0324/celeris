---
status: proposed
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
---
# worker の能力の分離: EventSink から Browser の認可・制御・secret 操作を外し、WorkerAdapter の with_* を 1 つの設定にする

- 日付: 2026-11-10
- 状態: 提案（健全性 v2 R2。この ADR ではコードを変えない。実装は同じ task の unit browser-port・adapter-settings・dups、記録は unit record）
- 関係: [ADR-0003 worker protocol](0003-worker-protocol.md)（`EventSink`・`WorkerAdapter`）、[ADR-0080 browser の policy broker と承認](0080-browser-phase2-policy-broker-approval.md)（H3 の auth section、`credential-stays-in-broker`）、[ADR-0100 live proxy](0100-browser-phase3-live-proxy-acl.md)、[ADR-0107 browser fallback 候補の準備](0107-browser-fallback-candidate-preparation.md)（`prepare_run_adapter`）、ADR-0113 D3（control gate）、ADR-0024 D2・ADR-0043 D3・ADR-0072 D14・ADR-0075 D4・ADR-0098 D6（with_* の各経路）、[ADR 2026-10-11 llm-source-harness-selection](2026-10-11-llm-source-harness-selection.md)（`with_llm_endpoint` の fail closed）、[ADR 2026-11-07 健全性 v2 の基準](2026-11-07-health-v2-baseline.md)（R2 の scope・write_set）、[ADR 2026-11-10 adapter の結果契約](2026-11-10-adapter-result-contract.md)（R1）、[ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)

## 背景

`crates/task-worker/src/adapter.rs` の `trait EventSink` は 23 method を持ち、そのうち 10 個が Browser の認可・制御・secret の操作（承認の消費、saved credential の読み出しと再検証、auth section、control gate、live event、lifecycle）である。10 個はすべて既定実装を持つ（`Err`・`None`・何もしない。ただし `browser_saved_credential_valid` の既定だけは許可側の `true`）。そのため:

- 通知・観測しか要らない sink（`ReviewerSink`・`ChatRunSink`・`PrintSink`・`NullSink`）も、型の上では Browser の能力を持つ。
- browser の run に能力を渡し忘れても compile は通り、run が始まってから既定値（`browser wait store unavailable` など）で気付く。`browser_saved_credential_valid` の既定 `true` は fail-open である。
- harness の adapter（claude_code・acp）に渡す `BrowserSink`（`browser.rs`）は、harness が使わない browser_* 7 個を転送している。

`trait WorkerAdapter` の設定の method 5 つ（`with_model`・`with_llm_endpoint`・`with_permission_mode`・`with_env`・`with_container`）は、9 adapter が計 26 個を写し、`BrowserSpecialistAdapter` が 5 個を転送し、試験の adapter 22 個が計 31 個を写す。呼び出し側は「受けられなかったとき」の扱いを match で繰り返す（`dispatcher/worker_task.rs` の `prepare_run_adapter` は同じ形の `with_env` の match を 4 回書く）。

数値は計測台本 [measure.py](../progress/2026-11-10-health-v2-r2/measure.py) の `--json` の出力（基準値は [R2 の進捗](../progress/2026-11-10-health-v2-r2.md)）。

## D1. EventSink に残す method（通知・観測の 13 個）

`EventSink` は `adapter.rs` に置いたまま、次の 13 個だけを持つ。既定実装は今のまま（何もしない・`Ok(())`・`progress` に流す）。どれも能力ではなく観測の通知で、何もしない既定は安全側である。

| method | 種類 | 主な実装（非試験） |
|---|---|---|
| `before_spawn` | 起動の印（`Err` なら spawn しない） | `ChatRunSink` |
| `progress`（既定なし） | 観測 | 全 sink |
| `progress_with` | 観測 | `StoreSink`・`ChatRunSink` |
| `artifact`（既定なし） | 観測 | 全 sink |
| `context_compacted` | 観測 | `BrowserSink`（転送） |
| `heartbeat` | 生存通知 | `StoreSink`・`ChatRunSink`・`BrowserSink` |
| `delegate` | 提案の通知 | `StoreSink`・`PrintSink` |
| `comment` | 通知 | `StoreSink` |
| `usage_progress` | 観測 | `StoreSink`・`ReviewerSink` |
| `rate_limit` | 観測 | `StoreSink`・`ReviewerSink`・`BrowserSink` |
| `policy_violation` | 観測（警告） | `StoreSink`・`ReviewerSink` |
| `session_established` | 観測 | `StoreSink`・`ReviewerSink`・`ChatRunSink`・`BrowserSink` |
| `session_resume_failed` | 観測 | `StoreSink`・`ReviewerSink`・`ChatRunSink`・`BrowserSink` |

B0 の `consolidate_into_owner` は「通知・観測は `crates/task-core/src/notify.rs` を正本にする」と書く。`notify.rs` は「人の判断が要る出来事を 1 回だけ知らせる」重複排除の台帳（ADR-0037）で、worker の run から dispatcher への event の口である `EventSink` とは意味が違う。見た目が似ているだけのものを 1 つにしない（汎用規約 8）ので、`EventSink` を `notify.rs` へ移さず、R2 は `notify.rs` を変えない。

## D2. Browser の能力の trait 2 つと注入の経路

### 2a. trait（既定実装なし）

| trait | 置く file | method（署名は今の `EventSink` の同名 method と同じ） | 何の能力か |
|---|---|---|---|
| `BrowserAuthority` | `crates/task-worker/src/browser_policy.rs`（B0 が Browser の認可の正本に挙げた file） | `browser_wait_open`・`browser_waits`・`browser_saved_credential`・`browser_saved_credential_valid`・`browser_approval_consume`・`browser_operation_approval_consume`（6 個） | 承認の待ちの開設・読み出し・1 回だけの消費、daemon が持つ saved credential（secret）の読み出しと再検証 |
| `BrowserControl` | `crates/task-worker/src/browser_live.rs`（`ControlGate`・`LiveSink` の定義がある file） | `browser_updated`・`browser_auth_section`・`browser_control_gate`・`browser_live`（4 個） | session の lifecycle の記録、auth section の開閉、control gate の取得、scrub 済み live event の記録 |

- 両 trait とも `Send + Sync` を supertrait にし、既定実装を持たない。`browser_saved_credential_valid` の既定 `true` は消え、実装が必ず判定を書く（fail-closed を強める方向。弱めない）。
- method の名前は `browser_` を付けたまま変えない。呼び出し側の変更が受け手（`sink.` → `browser.authority.` など）だけになり、`git grep browser_saved_credential` などの追跡が続く。
- 2 つに分ける理由: `BrowserAuthority` は secret（saved credential）と承認を扱い、`BrowserControl` は secret を扱わない。live event の出口（`EventSinkLive`・`forward_events`）と control gate の取得だけを要る処理は `&dyn BrowserControl` だけを受け取り、secret に届かない。1 つの trait（10 method）にすると、この区別が型に出ない。
- 2 つを束ねる supertrait（facade の trait）は作らない。注入には参照の組を使う:

```rust
// crates/task-worker/src/browser_policy.rs
#[derive(Clone, Copy)]
pub struct BrowserPorts<'a> {
    pub authority: &'a dyn BrowserAuthority,
    pub control: &'a dyn BrowserControl,
}
```

### 2b. 注入の経路

1. `StoreSink`（`crates/task-dispatch/src/dispatcher/sinks.rs`）が `BrowserAuthority` と `BrowserControl` を実装する。中身は今の `impl EventSink for StoreSink` の browser_* 10 個をそのまま移す（`credential_site` の利用、`browser_wait_open` の error の code 化、`StoreGate` の生成を含む）。`ReviewerSink` は実装しない。
2. `run_worker`（`crates/task-dispatch/src/dispatcher/worker_task.rs`）は、`task_core::browser::requests_browser(&req.task.skills)` が真の run にだけ `Some(BrowserPorts { authority: &sink, control: &sink })` を作り、それ以外は `None` を渡す。判定は同じ関数の直前にある「remote・container の browser run を拒否する」条件と同じ述語を使い、新しい条件を足さない。
3. `task_worker::browser` の入口（`run`・`run_with_candidates`・`run_with_executable`・`run_with_executable_record`・`run_with_executable_candidates_record`）は引数 `browser: Option<BrowserPorts<'_>>` を受ける。`run_with_executable_attempt` は、今の「browser の run でなければ `adapter.run` へ素通り」の判定（`requests_browser`・Execute・planner でない・review でない）の直後、`browser_adapter`・policy・runtime・harness のどれよりも前に、`browser` が `None` なら `Err(AdapterError::Other("browser capability not injected".into()))` を返す。素通りの経路（browser の run でない）は `browser` を見ない。
4. 入口から先（`browser_auth.rs`・`browser_launcher_run.rs`・`launcher_run::run_registered`・`EventSinkLive`）は `BrowserPorts<'_>`（`Option` でない）か、要る側の `&dyn BrowserAuthority`／`&dyn BrowserControl` だけを受け取る。通知・観測は今までどおり `&dyn EventSink` で受ける。
5. harness の adapter に渡す `BrowserSink` は browser_* の転送 7 個を消し、通知・観測の転送だけを持つ。harness の adapter（claude_code・acp）は Browser の能力に届かなくなる（今も呼んでいない）。
6. `celerisctl worker run` は今も browser の task を拒否する（`commands/worker.rs` の `requests_browser` の判定）ので変えない。`PrintSink` は Browser の trait を実装しない。
7. 試験の sink（`browser_tests.rs`・`browser_launcher_run_tests.rs`・`browser_launcher_run/live_frame_tests.rs`、task-api の `tests/browser_e2e.rs`・`tests/browser_h3_injection.rs`）は、browser_* を新しい trait の impl へ移し、入口に `Some(BrowserPorts { .. })` を渡す。

### 2c. 採らなかった案

| 案 | 採らない理由 |
|---|---|
| `EventSink` に `fn browser(&self) -> Option<&dyn BrowserPort>` の accessor を 1 つ残す | 共通の interface に browser 専用の口が残り（`local_hook_in_shared_interface`）、渡し忘れは実行時まで分からない |
| 既定実装を残し、`browser_saved_credential_valid` の既定だけ `false` にする | fail-closed は強まるが、能力は分かれず objective を満たさない |
| Browser の 10 method を 1 つの trait にする | secret を扱う method と扱わない method の区別が型に出ない（2a） |
| browser の run のときだけ作る別の struct（store と task id を持つ）に実装する | 能力を browser の run にだけ渡す点は同じだが、`StoreSink` の `credential_site` と store の参照を写すことになる。今回は `StoreSink` が実装し、渡す先を `run_worker` で絞る |

## D3. WorkerAdapter の with_* 5 つを `configure` 1 つにする

### 3a. 署名（`crates/task-worker/src/adapter.rs`）

```rust
/// run ごと・route ごとに adapter へ施す設定。空の欄は「変えない」。
#[derive(Clone, Default)]
pub struct AdapterSettings {
    pub model: Option<String>,
    pub llm_endpoint: Option<LlmEndpoint>,
    /// `Some("")` は渡さない（呼び出し側が `None` にする。今の `prepare_run_adapter` と同じ）。
    pub permission_mode: Option<String>,
    /// 末尾に重ねる env（同名は後勝ち。ADR-0024 D2 の順序のまま並べる）。
    pub env: Vec<(String, String)>,
    pub container: Option<crate::container::SharedPlan>,
}

pub struct LlmEndpoint {
    pub base_url: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdapterSetting { Model, LlmEndpoint, PermissionMode, Env, Container }

/// `configure` の結果。
pub struct Configured {
    /// 受けた項目を適用した複製。何も適用しなかったら `None`。
    pub adapter: Option<Arc<dyn WorkerAdapter>>,
    /// 指定されたが受けられなかった項目（空 = 指定を全部適用した）。
    pub unsupported: Vec<AdapterSetting>,
}

impl Configured {
    /// 呼び出し側の唯一の受け取り方。複製が無ければ `base` を使う。
    pub fn resolve(self, base: &Arc<dyn WorkerAdapter>) -> (Arc<dyn WorkerAdapter>, Vec<AdapterSetting>);
}

#[async_trait]
pub trait WorkerAdapter: Send + Sync {
    // id・model_namespace・account_id・run は今のまま
    /// D3: `settings` を 1 回で適用した複製と、受けられない項目を返す。
    /// 既定 = 何も受けない（`adapter: None`、指定された項目を全部 `unsupported` に入れる）。
    fn configure(&self, settings: &AdapterSettings) -> Configured { .. }
}
```

- `configure` に既定実装を置く理由: `WorkerAdapter` の impl は試験に 108 個、`crates/campaign/src/dagbench/celeris_side.rs` の `StubWorker`（R2 の範囲の外）にあり、どれも設定を受けない。既定は「受けない項目を `unsupported` で報告する」だけで、許可側に倒れない（今の with_* の既定 `None` と同じ意味）。Browser の trait（D2）と違い、能力を与える口ではない。
- 各 adapter は受けられる項目を 1 か所で宣言し、それ以外を `unsupported` に入れる（`AdapterSettings` に「受ける項目の集合を渡すと残りを返す」helper を置き、9 adapter が同じ判定を写さない）。受ける項目は今の with_* の実装と同じ:

| adapter | model | llm_endpoint | permission_mode | env | container |
|---|---|---|---|---|---|
| acp | 受ける | — | — | 受ける | 受ける |
| aider | 受ける | — | — | 受ける | 受ける |
| claude_code | 受ける | — | 受ける | 受ける | 受ける |
| codex | 受ける | — | — | 受ける | 受ける |
| fake | 受ける（記録だけで実行は変えない） | — | — | 受ける | 受ける |
| langmem | 受ける | — | — | 受ける | — |
| local_deep_research | 受ける | — | — | 受ける | — |
| paperqa | 受ける | — | — | 受ける | — |
| pi | 受ける | 受ける | — | 受ける | 受ける |

- `BrowserSpecialistAdapter` は `configure` の転送 1 つだけを持つ（内側の `configure` の結果の複製を包み直し、`unsupported` はそのまま返す）。

### 3b. unsupported の扱い（呼び出し側ごとに今の挙動を保つ）

| 呼び出し側 | 渡す設定 | 今の挙動（受けられないとき） | 変更後 |
|---|---|---|---|
| `task-worker/src/route.rs` `route_adapter` | model・account の env・LLM endpoint | model: 元の adapter のまま続行。env: `RouteError::AccountUnsupported`。endpoint: `RouteError::EndpointUnsupported`（fail closed） | 1 回の `configure`。`Model` は無視、`Env` → `AccountUnsupported`、`LlmEndpoint` → `EndpointUnsupported`。両方なら今と同じく `AccountUnsupported` を先に返す。`CredentialUnresolved` は `configure` の前に判定する |
| `task-dispatch/src/dispatcher/route_launch.rs`・`route_failure.rs` `adapter_for_account` | model、account の env | model: 続行。env: `None` → `RouteLaunchError::AccountUnavailable`（fail closed） | 同じ。隣り合う 2 回の適用は 1 回の `configure` にまとめてよい |
| `dispatcher/worker_task.rs` `prepare_run_adapter` | env（cargo → followups → WU → run tmp の順に連結）・container・permission mode | env: 4 つの match がそれぞれ debug log で続行（戻り値の `env_applied` は cargo の env が適用されたか）。container: warn「running on the host」で続行。permission mode: debug log で続行 | env を 1 度組み立てて 1 回の `configure`。`Env` → 適用しなかった env の種類を並べた debug log 1 行で続行、`Container` → 今と同じ warn、`PermissionMode` → debug log。`env_applied` は「cargo の env があり、`Env` が unsupported でない」。env の対応は adapter 単位（全部受けるか全部受けないか）なので、4 回の適用と結果は同じ |
| `dispatcher/housekeeping.rs`（reviewer の check の cargo env） | env | rust の repo なら warn、続行 | 同じ |
| `task-dispatch/src/review.rs`（reviewer の run の TMPDIR） | env | warn、TMPDIR なしで続行 | 同じ |
| `dispatcher/eval_trials.rs` | env（cargo） | trial を失敗にして error（「cannot take CARGO_TARGET_DIR」） | 同じ |
| `dispatcher/cos_chat/launch.rs` | env（run credential） | credential を失効させて error（「cannot receive a run credential」） | 同じ |
| `crates/celeris/src/daemon/adapters.rs`（試験） | model | `unwrap` | `configure(..).resolve(..)` |

`task-worker/src/workspace.rs`・`task_repos.rs` の `.with_env(`・`.with_container(` は `LocalWorkspace` の builder で `WorkerAdapter` ではないので変えない。

## D4. write_set の拡張

B0 の R2 の write_set（[slices.json](../progress/2026-11-07-health-v2-b0/slices.json) の R2）は task-dispatch・各 harness adapter の file・`route.rs`・celeris の daemon を含まない。計画の決定 write-set に人が「widen-all」（ここに挙げる path まで広げ、Browser の能力の分離と with_* の整理の両方を R2 で行う）と答えた。`slices.json` は書き換えず、leaf ごとの範囲 check（計画の各 unit の `scope` の check）で縛る。

### 4a. B0 の write_set の外で変える path

| unit | path | 理由 |
|---|---|---|
| browser-port | `crates/task-dispatch/src/dispatcher/sinks.rs` | 能力の実装者 `StoreSink` が `BrowserAuthority`・`BrowserControl` を実装する（D2 2b-1） |
| browser-port・adapter-settings | `crates/task-dispatch/src/dispatcher/worker_task.rs` | `run_worker` の注入（D2 2b-2）と `prepare_run_adapter` の `configure`（D3 3b） |
| browser-port | `crates/task-api/tests/browser_h3_injection.rs` | 試験の sink を新しい trait へ移す。B0 の正規表現 `tests/[a-z_]+\.rs` は file 名の数字を通さないので外れる（`browser_e2e.rs` は内） |
| browser-port・adapter-settings | `crates/task-worker/src/lib.rs` | 新しい型（`BrowserAuthority`・`BrowserControl`・`BrowserPorts`・`AdapterSettings`・`LlmEndpoint`・`AdapterSetting`・`Configured`）の再公開 |
| adapter-settings | `crates/task-worker/src/{acp,aider,claude_code,codex,fake,langmem,local_deep_research,paperqa,pi}.rs` | with_* の写し 26 個を `configure` 1 つずつにする |
| adapter-settings | `crates/task-worker/src/route.rs` | `route_adapter` の 3 回の適用を 1 回の `configure` にする |
| adapter-settings | `crates/task-worker/src/{acp,aider,claude_code,codex,langmem,local_deep_research,paperqa,route}/tests.rs` | 試験の with_* の呼び出しと試験の adapter の with_* を移す |
| adapter-settings | `crates/task-dispatch/src/dispatcher/{route_launch,route_failure,housekeeping,eval_trials}.rs`、`dispatcher/cos_chat/launch.rs`、`crates/task-dispatch/src/review.rs` | 呼び出し側（D3 3b） |
| adapter-settings | `crates/task-dispatch/src/dispatcher/tests/*.rs`（with_* を持つ試験の adapter は `account_auth_failure`・`build_cache`・`cos_chat_launch`・`cos_chat_rollover`・`cos_launch_cross`・`cos_launch_wire`・`eval_trial_dispatch`・`followups`・`mod`・`review_route`・`route_launch`・`run_tmpdir`・`scope_checks`・`trial_context`）、`dispatcher/cos_chat/harness_tests.rs`、`crates/task-dispatch/src/review/tests.rs` | 試験の adapter の with_* を `configure` に移す |
| adapter-settings | `crates/celeris/src/daemon/adapters.rs`、`crates/celeris/tests/trial_context_canary_e2e.rs` | 試験の `with_model` の呼び出しと、試験の adapter の `with_env` |

B0 の write_set の内で変える path（参考）: `crates/task-worker/src/{adapter,browser,browser_policy,browser_live,browser_auth,browser_launcher_run,browser_specialist,browser_tests,browser_launcher_run_tests,browser_runtime}.rs`、`crates/task-worker/src/browser_launcher_run/live_frame_tests.rs`、`crates/task-api/tests/browser_e2e.rs`、`crates/celeris-credentiald/src/lib.rs`、`crates/task-core/src/browser.rs`、この ADR と R2 の進捗。

### 4b. 範囲 check が許すが、要るときだけ変える path

| unit | path | 変える条件 |
|---|---|---|
| browser-port | `crates/task-dispatch/src/dispatcher/tests/`・`crates/task-dispatch/tests/` | browser の入口や `StoreSink` の browser_* を直接呼ぶ試験が見つかったとき（2026-11-10 の grep では無い） |
| browser-port・adapter-settings | `docs/architecture-map.md` | 行 73（run 途中の sink）・行 89（Adapter 選択）の主な file・試験の列が変わったとき |
| adapter-settings | `crates/celeris/src/config/harness.rs`・`crates/celerisctl/src/commands/worker.rs` | adapter の組み立てや worker run が with_* に触れていたとき（2026-11-10 の grep では無い） |

`crates/campaign/` は変えない（`StubWorker` は with_* を持たず、`configure` の既定で足りる）。範囲の外が要るときは、unit は作業を止めて plan_issue で返す（B0 の停止条件）。

## D5. 撤去対象と不変条件の試験

### 5a. 撤去（同じ変更で消す）

| 撤去するもの | unit | 確かめ方 |
|---|---|---|
| `EventSink` の browser_* 10 個と既定実装（`browser_saved_credential_valid` の既定 `true` を含む） | browser-port | `measure.py --check-browser-split`（`event_sink.browser_count` が 0） |
| `browser.rs` の `BrowserSink` の browser_* の転送 7 個 | browser-port | 同上（`unrelated_sinks.BrowserSink` が 0） |
| `WorkerAdapter` の `with_model`・`with_llm_endpoint`・`with_permission_mode`・`with_env`・`with_container` と、9 adapter の写し 26 個、試験の adapter の写し 31 個 | adapter-settings | `measure.py --check-adapter-settings` |
| `BrowserSpecialistAdapter` の with_* の転送 5 個（`configure` の転送 1 個にする） | adapter-settings | 同上（`browser_specialist_setting_forwards` ≤ 1） |
| `prepare_run_adapter` の 4 重の `with_env` の match | adapter-settings | 同上（`worker_task_with_env_calls` が 0） |
| B0 の dup 4 件: `browser_runtime.rs` の `SANDBOX_UID`（pub・未参照）と `INNER_ID` の二重定義を 1 つの定数に、`browser_live.rs` の `close_and_join`、`celeris-credentiald` の `from_result`、task-core `browser.rs` の `normalize_host_pattern`（未参照） | dups | 消す前に crates・web・control-ts・scripts を grep し、参照があれば消さずに進捗に書く |

移行のための旧い経路は残さない（互換の別名・deprecated・新旧切替の flag を置かない）。

### 5b. 不変条件と試験

| 不変条件 | 試験 | 種類 |
|---|---|---|
| `credential-stays-in-broker`: saved credential は daemon の能力の実装からだけ読み、harness の入力から読まない | `browser_launcher_run_tests.rs` の `saved_credential_reuse_waits_every_run_and_falls_back_without_secrets`・`launcher_stale_credential_approval_is_refused_before_connecting`・`launcher_credential_preconnect_unadmittable_wait_refuses_without_connecting` | 既存（sink を新しい trait へ移すだけ） |
| fail-closed: browser の run に能力が無ければ harness を spawn しない | 新規 `browser_port_missing_refuses_before_spawn`（task-worker の lib 試験。名前は `browser_port_` で始める）: browser の task と `None` で入口を呼び、`Err`（`browser capability not injected`）が返り、adapter の `run` が呼ばれず substrate も作られないことを見る。既存の control gate・前提の拒否は `browser_tests.rs` の `sensitive_backend_is_refused_before_substrate_harness_and_wait_creation`・`missing_egress_resolver_refuses_before_browser_start` | 新規＋既存 |
| 承認は 1 回だけ消費される | `crates/task-core/src/browser_wait/tests.rs` の `approve_once_consumes_once_and_deny_fails_task`、task-api `tests/browser_e2e.rs` の `approved_credential_requires_conformance_and_does_not_consume_approval` | 既存 |
| auth section の間は takeover・renew を拒否する | task-api `tests/browser_e2e.rs` の `auth_section_store_denies_takeover_until_closed`、`tests/browser_h3_injection.rs` | 既存 |
| 通知・観測だけの sink は Browser の能力に依存しない | `measure.py --check-browser-split`（`ReviewerSink`・`ChatRunSink`・`PrintSink`・`NullSink`・`BrowserSink` に browser_* が 0、Browser の trait の既定実装が 0） | 計測台本 |
| LLM endpoint を持たない harness の route は fail closed のまま | `crates/task-worker/src/route/tests.rs` の `worker_route_openai_compatible_endpoint_and_credential`（`EndpointUnsupported`）。新規 `adapter_settings_`（task-worker の lib 試験）: 各 adapter の `unsupported` の報告（例: claude_code に endpoint、langmem に container）、既定実装が全項目を報告すること、`BrowserSpecialistAdapter` の転送が内側の `unsupported` を保ち id が `browser-specialist` のままであること、env の後勝ちの順序、`route_adapter` の `AccountUnsupported`（今は試験が無い） | 既存＋新規 |
| unsupported の扱いが呼び出し側ごとに今と同じ（D3 3b） | 既存の `dispatcher/tests/{eval_trial_dispatch,cos_chat_launch,run_tmpdir,route_launch,account_auth_failure}.rs` が試験の adapter を `configure` に移しても通ること | 既存 |
| `events-append-only`・`no-llm-in-dispatcher`・`no-unwrap-outside-tests` を変えない | `cargo clippy --workspace -- -D warnings`・`bash scripts/dev/test-affected.sh`・`bash scripts/dev/check-health.sh` | 検査 |

userns・実 browser の要る試験（task-api の `browser_e2e.rs`・`browser_h3_injection.rs` の一部）は worker の sandbox で完走しないので、unit は `--no-run` で build だけを確かめ、実行は release gate に回す。

## 結果

- 通知・観測だけを使う sink と harness の adapter は、型の上で Browser の認可・制御・secret の能力を持たなくなる。browser の run に能力を渡し忘れると、harness の spawn の前に `AdapterError` で止まる。
- with_* の写し 26 個（試験 31 個）と browser-specialist の転送 5 個が `configure` 1 つずつになり、`prepare_run_adapter` の match 4 つが 1 つになる。
- 置き場所の正本: 通知・観測は `adapter.rs` の `EventSink`、Browser の認可・secret は `browser_policy.rs` の `BrowserAuthority`、Browser の制御・live は `browser_live.rs` の `BrowserControl`、adapter の設定は `adapter.rs` の `configure`。

## 付記（2026-11-11、unit dup-cleanup）

D5 5a の「B0 の dup 4 件」は unit dups ではなく、それを置き換えた unit dup-cleanup が実装した（dups は範囲検査の開始 snapshot に並行の変更が混じったため superseded）。

- userns の内側の UID / GID（ADR-0115 の `1000 → S`）の定数は `crates/task-worker/src/browser_launcher/userns.rs` の `pub const INNER_ID: u32 = 1000;` の 1 つにした。2 map を作る `Mapping::helper_args`・`Mapping::expected`、holder の `setresgid` / `setresuid`、`browser_launcher/backend.rs` の `maps_match` の期待する map、`browser_runtime.rs` の bwrap の `--uid` / `--gid`（`INNER_ID.to_string()`）と spawn の子の `setresgid` / `setresuid`、`browser_launcher_run.rs` の `runtime_facts` がこの定数を使う。`browser_runtime.rs` の `SANDBOX_UID`（pub・未参照）と、`browser_runtime.rs`・`browser_launcher_run.rs` の private な `INNER_ID` は消した。値と map の形は変えていない。
- 未参照の 3 つを消した: `browser_live.rs` の `FrameRelay::close_and_join`（`#[cfg(test)]`、試験からも呼ばれていない）、task-core `browser.rs` の `normalize_host_pattern`、`celeris-credentiald` の `lib.rs` の `CredentialUseResult`（struct と唯一の impl `from_result` の両方）。消す前に `crates`・`web`・`control-ts`・`scripts` を grep し、定義の他に参照が無いことを確かめた。
- `CredentialUseResult` は [ADR-0078](0078-browser-execution-capability.md) が LLM に返す結果の interface 名として挙げているが、実装の中で作る所も返す所も無かった（broker は `injection_ipc` の応答と audit の code で結果を返す）。ADR-0078 の本文は書き換えない。credential broker の挙動は変わらない（消したのは呼ばれていない型と関数だけ）。
