---
title: 健全性v2 R5 dedupe-api — task-api の試験の重複 setup・同値 scenario・旧実装の固定を整理する
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 dedupe-api — task-api の試験の重複 setup・同値 scenario・旧実装の固定を整理する

完了日: 2026-11-11

WorkUnit `dedupe-api`。[`contracts.json`](contracts.json) の task-api の候補のうち、R5 の write set の中で扱えるものを片付けた。基準は `$CELERIS_WU_BASE`（slices-check は `git merge-base HEAD main`）。番号 D・E は [assemble.md](assemble.md) と同じ（`candidates.duplicate_setup`・`equivalent_scenario` の 1 始まりの順）。

## 結論
- `duplicate_setup` の task-api の 32 組のうち、member の file が全部 write set に入る 28 組を片付けた。各 file の写しの helper を消して `tests/common` の 1 つに寄せた。試験関数は 1 本も消していない（`cases.py` は 855 → 855、表の行は 757 → 757）。
- 消した局所の helper は 109 個、`tests/common` に足した helper は 12 個。`crates/task-api/tests` の 46 file で 1822 行を消し、1160 行を足した（足した行の多くは、呼び出しの名前が長くなって rustfmt が折り返した行）。
- `equivalent_scenario` の E4（`chat_attachments.rs` の 2 本）は束ねていない。理由は「束ねなかった候補」の節に書いた。
- `old_impl` の task-api の候補 `old-console-milestone-review-block` は `task-api/src/console.rs` の本体を変えるので write set の外で、扱っていない（ADR D5-6）。
- 決定 `test-removal` の答えは a（証拠付きで消してよい）だが、この unit で消した試験関数は 0 件。`removals/task-api.json` は空の配列で置いた。

## 変更
- `phase 1: dedupe-api: task-api の試験の写した helper 28 組を tests/common に寄せる` — 下の表の helper を寄せた。限定 mutation の patch 3 件も同じ commit に入れた。
- `phase 1: dedupe-api の進捗を書く` — この file。

### tests/common に足した helper
| helper | 置き場所 | 片付けた候補 |
|---|---|---|
| `put_admin`・`delete_admin` | `common/mod.rs`（`patch_admin` の隣） | D22・D33 |
| `web_operator_env(web_token, operator_token)` | `common/mod.rs` | D20（3 file の `env()` は本文が同じで、token の値だけが file ごとに違う。値は引数で渡す） |
| `seed_secretary`（`post_admin` で書き直した） | `common/mod.rs` | D24 |
| `job_body` | `common/mod.rs` | D19 |
| `git` | `common/mod.rs` | D12 |
| `playwright_headless_shell`（元の名前は `browser`） | `common/mod.rs` | D02 |
| `cos_run_bearer(env, thread_id, key)`・`issue_bearer(env, thread_id, run, policy_version)` | `common/cos_ops.rs`。`cos_bearer` はこの 2 つを呼ぶ形に直した | D14・D36 |
| `post_op`・`decision_answered_by`・`raise_decision` | `common/cos_ops.rs` | D40・D11・D44 |
| `ago`・`ingest_plan_gate`・`conforms` | 新しい `common/plan_conforms.rs` | D17・D18・D48 |

既にあった helper を呼ぶ形に直した候補: D03・D27・D45（`admin_env`）、D05・D30（`get_admin`）、D35・D32（`post_admin`）、D15（`patch_admin`）、D10・D31（`admin_headers`）、D07（`cos_ops::op_body`）、D13（`cos_ops::cos_bearer`）、D38（`cos_ops::db`）。

候補の外で、同じ本体だった helper も同じ変更で寄せた: `browser_site_policy_db.rs` の `put`・`del`・`post`・`patch`・`auth`（D30 の `get` と同じ形）、`project_auto_integrate.rs` の `patch`（`patch_admin` と同じ要求）、`organization.rs` の `headers`（D31 の `auth` の変換だけに使っていた）、`notify.rs` の `auth`、`cos_chat_attach_pin.rs` の `const OPS`（`post_op` を寄せて使われなくなった）。

試験関数の本体は、呼び出す helper の名前を変えただけで、要求・env・assert を変えていない。寄せた helper の本体は候補の `body_sha256` が一致した写しで、`cos_bearer` だけは `common::project_cos_thread` と `super::project_cos_thread`（同じ関数）の違いがあった。`web_operator_env` は 3 file の本文が同じでも定数 `WEB_TOKEN`・`OPERATOR_TOKEN` の値が file ごとに違うので、値を引数にして各 file の定数を渡す。

