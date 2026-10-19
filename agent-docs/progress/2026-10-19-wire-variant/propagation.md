---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 通常API・永続化・dispatcherへのvariant伝播（normal-propagation）
work_unit: verify-record
status: done
updated: 2026-10-19
---
# 通常 API・永続化・dispatcher への variant 伝播（propagation）

親: 「選択機構だけの variant 設定と伝播を配線」（段階 propagate）。前段の契約は同 dir の `contract.md`（型・hash・投入/照合の配線まで）と `integration-fix.md`（統合失敗の診断。コード欠陥なし）。

**適用証明（実 org 適用・子孫 Assigned の実測・run 開始時の構成証拠）は後段（normal-e2e / verify-record）まで未実施。audit と pin-boundary（§0・§8）はコードを変えていない。**

## 0. 訂正（pin-boundary, 2026-10-19）

audit 時点の記載のうち、実装（core-wire / core-store / core-callsite-fix / core-api-schema / core-web-types）と食い違うものを本文で直した。旧い記載は残していない。

| audit の記載 | 実装・確定 | 根拠 |
|---|---|---|
| migration の空き番号は 0087 | **0088**（`crates/task-core/migrations/0088_task_campaign_variant.sql`）。0087 は別 branch の `0087_decision_log_lane_stages.sql` が使うため `RESERVED_VERSIONS` に 87 を入れて飛ばした | `0354243c`、`core-store.md` |
| 新関数 `task_campaign_resolve_checked` を足し、旧 `task_campaign_resolve` を置き換えて消す | **新関数は作っていない**。既存 `task_campaign_resolve` の名前と型を保ったまま中身を強化した（root まで辿り、行を持つ全祖先が一致しなければ `StoreError::Invalid`、循環・上限 32 超えも `Invalid`）。呼び出し側（`run_context.rs`・`lane_assist.rs`・`decision_log_wire.rs`・e2e）は呼び替え不要 | `0354243c` |
| core-wire の葉で struct literal（task-core＋task-dispatch 試験）・API schema・web 生成型を一緒に直す（複数 subtree を同じ葉に） | **subtree ごとに別葉**: core-wire（task-core の `variant_wire.rs` だけ。`CampaignIdentity` は不変）→ core-store（task-core、`CampaignIdentity` 新欄・migration・store）→ core-callsite-fix（task-dispatch 試験 literal）→ core-api-schema（`docs/api/v1`）→ core-web-types（`web/api/generated`） | `c8bd52e0`・`0354243c`・`9b786d02`・`ee920ba6`・`b7f1f15c` |

既存 3 task と main の再照合（pin-boundary 時点）は §2 の末尾。

## 1. 時点・SHA（audit, 2026-10-19T15:28Z）

| 対象 | 値 |
|---|---|
| 本 WU の HEAD / base | `6bb9ecc7ece1ae12be27466a0f7d195bd6fc9a4b`（integrate wu/contract-integration-fix） |
| 最新 main | `git ls-remote origin refs/heads/main` = `fd9027fbfa0f7464aa3eb405dac4330f0dfd92cf`（commit 時刻 2026-10-19T14:29:56Z）。ローカル `main` も同じ |
| merge-base(HEAD, main) | `c9654bff`。main は HEAD に無い 21 commit（24 files, +3818/-13） |

main 側の差分の中身: `crates/campaign/src/human_ops{,_join}.rs`・`tests/human_ops*.rs`（評価者の生記録）、`crates/campaign/{Cargo.toml,src/lib.rs,src/main.rs}`（mod 追加）、`task-dispatch` の cos_chat launch / `review.rs`、`task-worker` の cos_chat / `workspace.rs`、`docs/ops/*`、`config/skills/cos-operator/*`。`CampaignIdentity`・`task_campaigns`・`decision_log`・`matching`・`run_context.rs`・`protocol.rs`・`variant*.rs`・`celeris_exec.rs`・schema 生成物には触れていない。衝突し得るのは `crates/campaign/src/lib.rs`（`pub mod` の行追加）と `review.rs`（RunContext 文字列の 1 箇所。worker-pin が RunContext に欄を足すときだけ）。**本 WU では main を取り込んでいない**（取り込みは統合の判断。上記以外に阻害なし）。

## 2. 既存 3 task の状態と証拠（明示 DB の read-only 読取）

読取: run 環境の `CELERIS_RUN_DB=/local/celeris/data/db/celeris.sqlite3` を `sqlite3 "file:$CELERIS_RUN_DB?mode=ro"` で読んだ（書き込み・既定 DB 作成なし）。列は `tasks.status`（`state` 列は無い）。

| task | status | updated_at | 最終 event seq / ts | git 上の実装・試験 |
|---|---|---|---|---|
| `01M58VFRBA6HG0SSF3CG5BKM91`（本測定移行 2. 設定確定） | **ready** | 2026-10-19T12:30:57Z | 1601 / 2026-10-19T12:50:28Z | `celeris/01M58VFRBA6HG0SSF3CG5BKM91` = `cd7f85ca`（integrate wu/symlink-components。integration-fix の読取時と同じ head）。HEAD にも main にも未包含（26 files +744/-50: `campaign/src/{snapshot,isolation}.rs`、`tests/{inner_symlink,measure_daemon_example,isolation,cap_probe,final_resolved,a_program_pin_and_watch}.rs`、`measure-daemon.example.toml`・`measure-launcher.example.env`、`docs/ops/measure-run-final.md`）。WU branch 7 本（daemon-decl ほか） |
| `01M58VG5C4XGK4E70M6WJ6MEX0`（3. 無 LLM 統合・主比較先行） | **cancelled** | 2026-10-19T11:19:47Z | 4 / 同 | branch・commit なし |
| `01M58VGDNQ8GWGJPB8SJ73XSPF`（4. A0/B0 最小実 pilot・最終報告） | **cancelled** | 2026-10-19T11:19:47Z | 4 / 同 | branch・commit なし |

