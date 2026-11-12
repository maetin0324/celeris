---
title: 受信箱の一次対応で CoS が答える範囲を「人に残す件の列挙以外すべて」にし、Celeris の外の操作は operator 宛てに分ける（skill v12）
tasks: [01M75722SX158YRMFYPQRVEEDY]
status: done
updated: 2026-11-12
---
# 受信箱の一次対応で CoS が答える範囲を広げ、operator 宛てを分ける（skill v12）

完了日: 2026-11-12

設計判断は [ADR 2026-11-12 cos-triage-scope](../adr/2026-11-12-cos-triage-scope.md)。基準は `git merge-base HEAD main`。

## 変更（attempt 1）
- `phase 1: CoS の一次対応の範囲を人に残す件の列挙以外に広げ、operator 宛ての経路を足す（ADR・basis・照合・skill v12）`
  - ADR 2026-11-12-cos-triage-scope（D1 人に残す件の列挙、D2 operator 宛て、D3 task_scope と根拠、D4 決定的な照合、D5 常設の節、D6 計画の承認と決定、D7 撤去）。
  - task-api `cos::basis`: `BasisCategory::DocumentedDefault`・`WorkGuidance` を消し `TaskScope` を足した（refs に根拠が要る。`BasisRefKind::Task` を足した）。
    認可（authorization）への回答は `job_wait_extend`・`within_approved_frame` だけにした。監査の reason は「CoS の判断（<category>）」。
  - task-ops `triage_match`: 件の種類と本文・ledger の grant から `reserved`（人に残す件）と `candidates`（定型の候補と grant）を決定的に出す。LLM なし。
  - task-api `cos::inbox`: escalation packet に必須の `to`（`human`・`operator`）。`operator` は経路だけ取り、通知（Discord）と統括の札を作らない。
    `reserved` の件への answer と operator 宛ては 422 `cos_reserved_for_human`。`GET /api/v1/cos/inbox` の item に `addressee`。
  - task-core migration 0124: `cos_notification_routes.route` に `operator` を足した（表の作り直し、行は写す）。
  - skill cos-inbox-triage を version 12、`[cos.triage] policy_version` の既定を "12"（cos.rs・example.toml・docs/ops/cos-chat.md）。

## 変更（attempt 1 の Run #2）
- `phase 1: 件ごとの照合の行を CoS の prompt に付け、常設の節に方針 12 の要点を置き、/cos/operations でも人に残す件を断る`
  - task-dispatch `cos_chat::triage_hint`: CoS run に渡す受信箱の件ごとに `task_ops::triage_match::match_item` を呼び、
    `CosChatInboxItem.triage_match` に 1 行を付ける（thread の案件の grant と global の grant を読む。LLM なし）。worker は「照合（daemon）」の行で出す。
  - task-worker `cos_chat::standing`: 受信箱の件を持つ run の常設の節に `TRIAGE_POLICY`（人に残す件の列挙・operator 宛て・task_scope の根拠。726 字、上限 `POLICY_MAX` 800）。Core には足していない。
  - task-api `/cos/operations`: instructed_by の無い答えでも、照合が人に残す件と判定した待ちは 422 `cos_reserved_for_human`。resolve では回答の検証より先に判定する。
  - web の生成型・worker protocol schema を更新した。
- `phase 1: policy 12 の scenario 試験 (b)〜(f) を足し、計画の承認の本文に承認の理由を出し、schema の版を 124 にする`
  - 計画の承認の件の本文に「承認の理由: …」（`PlanApprovalRequested.reasons`）を足した。照合が `near_limit:` を読む。
  - `SCHEMA_VERSION` を 124 にし、campaign の `SUPPORTED_SCHEMA_VERSIONS` と delivery の migration の試験を合わせた。
- `phase 1: 計画の再判断の説明と skill §9 を policy 12 に合わせ、api.md と進捗を直す`
  - D6: `task_core::chat::plan_rejudge` の条件は答えた主体を見ないので、CoS が決定に答えた後も計画の再判断が起きる（コードは変えず、説明と skill §9 を直した）。
  - `docs/api/v1/api.md` の basis の説明を task_scope・`to`・`addressee`・`cos_reserved_for_human` に直した。