### 束ねなかった候補
| 候補 | 理由 | 次の扱い |
|---|---|---|
| E4 `chat_attach_api_rejects_duplicate_file_fields`・`chat_attach_api_requires_client_upload_id` | `chat_attachments.rs` は B0 capability-map の `cos-chat-threads-runs` の `tests` に載る。`cases.py` の規則 3 は capability-map の `tests` の file の試験関数の減りを、removals があっても拒む。段 bundle がこの file を `cos_delivery_suite_a/` へ動かしたので、規則 3 は今 path で照合できない（`cases.py` が「capability-map の crates/task-api/tests/chat_attachments.rs は base に無い（照合しない）」と出す）。照合できなくなったことを使って消すのは検査器を弱めるのと同じなので、束ねない。束ねた版を一度書いて compile まで確かめたが、元に戻した | 提案 1・2 |
| D01（`count`）・D16（`env_with_attachments`）・D41（`bridge_binary`） | member に数字を含む file（`cos_live_fix_d1.rs`・`cos_live_fix_d2.rs`・`browser_h3_injection.rs`）がある。write set の外（ADR D5-6） | R6 か別の task |
| D46（`seq`） | member が `task-api/src/cos/basis/` の試験でない src の file の inline module。write set の外 | R6 か別の task |
| `old-console-milestone-review-block` | 案は `task-api/src/console.rs` の途中目標の節を消す変更と同じ変更で行う。src の本体は write set の外 | R6 |

D01・D16 は write set の中の file（`general_cos_consult.rs`・`unblocks.rs`・`cos_chat_attach_handoff_kb.rs`・`cos_chat_attach_pin.rs`）だけを寄せることもできるが、写しが common と数字を含む file に分かれて残るので、組ごと R6 に回した。

## 限定 mutation（寄せた helper を使う残す試験が、対象 module の変異を落とすこと）
試験関数を消していないので removals の項目は無い。寄せた helper（env・要求・bearer・plan gate の fixture）を通る候補の `keep_tests` が、`mutation_target` の module の変異を今も落とすことを、多い組から 3 件確かめた。3 件とも `sh agent-docs/progress/2026-11-10-health-v2-r5/mutate.sh <patch> task-api '<filter>'` が `KILLED`（nextest exit 100）を返し、src は戻った（`git status --short crates/task-api/src` が空）。

| patch（`mutations/task-api/`） | 変異 | 対象の候補 | filter の試験 | 結果 |
|---|---|---|---|---|
| `dedupe-admin-token.patch` | `middleware.rs` の `resolve_principal` が token の無い要求を 401 にせず、`require_admin` が未認証を通す | D03・D05・D15・D22・D27・D31・D33・D35 など（`middleware.rs` の 8 組） | `execution::posting_without_a_token_is_unauthorized`・`knowledge::the_write_endpoints_are_admin_only`・`repos::changing_repos_needs_the_admin_token_but_reading_does_not`・`skills::put_skill_creates_and_updates_and_requires_admin`・`approvals::admin_operations_require_a_token_when_one_is_configured`・`organization::org_admin_endpoints_require_a_token` | KILLED（6 run、6 failed） |
| `dedupe-cos-bearer-thread.patch` | `cos/mod.rs` の `CosCaller::from` が credential の thread_id を捨てる | D13・D14・D36 | `project_brief::project_status_put_is_written_by_the_project_cos_only`・`project_controller::project_wake_api_cos_may_only_turn_it_off`・`project_roadmap::project_roadmap_cos_and_operator_cannot_decide`・`general_cos_consult::general_cos_consult_proposal_is_cos_words_and_never_an_instruction`・`single_human_window::project_escalation_is_answered_through_the_general_cos_and_relayed_to_the_project_cos`・`cos_operations::cos_chat_ops_api_idempotency_same_hash_same_operation_different_hash_409` | KILLED（6 run、6 failed） |
| `dedupe-plan-conforms-plan-id.patch` | `cos/basis/plan_conforms.rs` が待っている plan と違う `basis.plan.plan_id` を通す | D17・D18・D48 | `cos_plan_conforms::plan_conforms_approves_a_plan_whose_sensitive_stages_are_guarded`・`plan_conforms_refuses_other_answers_stale_versions_and_unguarded_stages`・`cos_plan_conforms_guards::a_failed_plan_check_stays_with_a_person_and_names_its_kind` | KILLED（3 run、1 failed: `plan_conforms_refuses_other_answers_stale_versions_and_unguarded_stages`） |