**再照合（pin-boundary, 2026-10-19）**: `git fetch origin` 後の `origin/main` = `254c885a921e`（integrate wu/sub-pins）。本 WU の base `1b0d1ece` より main が 48 commit 先（64 files, +6500/-78）。`01M58VFRBA6HG0SSF3CG5BKM91` は **done**（updated 2026-10-19T17:06:28Z）で、その branch head `254c885a` が main そのもの（main に統合済み）。`01M58VG5C4XGK4E70M6WJ6MEX0`・`01M58VGDNQ8GWGJPB8SJ73XSPF` は cancelled のまま。main 側の追加は `campaign/src/{offline_lock,human_ops,human_ops_join}.rs`・`snapshot.rs`・`isolation.rs`、`scripts/campaign/check-{main,sub}-pins.py`、`measure-daemon.example.toml`、cos_chat / review の RunContext 周り。`variant*.rs`・`preflight.rs`・`celeris_exec.rs`・`crates/task-{core,api,ops}` には触れていない（`git diff --stat HEAD...origin/main -- <それら>` は空）。`git merge-tree HEAD origin/main` の衝突は既存 progress 4 件の front matter（両側で同じ欄を補った）だけ。本 WU では main を取り込まない。§8 は main の `offline_lock::catalog_from_evidence`（承認済み sha256 を呼び出し側が明示して実証跡を読む）と `measure-daemon.example.toml`（専用 daemon config 雛形）を再利用し、作り直さない。

contract.md 冒頭（running / ready / ready）は前日朝の読取で古い。現在値は上表。重複回避: 2 の branch は campaign の snapshot/isolation/測定 daemon 例のみで、variant・CampaignIdentity・dispatcher に触れない。本 task はそれを再実装しない（`measure-daemon.example.toml` の「本番と別の db・port・workspace」方針は専用環境の運用手順として参照するだけ）。3・4 は取消済みで依存しない。

## 3. 再利用する既存実装 / 不足

### 再利用（作り直さない）
| 領域 | 既存（path） |
|---|---|
| 識別の型 | `task_core::decision_log::CampaignIdentity`（`crates/task-core/src/decision_log.rs:51`、`deny_unknown_fields`、`validate`） |
| 保存・原子性 | `task_campaign_set_tx`（task 作成と同じ transaction）、`task_campaign_resend_tx`（同 key / 同試行 → Same / Conflict）、`crates/task-core/src/store/decision_log.rs:635-740`。API 側 `crates/task-api/src/handlers/tasks.rs:325-500`（事前 `campaign_resend`＋tx 内の再判定 `(concurrent)` 409） |
| 祖先解決 | `task_campaign_resolve`（最寄り優先、`CAMPAIGN_ANCESTOR_LIMIT`）。利用者: `run_context.rs:443`（trial_scope）、`decision_log_wire.rs:428`、`lane_assist.rs:208` |
| 決定ログ | `DecisionLogProposal::apply_campaign`（`decision_log.rs:150`）、`decision_log_runs` の識別列（migration 0085） |
| 割当 | `assign_if_needed` → `task_ops::matching::decide(&org_list, &task)`（`crates/task-dispatch/src/dispatcher/dispatch_run.rs:1397`）。assignee 有りは `NotApplicable`（retry は既存 assignee を保持） |
| 試行隔離 | `trial_scope`（`run_context.rs:440`、試行 KB = 根 task 作業場所の `kb/`）、campaign `isolation.rs`（prepare/verify/canary）、`measure_env::check_config`（測定 daemon config と通常 config の突合せ） |
| campaign 契約 | `variant.rs`（`VariantRef`/`VariantSpec`/`VariantError`）、`variant_pin.rs`（`OrgSnapshot::from_get_org`・`check_pin`・`check_live_org`）、`preflight::check_variant`、`celeris_exec.rs`（`with_variant`、POST body・GET 照合・自作 task の cancel） |
| 専用 daemon の試験枠 | `crates/celeris/tests/campaign_wiring_e2e.rs` の `Daemon::start`（tempdir に config.toml・token・DB、loopback、fake harness）。org の種は `seed_org_if_empty`（`crates/celeris/src/daemon/bootstrap.rs:561`）が空 DB にだけ入れる |
| 生成手順 | `UPDATE_SCHEMA=1 cargo test -p task-api`（`docs/api/v1/api-v1.schema.json`、`schema.rs` の `committed_schema_matches_generated`）、`UPDATE_SCHEMA=1 cargo test -p task-worker`（`docs/protocol/worker-protocol.schema.json`）、`pnpm -C web gen:types`（`web/api/generated/{schema.json,types.ts}`）。`gui/` は追跡 3 file のみで生成型なし → 対象外 |

