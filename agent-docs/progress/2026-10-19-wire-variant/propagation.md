---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 通常API・永続化・dispatcherへのvariant伝播（normal-propagation）
work_unit: audit
status: running
updated: 2026-10-19
---
# 通常 API・永続化・dispatcher への variant 伝播（propagation）

親: 「選択機構だけの variant 設定と伝播を配線」（段階 propagate）。前段の契約は同 dir の `contract.md`（型・hash・投入/照合の配線まで）と `integration-fix.md`（統合失敗の診断。コード欠陥なし）。

**適用証明（実 org 適用・子孫 Assigned の実測・run 開始時の構成証拠）は後段（normal-e2e / verify-record）まで未実施。本節（audit）はコードを変えていない。**

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
2. `task_campaigns` に variant の列が無い（migration 0085 は 6 欄）。空き番号は **0087**（全ローカル branch の `crates/task-core/migrations/` を走査し 0087 以上は無し）。
3. **GET `/api/v1/tasks/{id}` は campaign 識別を返さない**。`TaskDetail`（`crates/task-ops/src/view.rs:164`）にも `Task`（`task-core/src/model.rs:448`）にも campaign 欄が無い。一方 `celeris_exec.rs:283` は `detail["task"]["campaign"]["variant"]` を読む → 現状では variant 付き再開・冪等照会は常に `Refused`。返す場所と読む path を揃える必要がある（§5 D4）。
4. 祖先整合: 子孫は行を持たず最寄り優先で読むだけ。子孫に行を付ける経路（`task_campaign_set` は parent を見ない）で root と食い違っても検出しない。
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

- **D1 保存**: migration `0087_task_campaign_variant.sql` で `task_campaigns` に `variant_json TEXT`・`variant_org_json TEXT`（NULL 可）。`decision_log_runs` に `variant_id TEXT`・`variant_content_sha256 TEXT`（arm 分割用）。`task_campaign_set_tx` / `own_tx` / `own` / `resend_tx` は新欄を読み書きし、resend の Same 判定は新欄込みの全欄一致（variant だけ違えば 409 `campaign_identity_conflict`）。
- **D2 祖先整合**: `task_campaign_set_tx` は対象 task の祖先鎖に識別があり、それと 1 欄でも違えば `StoreError::Invalid`（root と子孫の不一致を拒否）。`task_campaign_resolve_checked`（新）: 鎖全体を辿り、行を持つ全祖先が一致しなければ `Err`（最寄り優先で矛盾を隠さない）。dispatcher と run_context はこちらを使う。既存 `task_campaign_resolve` は checked に置き換えて消す（破壊的変更の方針。利用者 3 箇所＋試験）。
- **D3 通常 POST**: 既存の `campaign_resend` → tx 内 `task_campaign_resend_tx` → `task_campaign_set_tx` の順をそのまま使い、`validate`（D1 の片欠け拒否含む）を tx 前に行う。新 endpoint・新 scheduler/router は作らない。
- **D4 GET**: `TaskDetail` に `campaign: Option<CampaignIdentity>`（`task_campaign_resolve_checked` の結果。祖先矛盾は `null` に丸めず API problem で返す）を足し、campaign 側 `celeris_exec.rs` の読む path を `detail["campaign"]["variant"]` に変える（campaign-bridge）。`Task` 本体には足さない（`Task` literal が多く、識別は継承で決まる値のため）。
- **D5 dispatcher（dispatch-pin）**: `assign_if_needed` の直前に、識別に `variant_org` があれば `org_list()` から node 集合と `coding_node` の effective profile hash を計算し `CampaignOrgPin` と比較。不一致（org drift・非対象差分）は割当てず task を `blocked`＋理由 event（適用済み扱いしない）。一致なら既存 `matching::decide` をそのまま呼び、spec-off（`nodes` が根＋1 node）では結果の node が `coding_node` でなければ拒否。assignee 既存（retry）は `decide` が `NotApplicable` を返すので、その assignee が `nodes` に含まれ spec-off なら `coding_node` であることを検査。
- **D6 系譜（dispatch-lineage）**: 子孫・retry・continuation・repair は行を作らず D2 の checked 解決で継ぐ（新規配線は「呼び出しを checked に替える」と「子生成時に親の識別が解決できるか確認」だけ）。子 task 作成経路（`child_tasks.rs`）で識別を新規に付ける箇所があれば D2 の検査を通す。
- **D7 worker（worker-pin）**: `RunContext` に `campaign: Option<CampaignRunPin { identity: CampaignIdentity, org_observed_sha256 }>`（serde default・skip）。dispatcher が run 開始直前に D5 と同じ照合を再実行して値を入れる（run 開始時の drift も拒否）。worker は値を run の証拠（result 側の記録）へ写し、continuation/再開時は保存済みの pin と一致しなければ明示失敗。

## 6. 専用試行環境の所有と org snapshot の受渡し点（受け入れ 1）

