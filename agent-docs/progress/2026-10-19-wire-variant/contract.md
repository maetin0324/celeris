---
task: 01M59P79S6NTGRN3X7SQDV9C0G
work_unit: baseline-contract
status: done
completed: 2026-10-19
---
# 専門分担 variant の境界契約（baseline-contract）

測定値・候補判断は書かない。機構は `crates/campaign/fixtures/ablation/selection.json`（sha256 `b6132a44…64bc`、変更しない）で固定済みの `specialization` のみ。

## 1. 最新 main と既存 3 task の照合（受け入れ 0）
- main: `git fetch origin` 後の `origin/main` = `c9654bff56332c160523d904b2248ae527d9b820`（fix-pilot-wiring）。本 WU の HEAD/base = `7010b743`（select-mechanism の統合。main より先）。2026-10-19 に読取。
- 状態の読取元: `~/.config/celeris/config.toml` の `[db] path = /local/celeris/data/db/celeris.sqlite3` を `celerisctl --db <path> show <id> --json` で読んだ（DB 既定は未設定だが上記で読めた。推測していない）。時点は各 task の `updated_at`。

| task | 状態 | updated_at | 実装 commit（main 上・再利用） | 試験証拠 | 重複回避 |
|---|---|---|---|---|---|
| 01M58VFRBA6HG0SSF3CG5BKM91 設定確定 | running | 2026-10-19T09:01:44Z | manifest（`manifest.rs` CampaignManifest/Config/ArmRouting）、lock（`lock.rs` CampaignLock/ExecutionPin/verify_lock/check_successor）、env-lock（`env_lock.rs` `celeris-env-lock/3`）、preflight（`preflight.rs`）、bc-probe 系 `8bacf303` `bf05d7dd` `9567eb1b` `bc2ae973` | `crates/campaign/tests/{campaign_lock,env_lock,preflight_contract,lock_cli}.rs` ほか。本 WU では再実行していない（未確認＝running 中の証拠は未読） | manifest/lock/env-lock/preflight を作り直さない。variant は既存 hash の入力に 1 欄足すだけ |
| 01M58VG5C4XGK4E70M6WJ6MEX0 無LLM統合 | ready | 2026-10-19T08:21:04Z | runner（`runner.rs`）・`CelerisExecutor`（`celeris_exec.rs`）・`50fee036` | `mock_e2e.rs` `celeris_exec.rs` `lock_execution.rs`。実行証拠は未読（未実施の ready） | 通常 POST・再開・冪等照会の経路を作らない。variant 欄を載せるだけ |
| 01M58VGDNQ8GWGJPB8SJ73XSPF 主比較 pilot | ready | 2026-10-19T08:21:04Z | pilot 配線 `pilot_wiring.rs` `pilot_fixtures.rs` | 実 pilot は未実施（ready）。未確認 | 本 variant 比較（`M1-specialization`）は A0/B0・B1/C1 と別。互いに依存を足さない |

未確認: 3 task の run 内ログ・実測証拠（本 WU では読んでいない）。

## 2. spec-on / spec-off の定義と EffectiveProfile 同値（受け入れ 1）
切替点は org（DB が正。seed は `org.toml`、変更は `POST/PATCH /api/v1/org`）だけ。task-template・`routing_override`・profile の新欄・新 scheduler/router は作らない（`check_template` が禁じる欄は据え置き）。
- `M1-spec-on`: B0 と同じ組織（測定用 seed）。担当は `task_ops::matching::decide` が org から決定的に選ぶ。
- `M1-spec-off`: 根 `cos` と、その直下の単一の非根作業課 1 つ。作業課の `EffectiveProfile`（`task_core::profile::EffectiveProfile` の `harnesses_allowed` `harness_default` `tier` `allowed_tiers` `max_lane` `max_attempts` `review_harness` `review_tier` `review_escalate_on_fail` `tools` `deny_tools` `permissions/approvals` `knowledge` `skills_mounts` `browser` `run` `policy`）は、B0 で coding genre を受ける課の `EffectiveProfile` と**同値**（`node_id` と `chain` だけが違ってよい）。根 `cos` は候補にならない（`the_root_is_never_a_candidate`）。
- 同値の判定: `effective_profile_sha256` = `node_id`・`chain` を除いた `EffectiveProfile` を canonical JSON（キー整列・compact）にした sha256。spec-on 側は B0 の coding 課から、spec-off 側は単一課から計算し、不一致なら variant を拒否する。
- 伝播: retry は assignee を保持（`decide` が assignee 有りを `NotApplicable`）、子孫は単一 node へ。support harness（builtin）は org 外のまま。

## 3. 責務境界と次段へ渡す共有型
所有は `campaign` crate（新 `crates/campaign/src/variant.rs`）。task-core/API は「識別欄を運ぶ・比較する」だけで、variant の意味を知らない。