### 不足（本 task で足す）
1. `CampaignIdentity` に variant が無い → 現 API は variant 付き POST を `deny_unknown_fields` で 4xx（fail-closed）。
2. `task_campaigns` に variant の列が無い（migration 0085 は 6 欄）→ 0088 で追加済み（§0）。
3. **GET `/api/v1/tasks/{id}` は campaign 識別を返さない**。`TaskDetail`（`crates/task-ops/src/view.rs:164`）にも `Task`（`task-core/src/model.rs:448`）にも campaign 欄が無い。一方 `celeris_exec.rs:283` は `detail["task"]["campaign"]["variant"]` を読む → 現状では variant 付き再開・冪等照会は常に `Refused`。返す場所と読む path を揃える必要がある（§5 D4）。
4. 祖先整合: 子孫は行を持たず最寄り優先で読むだけ。子孫に行を付ける経路（`task_campaign_set` は parent を見ない）で root と食い違っても検出しない → core-store で `task_campaign_set_tx` の祖先・子孫検査と既存 `task_campaign_resolve` の全祖先一致検査に直した（§0）。
5. dispatcher は variant の org 形状を見ない（spec-off の単一 node 制約・retry の既存 assignee 検査なし）。run 開始時の org drift 検査なし。
6. `RunContext`（`crates/task-worker/src/protocol.rs:418`）に campaign / variant / org の欄が無い → worker 側の構成証拠・再開検証の材料が無い。
7. 決定ログの run 行に variant が落ちない（arm 区別は `arm` 文字列のみ）。

## 4. 型境界の設計（確定）

依存: `campaign → task-core`（`crates/campaign/Cargo.toml`）。task-core → campaign の逆依存は作らない。

- **下位 wire 型は task-core 所有**（`crates/task-core/src/decision_log.rs`）:
  - `CampaignVariantRef { comparison_id: String, id: String, content_sha256: String, selection_sha256: String }`（`deny_unknown_fields`、JsonSchema）。JSON 形は campaign `VariantRef` と同一（`id` は `"M1-spec-on"` 等の文字列）。task-core は意味を知らず、`validate` は非空・`*_sha256` が 64 hex だけを見る。
  - `CampaignOrgPin { nodes: Vec<String>, coding_node: String, effective_profile_sha256: String }`（`VariantSpec.org` = campaign `OrgPin` と同じ 3 欄）。dispatcher が自分の `org_list()` と比べる材料。
  - `CampaignIdentity` に `variant: Option<CampaignVariantRef>` と `variant_org: Option<CampaignOrgPin>` を `#[serde(default, skip_serializing_if = "Option::is_none")]` で追加。`validate`: どちらか片方だけは拒否、`variant_org.nodes` に `coding_node` を含み重複なし。
  - `effective_profile_sha256` の計算は task-core へ下ろす（`task_core::profile` に `EffectiveProfile` → `node_id`・`chain` を除いた canonical JSON の sha256）。campaign の `variant::effective_profile_sha256`（GET org の JSON 値から計算）と**同値であることを core-wire の試験で固定**し、campaign 側は bridge で task-core 版を呼ぶ形に寄せる（二重実装を残さない）。
- **campaign は公開型を維持し明示変換**（campaign-bridge）: `impl From<&VariantRef> for task_core::decision_log::CampaignVariantRef`、`impl TryFrom<&CampaignVariantRef> for VariantRef`（未知 id → `VariantError::Schema`）、`From<&OrgPin> for CampaignOrgPin`。`VariantRef` / `VariantSpec` / manifest / lock の struct と serde 形は変えない → `VariantSpec::content_sha256`・`manifest::compute_sha256`・`selection.sha256`（`b6132a44…64bc`）・既存 lock の hash は不変。試験: 両型の `serde_json::to_value` 一致、往復、未知 id 拒否、既存 hash 固定値。
- hash の保持: task 側は hash を再計算しない。保存した 4 欄＋org pin をそのまま返し、比較は欄の完全一致（既存 409 と同じ流儀）。

## 5. 決定（設計）

- **D1 保存**: migration `0088_task_campaign_variant.sql` で `task_campaigns` に `variant_json TEXT`・`variant_org_json TEXT`（NULL 可）。`decision_log_runs` に `variant_id TEXT`・`variant_content_sha256 TEXT`（arm 分割用）。`task_campaign_set_tx` / `own_tx` / `own` / `resend_tx` は新欄を読み書きし、resend の Same 判定は新欄込みの全欄一致（variant だけ違えば 409 `campaign_identity_conflict`）。
- **D2 祖先整合**: `task_campaign_set_tx` は対象 task の祖先鎖に識別があり、それと 1 欄でも違えば `StoreError::Invalid`（root と子孫の不一致を拒否）。既存 `task_campaign_resolve` の中身を強化: 鎖全体を辿り、行を持つ全祖先が一致しなければ `Err`（最寄り優先で矛盾を隠さない）。名前・型は保ち、新関数は作らない。旧い最寄り優先だけの実装は消えた。
- **D3 通常 POST**: 既存の `campaign_resend` → tx 内 `task_campaign_resend_tx` → `task_campaign_set_tx` の順をそのまま使い、`validate`（D1 の片欠け拒否含む）を tx 前に行う。新 endpoint・新 scheduler/router は作らない。
- **D4 GET**: `TaskDetail` に `campaign: Option<CampaignIdentity>`（`task_campaign_resolve` の結果。祖先矛盾は `null` に丸めず API problem で返す）を足し、campaign 側 `celeris_exec.rs` の読む path を `detail["campaign"]["variant"]` に変える（campaign-bridge）。`Task` 本体には足さない（`Task` literal が多く、識別は継承で決まる値のため）。
- **D5 dispatcher（dispatch-pin）**: `assign_if_needed` の直前に、識別に `variant_org` があれば `org_list()` から node 集合と `coding_node` の effective profile hash を計算し `CampaignOrgPin` と比較。不一致（node 集合か coding 課 profile の drift。他 node・役割モデルなどの非対象は検出範囲外で、§8 の L2 が判定）は割当てず task を `blocked`＋理由 event（適用済み扱いしない）。一致なら既存 `matching::decide` をそのまま呼び、spec-off（`nodes` が根＋1 node）では結果の node が `coding_node` でなければ拒否。assignee 既存（retry）は `decide` が `NotApplicable` を返すので、その assignee が `nodes` に含まれ spec-off なら `coding_node` であることを検査。
- **D6 系譜（dispatch-lineage）**: 子孫・retry・continuation・repair は行を作らず D2 で強化した `task_campaign_resolve` で継ぐ（新規配線は「解決の `Err` を割当・run 開始の明示失敗にする」と「子生成時に親の識別が解決できるか確認」だけ）。子 task 作成経路（`child_tasks.rs`）で識別を新規に付ける箇所があれば D2 の検査を通す。
- **D7 worker（worker-pin）**: `RunContext` に `campaign: Option<CampaignRunPin>`（serde default・skip。中身は §8.5 で確定: `identity`・`org_nodes_sha256`・`selected`・`daemon_instance_id`・`config_sha256`）。dispatcher が run 開始直前に D5 と同じ照合を再実行して値を入れる（run 開始時の drift も拒否）。非対象の合否は daemon が判定せず、campaign の L2（§8.6）が判定する。worker は値を run の証拠（result 側の記録）へ写し、continuation/再開時は保存済みの pin と一致しなければ明示失敗。