## 受け入れ条件 4 の scenario 試験
- (a) `crates/task-api/tests/cos_job_wait_extend.rs` の `standing_job_wait_grant_from_the_general_thread_answers_the_project_wait_limit`: daemon の照合が global の grant を名指し、CoS が grant を根拠に「延長」と答えて answered（人に回さない）。
- (b)(c) `crates/task-api/tests/cos_chat_triage_scenarios.rs` の `cos_triage_scope_b_c_answers_the_decisions_then_approves_the_plan`: 決定が残る間の承認は 422、決定 2 件に task_scope で答え（推奨・推奨以外）、計画を plan_conforms で承認。operation の記録に policy_version と根拠。
- (d)(e) `cos_triage_scope_d_e_credential_and_beyond_budget_go_to_the_person`: credential の待ちと `near_limit:` の計画は、照合が `credential_input`・`beyond_approval`、回答も operator 宛ても 422、human 宛てで outbox 2 通。
- (f) `cos_triage_scope_f_outside_celeris_goes_to_the_operator_without_discord`: operator 宛ては notifications 0 件、経路 `operator`、札なし、addressee=operator。
- 照合の決定性: `task_ops::triage_match` の試験 2 件、`task-dispatch` の `cos_triage_hints_are_deterministic_store_reads`。

## 証拠（attempt 1。提出時の HEAD ではない）
- `cargo test -p task-ops --lib triage_match` → 2 passed。
- `cargo test --no-fail-fast -p task-api --test cos_routine_answers --test cos_triage --test cos_chat_triage_scenarios --test cos_operations --test cos_review_rework --test cos_integrate_task --test cos_plan_conforms --test cos_job_wait_extend --test authority_ledger --test single_human_window --test cos_operations_domains --test project_wake_human_chat` → 12 本すべて合格（22・3・4・4・8・7・4・4・5・17・13・1 件）。
- test-affected・clippy・fmt・progress-index は未実行（次の run で最終の HEAD で回す）。

## 変更（attempt 1 の Run #3）
- `phase 1: dependents_reopened の件を CoS が見届けで閉じられるようにし、途切れた一次対応の扱いを ADR に書く`
  - task のコメント 1: `InboxKind::observable_while_open`（`dependents_reopened` だけ真）。resolve は開いている件でもこの種類の observe を受ける。照合は `observe の候補`。skill §3 に書いた。試験 `cos_triage_scope_dependents_reopened_is_observed_without_the_person`（observe で閉じ、後続は Ready のまま、決定の observe は 422、通知 0 件）と triage_match の試験。
  - task のコメント 2: ADR D9 に確認の結果を書いた（コードは変えない）。01M756VNCZ の後は、切り替えで止まった run の件は回収で pending に戻り、新しい daemon が次の tick で拾う（既存の試験 `cos_fallback_timing_standby_to_active_picks_on_the_next_tick`）。本番の `-735` は旧い 120 秒の規則で送信済みの退避なので、同 ADR D2 のとおり人に残る。

## 変更（attempt 1 の Run #4 と attempt 2）
- attempt 1 の Run #4 は、`celeris::script_compare_cos_digest` の 6 件の失敗（host の `~/.local/celeris/releases` の未昇格の release を読み、受信箱に `release_unpromoted-selfdeploy` が増える）を、`crates/celeris/tests/support/script_compare.rs` の `config_text` に `[selfdeploy] releases_dir` を足して直した。
- 同じ直しが `main` の `phase 1: script_compare の試験の config が host の release 置き場を読まないようにする（release の gate の不合格）` に入っており、rebase の後は `config_text` に `[selfdeploy]` の表が 2 つになった。TOML の表の重複で `Config::load` が落ち、attempt 1 の review の test-affected は exit 100 になった。
- attempt 2 で `script_compare.rs` を `main` の版に戻した（この task の差分から外した）。`main` の版の `config_text` が `releases_dir`・`repo`・`detach` を一時 dir に向け、`write_config` がそれを assert する。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0（件数は報告本文）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。`sh scripts/dev/progress-index.sh --check` → exit 0。

## 未解決
- 本番で送信済みの退避の件（例 `dependents_reopened-01M75722QN630BBASEQ759KG4T-735`）は人に残る。配備の後は、人が受信箱で閉じるか、依存先の後続が動き出すと消える。
- 照合が判定する人に残す件は、件の種類と本文から決まるもの（credential の待ち・cluster の login・ブラウザの待ち・`near_limit:` の計画）だけ。目的の変更・本番 config・有料の試行・研究上の判断・検査を弱める判断は、CoS が方針（常設の節と skill §0）で判定する。

## 本番への移行
1. 配備の後、`~/.config/celeris/config.toml` の `[cos.triage] policy_version` を `"12"` にする（今は "11"）。
2. KB の skill を v12 に差し替える: `celerisctl skills import config/skills --name cos-inbox-triage --root <kb_root>`。
3. `GET /api/v1/system/health`（または CoS の run の policy_version）で 12 になったことを確かめる。版が違うと resolve は 422 `cos_policy_version_mismatch`。
4. migration 0124 は daemon の起動で当たる（`cos_notification_routes` の作り直し。行は消えない）。

## 提案