filter は試験名の末尾の正規表現の和（例 `test(/posting_without_a_token_is_unauthorized$/) | …`）。mutate.sh の出力の要点:
- `KILLED: …/mutations/task-api/dedupe-admin-token.patch（nextest exit 100）`、`Summary … 6 tests run: 0 passed, 6 failed, 872 skipped`
- `KILLED: …/mutations/task-api/dedupe-cos-bearer-thread.patch（nextest exit 100）`、`Summary … 6 tests run: 0 passed, 6 failed, 872 skipped`
- `KILLED: …/mutations/task-api/dedupe-plan-conforms-plan-id.patch（nextest exit 100）`、`Summary … 3 tests run: 2 passed, 1 failed, 875 skipped`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`、slices-check は `git merge-base HEAD main`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$CELERIS_WU_BASE"` → exit 0（試験関数 855 → 855、表の行 757 → 757、不変条件の試験 229（33 file）、removals 0 件）
- `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web。nextest `5860 tests run: 5860 passed (50 slow), 22 skipped`、web の vitest `Test Files 104 passed`・`Tests 712 passed`、doc test は全部 ok。test-affected の中の repo-health は `verdict=pass`（base は `git merge-base HEAD main`、fail_grade=0・new=3・worsened=0。3 件は WU base の時点で既にあり、下の WU base 基準では new=0）
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（`verdict=pass`、findings=720、new=0、worsened=0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `crates/task-api/tests/` の 47 file（`common/plan_conforms.rs` の追加を含む）と、この dir の `dedupe-api.md`・`removals/task-api.json`・`mutations/task-api/` の 3 patch。数字を含む file 名は無い
- task-api の試験の build は警告 0（未使用の import は `cargo fix -p task-api --tests` と手で消した。test-affected と mutate.sh の build の log に task-api の警告は無い。log にある警告は celeris の `script_exhaustive` の未使用 import と deno_core で、この unit は触れていない）
- `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (415 files)`）。`sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）

## 未解決
- E4 は束ねていない（上の表）。
- `cases.py` の規則 3 は B0 capability-map の path で照合するので、段 bundle が動かした task-api の 9 file（`api_principals.rs`・`chat_attachments.rs`・`cos_auth.rs`・`cos_ops_registry.rs`・`cron_jobs.rs`・`integration_api.rs`・`legacy_routing_events.rs`・`routing.rs`・`routing_stage.rs`）の試験関数は今は守られていない。この unit はこの 9 file の試験関数を消していない（`cron_jobs.rs`・`integration_api.rs`・`routing.rs` は helper の呼び出しだけを直した）。
- write set の外の D01・D16・D41・D46 と `old-console-milestone-review-block` は R6 か別の task に残る。

## 提案
1. `cases.py` の規則 3 が、束ねで動いた file を新しい path でも照合するようにする（`tests/<bundle>/<target>.rs` を元の `tests/<target>.rs` として引く。`classify.py --verify-leaf` と同じ対応）。B0 capability-map の path を書き換える案もあるが、B0 の記録は動かさない方がよい。直すまでは、段 dedupe と段 final の unit は上の 9 file の試験関数を消さない。
2. 「消してはいけない試験」の範囲を 1 つに決める。ADR D5-2(c) と B0 slices.json の停止条件は `invariants[].evidence_path`（と `invariant=true`・地図の関連試験）を挙げ、`cases.py` は capability-map の `capabilities[].tests` の file も守る。後者なら E4 は束ねられない（4 件の equivalent_scenario のうち capability-map の `tests` に載る file は E4 だけ）。前者なら `cases.py` の規則 3 を `invariants[].evidence_path` に絞る変更が要り、それは検査器の変更なので別の review に出す。
3. `tests/common/mod.rs` は `#![allow(dead_code)]` のまま。束ねで 1 binary の中の利用者が増えたので、使われない helper の検出は R6 以降で `dead_code` を module ごとに戻して確かめる。