## 6. 専用試行環境の所有と org snapshot の受渡し点（受け入れ 1）

所有者: **campaign runner が起動する測定専用 daemon**（本番と別の `db`・`api.listen`・`workspace_root`・KB・記憶。`measure_env::check_config` が通常 config との分離を検査）。自動試験では試験自身が tempdir に作る daemon（`campaign_wiring_e2e.rs` の `Daemon::start` と同型）が所有する。**共有 daemon（本番）の org は読みも書きもしない**。variant ごとに org を書き換えず、1 daemon = 1 variant の org で固定する（spec-on と spec-off は別 daemon・別 DB）。

| # | 受渡し点 | 誰が → 誰へ | 値 | 書換えの有無 |
|---|---|---|---|---|
| 1 | org の用意 | 試験 harness / 人の運用手順 → 専用 daemon の空 DB | config の org seed（`seed_org_if_empty`。空 DB のときだけ入る）。spec-off = 根 `cos`＋単一作業課 | 専用 DB にだけ書く。自動試験では許可。本番 org（`~/.config/celeris`・本番 DB）は不可 |
| 2 | preflight | 専用 daemon `GET /api/v1/org` → campaign `OrgSnapshot::from_get_org` → `check_pin` | `nodes`・`coding_node`・`effective_profile_sha256` を `VariantSpec.org` と照合 | 読むだけ。不一致なら有料実行 0 件で停止 |
| 3 | 投入 | campaign `CelerisExecutor::body` → `POST /tasks` | `campaign.variant`（`VariantRef` → `CampaignVariantRef`）＋ `campaign.variant_org`（`OrgPin` → `CampaignOrgPin`） | task 作成と同じ tx で `task_campaigns` に保存 |
| 4 | 割当前 | store `task_campaign_resolve` → dispatcher `assign_if_needed` 前 | 保存 pin と `org_list()` の再計算値 | 読むだけ。不一致は blocked |
| 5 | run 開始 | dispatcher → `RunContext.campaign` → worker | 照合済み pin と観測 hash | 読むだけ。drift は run を開始しない |
| 6 | 再開・冪等照会 | `GET /tasks/{id}` の `TaskDetail.campaign` → campaign `lookup` | 4 欄＋org pin の完全一致 | 読むだけ。欠落・不一致は `Refused` |

本番 host での測定 daemon の起動・org seed の配置は人が行う手順（`docs/ops/measure-run-final.md` 系。本 task では実行しない）。

## 7. 全利用者の列挙と葉ごとの修正順（callsite のビルド成立順）

### `CampaignIdentity` の struct literal（新欄で compile が壊れる箇所）
- `crates/task-core/src/store/decision_log.rs:752`（`task_campaign_own_tx`）、`:776`（`task_campaign_own`）
- `crates/task-dispatch/src/dispatcher/decision_log_wire.rs:705`（試験）
- `crates/task-dispatch/src/dispatcher/tests/trial_context.rs:77`
- `crates/task-dispatch/src/dispatcher/tests/lane_assist.rs:378`、`:420`
- 型参照のみ: `task-core/src/store/{task_store.rs:132, task_store_impl.rs:147}`、`task-api/src/handlers/tasks.rs:111`。JSON で作る利用者（壊れない）: `crates/task-api/tests/campaign_ident.rs`、`crates/celeris/tests/campaign_{wiring,lock,acceptance}_e2e.rs`、campaign `celeris_exec.rs:375-388`
- 祖先解決の利用者: `run_context.rs:443`、`decision_log_wire.rs:428,812`、`lane_assist.rs:208`、試験 `celeris/tests/campaign_*_e2e.rs`（resolve 5 箇所・own 1 箇所）、`task-api/tests/campaign_ident.rs:49-115`