共有型（具体名。型は後続 WU variant-contract で実装する）:
- `variant::VariantId` = `SpecOn`（`"M1-spec-on"`）| `SpecOff`（`"M1-spec-off"`）。比較 ID 定数 `COMPARISON_ID = "M1-specialization"`。
- `variant::VariantSpec`（schema `celeris.campaign-variant/1`）: `comparison_id`、`variant: VariantId`、`selection_sha256`（`selection.sha256` の値と一致）、`org: OrgPin { nodes: Vec<String>, coding_node: String, effective_profile_sha256: String }`、`non_target: NonTargetPin`、`sha256`（内容 hash。空なら未封印）。
- `variant::NonTargetPin`: 非対象を固定する値の写し — 全役割モデル/harness（`ExecutionPin.configs`）、資料（`snapshot.materials_sha256`）、KB（`snapshot.kb_snapshot`）、予算（`ExecutionPin.budget`・`termination`）、安全/承認上限（`EffectiveProfile` の permissions/approvals/deny_tools/browser/max_lane/max_attempts）、grader（lock の `GraderPin`）。
- `VariantSpec::content_sha256()` は `manifest::compute_sha256` と同じ作り（`sha256` を空にして `manifest::canonical` → compact JSON → sha256）。`seal()` / `check_seal()` は `CampaignLock` に倣う。
- `variant::VariantRef { comparison_id, id, content_sha256, selection_sha256 }` — manifest・lock・POST・GET が運ぶ短い参照。

配線（次段、すべて「省略時は従来どおり」）:
1. manifest: `CampaignManifest.variant: Option<VariantRef>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`）。省略した既存 manifest の `compute_sha256` は変わらない。`validate` が `selection_sha256` を同梱の `selection.sha256` と、`content_sha256` を `VariantSpec` の再計算と照合。
2. lock: `ExecutionPin.variant: Option<VariantRef>`（同じ skip 規則）。`bind_to_manifest` の `ExecutionPin::from_manifest(m) != lock.execution` で自動的に不一致を拒否。`check_successor`（再 seal）は variant の差し替えを拒否する（非対象が同じでも variant 欄の変更は後継として認めない）。
3. POST: `POST /tasks` の `campaign`（`task_core::decision_log::CampaignIdentity`）に `variant: Option<VariantRef>` を追加（`deny_unknown_fields` のため task-core の変更が要る。次段の task-core/API WU）。`CelerisExecutor::body` が manifest の `variant` から `campaign.variant` を載せる。`campaign_idempotency_key` の再送判定（既存 `task_campaign_resend`（`crates/task-api/src/handlers/tasks.rs` の `campaign_idempotency_key` 経路））に同じ欄を含め、同じ試行で variant が違えば 409。
4. GET 照合: `CelerisExecutor::lookup`／`tasks_with_label` が返す task の `campaign.variant` を期待値と比較。欠落・不一致は既存 task として採用せず `ExecError::refused`（適用済み扱いしない）。再開（`resume`）も同じ照合を通る。
5. preflight（GET のみ）: `GET /api/v1/org` の node 集合と `effective_profile_sha256` を `VariantSpec.org` と照合。不一致は有料実行 0 件で止める。org の書き換え（`POST/PATCH /api/v1/org`）は測定専用 daemon の運用手順で、本番 host 操作は人が行う。
6. 子孫・retry: 識別は祖先の鎖から継ぐ既存規則（ADR 2026-10-16-campaign-identity-propagation）。variant も同じ経路で、子孫へ新規配線は不要。

拒否する不一致（`VariantError` として次段で列挙）: 選択 hash 不一致、内容 hash 不一致（改変）、再 seal で非対象（モデル/harness/資料/KB/予算/安全/grader）が差を持つもの、`effective_profile_sha256` 不一致、variant の欠落（manifest にあるが POST/GET に無い）、再開・冪等照会での variant 不一致。延期の review-off / KB-off は扱わない（`selection.json` の `deferred`）。

## 4. 次段 WU への引継ぎ
- variant-contract: `variant.rs` と manifest/lock の `variant` 欄、hash 安定性試験。
- campaign-submit: `CelerisExecutor::body`/`lookup` と task-core `CampaignIdentity.variant`、task-api の 409 比較。
- adversarial-tests: `crates/campaign/tests/ablation_contract.rs`（hash 安定性、内容改変・再 seal・欠落・再開/冪等不一致）。
- 本 WU はコード変更なし。cargo の検査は不要のため実行していない（変更は文書のみ）。

## 未解決事項
- 3 task の run 内実測証拠は未読。
- `GET /api/v1/org` の応答が `EffectiveProfile` 同値の計算に十分かは variant-contract で確認（足りなければ `org_list` の応答を使う側で derive する。新 endpoint は作らない）。

## 提案
- なし（契約は上記の範囲。ADR は実装で決定が確定した時点で `agent-docs/adr/2026-10-19-specialization-variant.md` に書く）。