所有者: **campaign runner が起動する測定専用 daemon**（本番と別の `db`・`api.listen`・`workspace_root`・KB・記憶。`measure_env::check_config` が通常 config との分離を検査）。自動試験では試験自身が tempdir に作る daemon（`campaign_wiring_e2e.rs` の `Daemon::start` と同型）が所有する。**共有 daemon（本番）の org は読みも書きもしない**。variant ごとに org を書き換えず、1 daemon = 1 variant の org で固定する（spec-on と spec-off は別 daemon・別 DB）。

| # | 受渡し点 | 誰が → 誰へ | 値 | 書換えの有無 |
|---|---|---|---|---|
| 1 | org の用意 | 試験 harness / 人の運用手順 → 専用 daemon の空 DB | config の org seed（`seed_org_if_empty`。空 DB のときだけ入る）。spec-off = 根 `cos`＋単一作業課 | 専用 DB にだけ書く。自動試験では許可。本番 org（`~/.config/celeris`・本番 DB）は不可 |
| 2 | preflight | 専用 daemon `GET /api/v1/org` → campaign `OrgSnapshot::from_get_org` → `check_pin` | `nodes`・`coding_node`・`effective_profile_sha256` を `VariantSpec.org` と照合 | 読むだけ。不一致なら有料実行 0 件で停止 |
| 3 | 投入 | campaign `CelerisExecutor::body` → `POST /tasks` | `campaign.variant`（`VariantRef` → `CampaignVariantRef`）＋ `campaign.variant_org`（`OrgPin` → `CampaignOrgPin`） | task 作成と同じ tx で `task_campaigns` に保存 |
| 4 | 割当前 | store `task_campaign_resolve_checked` → dispatcher `assign_if_needed` 前 | 保存 pin と `org_list()` の再計算値 | 読むだけ。不一致は blocked |
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
1. **core-wire**（task-core）: `CampaignVariantRef`・`CampaignOrgPin`・`CampaignIdentity` 新欄・`validate`・profile hash。**同じ葉で** struct literal 6 箇所（task-core 2 ＋ task-dispatch 試験 4。`variant: None, variant_org: None` の 1 行追加のみ）と `docs/api/v1/api-v1.schema.json`・web 生成型を更新する。理由: `committed_schema_matches_generated` が統合時の test-parallel で落ちるため、型変更と schema を別段に分けると integrate-core が赤になる。範囲 check に上記 path を許可する。
2. **core-store**（task-core）: migration 0087、set/own/resend の新欄、D2 の checked 解決と既存 resolve の置換（利用者 3 箇所は同葉で呼び替え — 置換で消すため）。
3. **campaign-bridge**（campaign）: 明示変換、`variant_org` の投入、`celeris_exec` の GET path を `detail.campaign` へ（D4）。fake 応答の試験も同葉。
4. **api-atomic**（task-api ＋ task-ops view）: `TaskDetail.campaign`、POST の validate・409 試験（variant 違い・片欠け・祖先不一致）。schema 再生成（api-schema）は TaskDetail 変更と同じ段で行う。
5. **dispatch-pin** → **dispatch-lineage**（task-dispatch）: D5・D6。
6. **worker-pin**（task-worker ＋ RunContext literal 一式）: D7。protocol schema 再生成は同段。
7. **api-schema / protocol-schema / web-types**: 上で同段生成した成果の最終再生成と差分ゼロ確認。
8. **normal-e2e**: 専用 tempdir daemon（§6）で通常 POST → 実 dispatcher の `Assigned` が spec-off で単一作業課、retry 保持、drift で blocked を無 LLM で確認。
9. **verify-record**: test-parallel・clippy・fmt と本記録の確定。

## 証拠コマンド
- `git rev-parse HEAD` → `6bb9ecc7…`；`git ls-remote origin refs/heads/main` → `fd9027fb…`；`git log --oneline HEAD..main | wc -l` → 21；`git diff --stat HEAD...main` → 24 files
- `sqlite3 "file:$CELERIS_RUN_DB?mode=ro" "select id,status,updated_at from tasks where id in (…3 件…)"` → ready / cancelled / cancelled
- `git rev-parse celeris/01M58VFRBA6HG0SSF3CG5BKM91` → `cd7f85ca`；`merge-base --is-ancestor` → HEAD・main とも未包含
- `grep -rn "CampaignIdentity {" crates/`、`grep -rn "RunContext {" crates/`、`grep -rn "task_campaign_resolve" crates/` → §7 の一覧
- 全 branch の `git ls-tree … crates/task-core/migrations/` で 0087 以上なし
- 本葉は文書のみの変更のため cargo の検査は回していない（統合時に実行）

## 未解決事項
- `effective_profile_sha256` の task-core 版（struct 由来）と campaign 版（GET JSON 由来）が同値かは未検証（core-wire の試験で固定する）。同値にならなければ campaign 側を task-core 版へ寄せ、GET org の応答形を変えない。
- main（`fd9027fb`）の取り込み時期は統合側の判断。`crates/campaign/src/lib.rs` の mod 行で機械的衝突の可能性。
- 適用証明は未実施（後段）。

## 提案
- §7 の 1: core-wire の範囲 check に task-dispatch の試験 3 file と `docs/api/v1/api-v1.schema.json`・`web/api/generated/*` を許可する（計画の「生成物は別葉」とは、最終の再生成・差分ゼロ確認を別葉に残す形で両立させる）。