### `RunContext`（`Default` derive、literal 約 108 箇所）
`task-worker/src/claude_code/tests.rs`(53)・`preamble/tests.rs`(34)・`local_deep_research/tests.rs`(3)・`cos_chat/{tests,bench_tests}.rs`(3)・`codex/cos_chat_tests.rs`・`browser_tests.rs`・`protocol/tests.rs`(2)・`routing_context_transport.rs`、`task-dispatch/src/dispatcher/{worker_task.rs, eval_trials.rs, cos_chat/launch.rs}`・`task-dispatch/src/review.rs`、`celerisctl/src/commands/worker.rs`(2)、`task-api/tests/browser_{e2e,h3_injection}.rs`。`..Default::default()` を使わない literal だけが壊れる → worker-pin で一括。

### schema 利用者
- `docs/api/v1/api-v1.schema.json`（`task-api/src/schema.rs` の drift 試験。`CampaignIdentity`・`TaskDetail` が載る）→ `web/api/generated/{schema.json,types.ts}`（`web/scripts/gen-types.mjs`）
- `docs/protocol/worker-protocol.schema.json`（`task-worker/src/protocol/tests.rs` の drift 試験。`RunContext`）
- `gui/` は生成型なし（対象外）

### 修正順（各葉は単独で workspace がビルド・試験可能であること）
1. **core-wire**（task-core のみ）: `variant_wire.rs` に `CampaignVariantRef`・`CampaignOrgPin`・`effective_profile_sha256`。`CampaignIdentity` は不変（済、§0）。
2. **core-store**（task-core のみ）: `CampaignIdentity` 新欄・migration 0088・set/own/resend・既存 `task_campaign_resolve` の強化（済）。続けて **core-callsite-fix**（task-dispatch 試験 literal 4 箇所）→ **core-api-schema**（`docs/api/v1`）→ **core-web-types**（`web/api/generated`）を別葉で（済）。
3. **campaign-bridge**（campaign）: 明示変換、`variant_org` の投入、`celeris_exec` の GET path を `detail.campaign` へ（D4）。fake 応答の試験も同葉。
4. **api-atomic**（task-api ＋ task-ops view）: `TaskDetail.campaign`、POST の validate・409 試験（variant 違い・片欠け・祖先不一致）。schema 再生成（api-schema）は TaskDetail 変更と同じ段で行う。
5. **dispatch-pin** → **dispatch-lineage**（task-dispatch）: D5・D6。
6. **worker-pin**（task-worker ＋ RunContext literal 一式）: D7。protocol schema 再生成は同段。
7. **api-schema / protocol-schema / web-types**: 上で同段生成した成果の最終再生成と差分ゼロ確認。
8. **normal-e2e**: 専用 tempdir daemon（§6）で通常 POST → 実 dispatcher の `Assigned` が spec-off で単一作業課、retry 保持、drift で blocked を無 LLM で確認。
9. **verify-record**: test-parallel・clippy・fmt と本記録の確定。

## 8. 固定証拠の境界（pin-boundary, 決定後設計）

### 8.1 原則
- daemon 側（task-core / task-api / task-ops / task-dispatch / task-worker）が知るのは `CampaignIdentity` の `variant`（`CampaignVariantRef` 4 欄）と `variant_org`（`CampaignOrgPin` 3 欄）だけ。`VariantSpec`・B0 入力・manifest・lock・env-lock は campaign 所有の file で、daemon は読まない。daemon の config にそれらの path を足すこともしない（task-core → campaign の逆依存も、daemon → campaign の file 依存も作らない）。
- `VariantRef`・`VariantSpec`・`OrgPin`・`NonTargetPin` の struct と serde 形、`content_sha256`・`selection.sha256`・manifest/lock の hash、POST/GET の `campaign` 欄の形は変えない。本節は既存の wire に載っている値と既存の GET 応答だけで組む。
- 照合は 2 層に分ける。
  - **L1（daemon）**: 識別欄と、daemon 自身が観測できる値の一致を検査し、観測値を run の証拠として残す。
  - **L2（campaign）**: 封印証拠から期待値を作り、L1 の記録と GET の応答に照合する。L1 が通っただけでは試行を適用済みにしない。
- **`variant_org.effective_profile_sha256` は coding 課 1 node が B0 coding 課と同値であることの証拠にすぎない。** builtin 役割（`plan`・`reviewer`・`conversation`・`knowledge`・`smoke`）、他の node（spec-on の非 coding 課、両 variant の根 `cos`）、全役割のモデル、情報（KB・資料・記憶）、予算、認可、grader の不変性の証拠には使わない。記録・報告・試験名でも「hash 一致なので非対象は不変」と書かない。

### 8.2 封印証拠（信頼の根）と所有 path

| 証拠 | 所有 path（型・関数） | 封印 | 信頼の根 |
|---|---|---|---|
| 選択 | `crates/campaign/fixtures/ablation/selection.{json,sha256}`、`variant::selection_sha256`・`selection_json_sha256` | 同梱の固定値 | repo（変更しない） |
| manifest | `manifest.rs` `CampaignManifest`（`variant: Option<VariantRef>`）・`compute_sha256` | campaign sha256 | 承認済みの固定 root（`check-main-pins.py` と同じく固定値で照合） |
| lock | `lock.rs` `CampaignLock`（`materials_sha256`・`kb_snapshot`・`cases[].grader`・`execution.{configs,budget,termination,variant}`）、`bind_to_manifest`・`check_successor` | `lock_hash` | manifest に結ぶ |
| VariantSpec | `variant.rs` `VariantSpec::check_seal`・`to_ref`・`verify` | `content_sha256` = `manifest.variant.content_sha256` = `lock.execution.variant.content_sha256` | manifest |
| B0 入力 | `variant_pin.rs` `B0Inputs`（`variant-b0.json`: `coding_node`・`role_models`・B0 daemon の `GET /api/v1/org` 応答） | **現状は無封印**。`spec.verify` を通して coding 課の profile hash・node 集合・`role_models` だけが spec の封印に推移的に結ばれ、B0 の非 coding node と根 `cos` の profile は何にも結ばれていない | 決定: 承認済み sha256 を呼び出し側が明示して読む（main の `offline_lock::catalog_from_evidence(path, sha256)` と同じ方式）。sha256 が無い・一致しない場合、preflight は失敗する。所有は campaign（`variant_pin.rs`・`preflight::check_variant`。提案 P1） |
| env-lock | `env_lock.rs` `EnvLock`（`celeris-env-lock/3`: program pin・release・arm ごとの `catalog`） | `establish`・`diff` | campaign hash に結ぶ |
| 測定 config | 専用 daemon の `--config`（雛形 `agent-docs/progress/2026-10-16-mlsys-final/measure-daemon.example.toml`、main） | file の bytes の sha256 = `GET /api/v1/config` の `identity.config_sha256`（`env_check.rs`）。通常 config との分離は `measure_env::check_config` | 承認済みの config file |

### 8.3 対応表: 既存 org・設定経路の全 node と全非対象構成 ↔ 封印証拠 ↔ 照合点

daemon が値を読む path は既存のものだけ: store の `org_list()` と `task_core::profile` の継承解決（= `GET /api/v1/org` の `items`・`effective_profiles`）、`GET /api/v1/config` の `identity`、`runs.selected_{harness,source,model,account}_id`（migration 0071）、`GET /api/v1/llm/models`。

| # | 構成 | daemon 側の経路 | 封印証拠側の期待値 | POST（L1） | run 開始（L1） | campaign（L2） | 欠落・不一致 |
|---|---|---|---|---|---|---|---|
| 1 | node 集合 | `org_list()` の id | `spec.org.nodes`（spec-on = B0 の node、spec-off = `cos`＋単一課） | `variant_org.nodes` と照合（§8.4） | 同じ照合（§8.5） | `check_live_org` | POST は 409、run は開始せず blocked |
| 2 | coding 課の profile | `coding_node` の EffectiveProfile → `task_core::variant_wire::effective_profile_sha256` | `spec.org.effective_profile_sha256` = B0 coding 課 | 照合 | 照合 | `check_pin` | 同上 |
| 3 | 他の全 node の profile（spec-on の非 coding 課、両 variant の根 `cos`） | 同じ `org_list()` の全 node | B0 入力（承認済み sha256）の `org` の同じ node の profile。spec-off で消えた node は対象外、根 `cos` は一致必須 | 見ない（pin に欄が無く、wire を変えない） | 全 node の digest を観測値として記録（§8.5） | B0 org から同じ関数で digest を作って比較 | 記録が無い・一致しない試行は未適用（`Refused`、集計しない） |
| 4 | 全役割のモデル（planner / reviewer / cos / worker。builtin harness の run を含む） | config の `[harnesses.*]`・`[route_defaults.*]`・`[harness_selection]`・`[roles]`、DB の model catalog の上書き・`model_role_assignments`（0053/0054）・route store（0071）。結果は run ごとの `runs.selected_*` | `spec.non_target.role_models`・`configs`（= lock `execution.configs`）、env-lock の arm `catalog` | 見ない | 選ばれた harness・source・model を記録（既存の `runs.selected_*` と同じ値） | run ごとに `selected_model_id` を `role_models[役割]` と、harness を lock configs と、source×model を env-lock catalog と照合する。役割の対応: planner run → `planner`、reviewer run → `reviewer`、CoS run → `cos`、作業 run → `worker` | 不一致・null は未適用 |
| 5 | 起動時 config | `identity.config_sha256`・`instance_id`・`release`・`overrides` | 承認済み測定 config の sha256、manifest の B/C release | — | `instance_id`・`config_sha256` を記録 | preflight 時の値と各 run の記録が同一か（再起動・別 daemon を検出） | null（file 以外から起動）は variant 付き task では明示失敗 |
| 6 | 情報（KB・資料・記憶） | `identity.effective` の `knowledge_root`・`knowledge_default_mounts`・`memory_dir`・`langmem_enabled`・`knowledge_gc_enabled`・`cron_seed_count`。試行 KB は `trial_scope` | lock の `kb_snapshot`・`materials_sha256`、測定 config | — | `trial_scope` の KB root を記録 | `env_check`（effective の照合）と campaign `isolation` の verify | 失敗 |
| 7 | 認可・安全（approvals・deny_tools・tools・browser・max_lane・allowed_tiers・max_attempts） | 全 node の EffectiveProfile | `spec.non_target.safety`（coding 課由来）。他 node は #3 | #2 の範囲だけ | #2 と #3 | #2 と #3 | #2・#3 と同じ |
| 8 | review 設定 | profile の `review_harness`・`review_tier`・`review_escalate_on_fail` | `spec.non_target.review` | #2 の範囲 | #2・#3 | reviewer run の selected 値は #4 | 同上 |
| 9 | 予算・終了条件 | daemon は campaign の予算を持たない（task の budget 欄は保存するだけ） | lock の `execution.budget`・`termination` | — | — | campaign の runner・ledger・`budget.rs` が強制する | daemon 側の pin で証明したことにしない |
| 10 | grader | daemon には無い（campaign `grade_runner`） | lock の `cases[].grader`（`GraderPin`） | — | — | `verify_lock`・grade binding | daemon 側の pin で証明したことにしない |
| 11 | 識別 | `task_campaigns`（0088）と `task_campaign_resolve` | `manifest.variant` | `validate`・resend・祖先検査（済） | resolve の `Err` は明示失敗 | lookup の GET で完全一致 | 409 / blocked / `Refused` |

注意: `identity.config_sha256` は起動時に読んだ file の hash で、静的な値である。`reload_providers`（`crates/celeris/src/daemon/admin.rs:189`）は route registry・harness adapter・roles・genres・委譲設定を差し替えるが、`identity` は変わらない。DB 側の routing（catalog の上書き・役割割当）も file hash の外にある。だから #5 の一致を #4 の証明にはしない。#4 は run ごとの selected 値だけで判定する。

### 8.4 通常 POST の入力と所有 path（api-atomic が実装）
- 入力: 要求の `campaign`（`CampaignIdentity`）と、同じ store lock の中で読む `org_list()`。VariantSpec などの封印証拠は入力にしない（daemon は持たない）。
- 所有: `crates/task-api/src/handlers/tasks.rs` の作成経路（既存の `campaign_resend` → transaction 内の resend 判定 → `task_campaign_set_tx` の順を保つ）。node 集合と coding 課 hash の照合関数は task-core（`variant_wire` の隣）に置き、dispatcher と同じ関数を使う。
- 失敗: 片欠け・形の不正・祖先との不一致は既存の拒否（済）。`variant_org` が稼働 org と一致しない場合は 409 で、task・events・識別・冪等キーを残さない。org の照合は task 作成と同じ transaction の前段で行う。

### 8.5 run 開始時の入力と所有 path（dispatch-pin・worker-pin が実装）
- 入力: `task_campaign_resolve` の結果、その tick の `org_list()`、既存の route 決定（harness・source・model）、daemon の `identity`（`instance_id`・`config_sha256`）、`trial_scope`。
- 照合の所有: `crates/task-dispatch/src/dispatcher/dispatch_run.rs`。`assign_if_needed` の前と run 起動の直前で、§8.4 と同じ task-core の関数を呼ぶ。assignee の検査（spec-off は `coding_node` だけ、retry の既存 assignee は `nodes` に含まれ、spec-off なら `coding_node`）は D5 のとおり。
- 観測値の型と所有: `crates/task-worker/src/protocol.rs` の `RunContext` に variant 付き run だけ `campaign` 欄を足す（D7）。中身は次で確定:
  - `identity`（解決した `CampaignIdentity` をそのまま）
  - `org_nodes_sha256`（全 node の EffectiveProfile を `node_id` 込みで id 順に並べた canonical JSON の sha256。関数は task-core の `variant_wire` に置き、campaign の L2 も同じ関数で B0 org から計算する。依存は campaign → task-core の向きだけ）
  - `selected`（harness・source・model。`runs.selected_*` と同じ値）
  - `daemon_instance_id`・`config_sha256`
- 失敗: resolve の `Err`、org の不一致、assignee の違反、観測値を作れない場合（`config_sha256` が null、profile を解決できない）は、variant 付き task なら run を開始しない。task は blocked にし、理由を event に残す。適用済みとは扱わない。variant の無い task は従来どおりで、何も足さない。
- daemon は観測値を記録するだけで、非対象（#3〜#10）の合否を判定しない（期待値を持たないため）。

### 8.6 campaign 側の照合（L2）の所有
- 期待値: `crates/campaign/src/variant_pin.rs`。既存の `check_pin` に加え、run 単位で照合する関数を同じ module に置く。入力は spec・`BaselinePin`・承認済み B0 org・run の `campaign` 記録・selected 値。
- 読み取り: `celeris_exec.rs` が GET で読む。読む場所は `TaskDetail.campaign`（D4）と run の記録（既存の run 応答の selected 値）。新しい endpoint は作らない。
- 不足（記録が無い・null・照合できない）や不一致があった試行は `Refused` とし、集計に入れない。lookup の `Refused` と同じ扱い。
- 実装は campaign の葉（campaign-bridge と後段）で行い、task-core・task-api・task-dispatch・task-worker の葉と同じ葉にはしない。

## 証拠コマンド
- `git rev-parse HEAD` → `6bb9ecc7…`；`git ls-remote origin refs/heads/main` → `fd9027fb…`；`git log --oneline HEAD..main | wc -l` → 21；`git diff --stat HEAD...main` → 24 files
- `sqlite3 "file:$CELERIS_RUN_DB?mode=ro" "select id,status,updated_at from tasks where id in (…3 件…)"` → ready / cancelled / cancelled
- `git rev-parse celeris/01M58VFRBA6HG0SSF3CG5BKM91` → `cd7f85ca`；`merge-base --is-ancestor` → HEAD・main とも未包含
- `grep -rn "CampaignIdentity {" crates/`、`grep -rn "RunContext {" crates/`、`grep -rn "task_campaign_resolve" crates/` → §7 の一覧
- （audit 時点の「全 branch で 0087 以上なし」は core-store の再走査で誤りと判明。0087 は別 branch が使用 → 0088。§0）
- pin-boundary: `git fetch origin && git rev-parse origin/main` → `254c885a…`；`git log --oneline HEAD..origin/main | wc -l` → 48；`git merge-tree --write-tree --name-only HEAD origin/main` → 衝突は progress 4 件の front matter のみ；`sqlite3 "file:$CELERIS_RUN_DB?mode=ro"` → done / cancelled / cancelled
- pin-boundary の根拠 path: `crates/campaign/src/variant.rs`（`VariantSpec::verify`・`NonTargetPin`）、`variant_pin.rs`（`B0Inputs`・`baseline_pin`・`check_live_org`・`check_pin`）、`preflight.rs:499-545`（`check_variant`）、`env_check.rs`（`identity.config_sha256`）、`crates/celeris/src/daemon/admin.rs:189`（`reload_providers`）、`crates/task-api/src/types.rs:369`（`DaemonIdentityView`）、migration `0071_llm_route_store.sql`（`runs.selected_*`）
- audit・pin-boundary は文書のみの変更のため cargo の検査は回していない（統合時に実行）

## 未解決事項
- `effective_profile_sha256` の task-core 版（struct 由来）と campaign 版（GET JSON 由来）の同値は未固定（campaign-bridge の試験で固定する。core-wire.md）。
- reload（`reload_providers`）による `[roles]`・`[genres]`・`harness_selection` の差し替えは `identity` に出ない。§8.5 の観測でも、run の selected 値に現れない変更（genre の割当先など）は直接は検出できない。専用環境では reload しないことを運用手順に書き、検出の仕組みは本 task では足さない（提案 P2）。
- main（`254c885a`）の取り込み時期は統合側の判断。衝突は progress 4 件の front matter のみ（`git merge-tree`）。
- 適用証明は未実施（後段）。

## 提案
- P1（campaign-bridge・後段の campaign 葉）: §8.2 の `variant-b0.json` を、承認済み sha256 を明示して読む形にする。所有は `variant_pin.rs`・`preflight::check_variant`。
- P2（本 task 外）: reload の発生を daemon が event に残す（`identity` は起動時の静的値のため）。それまでは専用環境の runbook で reload を禁止し、L2 は reload の前後の run を区別できない前提で扱う。
- audit の旧提案（core-wire の範囲 check に task-dispatch 試験・schema・web 生成型を許可する）は、subtree ごとに別葉へ分けた計画（§0）で不要になったので取り下げた。

## 9. 統合検証（verify-record, 2026-10-19）

検証対象 HEAD = `2039b012`（normal-e2e 統合後）。コードは変えていない（本 WU の変更はこの節と `verify-record.md` のみ）。

**最新 main・既存 3 task（再確認）**: `git fetch origin` 後の `origin/main` = `cb0cd7b8`（HEAD より先、`git diff --stat HEAD...origin/main` = 235 files）。main は未取り込み（取り込みは統合側の判断）。既存 task は `sqlite3 "file:$CELERIS_RUN_DB?mode=ro"` の読取で `01M58VFRBA6HG0SSF3CG5BKM91` = done（2026-10-19T17:06:28Z、main に統合済み）、`01M58VG5C4XGK4E70M6WJ6MEX0`・`01M58VGDNQ8GWGJPB8SJ73XSPF` = cancelled。重複実装なし。

**実測**
| 検査 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | exit 0。`CELERIS_TEST_SUMMARY` passed 5916 / failed 0 / ignored 15（nextest 244 binaries + doctest 13、nextest 295.4s） |
| `cargo test -p celeris --test variant_propagation` | 5 passed（通常 POST・冪等・pin・子孫・fail closed。spec-on/off） |
| `cargo test -p task-dispatch variant_pin` | 17 passed（0 件一致ではない） |
| 生成物の差分ゼロ | `UPDATE_SCHEMA=1 cargo test -p task-api` / `-p task-worker` 後に `git status --short` が空（`docs/api/v1`・`docs/protocol`・`web/api/generated` に差分なし） |
| 文書検査 | `check-adr-numbers.sh` 0、`check-doc-links.sh` 0、`check-doc-layout.sh scripts/dev/docs-layout.tsv` 0、`check-architecture-map.py` 0 |

注: 素の `cargo test -p task-api` / `-p task-worker` は browser live 系（`browser_live_cross`・`launcher_run::live_frame_tests`）が sandbox の userns 不可で失敗する（変更と無関係。ADR-0126。`test-parallel.sh` では全通過）。

**型・callsite 順**: §7 のとおり（core-wire → core-store → core-callsite-fix → core-api-schema → core-web-types → … → worker-pin → dispatch-worker-pin → api/celeris-worker-callsite → protocol-schema）。celerisctl は default 展開で変更不要（ctl-worker-callsite は superseded）。`cargo clippy --workspace`（celerisctl 含む）が通ることで統合後の build 成立を確認。

**封印証拠・非対象不変性**: campaign-proof（`variant_proof.rs`、lib 47 passed）が全 task・全 run・org 全 node digest・env 固定を照合し、normal-e2e が共有 org 不変・pin 外 assignee/org drift/観測なしでの run 起動拒否を実測済み。

**未解決（要後続）**
- **daemon 本体が `Dispatcher::set_variant_observation` を呼ばない**ため、実 daemon では variant 付き task の run は常に blocked（fail closed）。daemon 起動時に instance id・config sha256 を渡す配線が別途必要（`crates/celeris/src`、別 subtree の葉）。
- `FixedEnv`・env-lock catalog・B0 を `prove_variant` の caller へ渡す配線は未実施。
- 実 LLM を使う適用証明は未実施（本 task は無 LLM のみ）。

**人の運用手順（本番操作はしていない）**: 専用環境は本番と別の db・port・workspace の測定 daemon（`measure-daemon.example.toml`）で起動し、reload しない。本番 org・config は変更しない。
