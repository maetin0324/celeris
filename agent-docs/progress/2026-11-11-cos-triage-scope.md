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
- (f) `cos_triage_scope_f_outside_celeris_goes_to_the_operator_without_discord`: operator 宛ては notifications 0 件、経路 `operator`、札なし、addressee=operator。`cos_triage_scope_f_human_required_waits_are_not_routed_to_the_operator`: 人が設定した human_required・human check の件は operator 宛てが 422（attempt 3）。
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

## 変更（attempt 3）
- attempt 2 の review の不合格（criterion 6）: 人が設定した `human_required`（label）・human check・override の待ちを、CoS が `to=operator` で回せた（人に Discord も札も届かない）。operator 宛ての経路（`cos::inbox::escalate_to_operator`）が照合の分類（`reserved`）しか見ていなかった。
- `phase 1: 人が human_required にした件を operator 宛てに回せないようにする（review の差し戻し）`
  - `cos::inbox::kept_by_person` を answer と operator 宛ての両方の判定の正本にした（override の待ち・`human_required_reason`・照合の `reserved`）。結果は `Kept`（`HumanRequired`・`Reserved`）で、answer は従来どおり 403 `cos_human_required`／422 `cos_reserved_for_human`、operator 宛ては 422 `cos_reserved_for_human`（`basis::kept_for_human`）で、経路を取らない。
  - 試験 `cos_triage_scope_f_human_required_waits_are_not_routed_to_the_operator`: label `human_required` の決定と human check（受け入れ確認）を operator 宛てにすると 422、operator の経路 0 件。human 宛てなら 200 で outbox 2 通。
  - review の advisory も直した: ADR D2 に human_required の operator 宛ての拒否、D3 の監査の reason を実装の形（「CoS の判断（<category>）」、版は `policy_version`、grant は `basis.refs`）、D4・D1 の `browser_wait` を `security_boundary`、D7 に `review_rework` の回数が旧い文言の監査も数える理由と撤去の条件。skill の「(a)〜(g) の条件」の言い方と `cos_plan_conforms.rs` の試験のコメントの `documented_default`。`docs/api/v1/api.md` の operator 宛ての拒否の説明。

## 変更（attempt 4）
- attempt 3 の review の不合格（criterion 6）: operator 宛ての件が、人が読む受信箱 thread（要約の判断「人に回した」、card の `escalated`「人の判断待ち」、人待ちの数）と評価の台帳（`human_waits`・`open_human_waits`）で人の待ちとして出ていた。mlsys の方針の版の記録に 11→12 の行が無く、skill §2 に v11 の 2 文（replan・withdraw は escalate、計画の decision は人に残る）が残っていた。
- 実装前の整理: 根本原因は operator 宛ての区別が `cos_notification_routes.route` にしか無く、表示と台帳が route を読んでいなかったこと。owner は `digest.rs`（と byte 一致の `control-ts/src/domains/cos_digest.ts`）、`web/features/chat/cards/card-model.ts`、`store/request_ledger.rs`。新しい item の state は足さず（状態の正本は route 1 つ）、card の state `operator` と台帳の `operator_waits` を route から導く。fallback・切替 flag は無し。撤去は skill §2 の v11 の 2 文、TS の `route` の `operator` を throw する制約、移行手順の手での skill import。
- `phase 1: operator 宛ての件を人の待ちとして見せない・数えない（review の差し戻し）`
  - digest（Rust と TS）: 経路 `operator` の `escalated` は判断「外部 operator に回した（人の判断は不要。Discord 通知なし）」、card の state `operator`（`OPERATOR_CARD_STATE`）。他の state は経路があっても変えない。fixture の `hand/decision/*` に経路の次元（`escalation`・`operator`）を足し（48 行増） `cargo run -p task-core --example dump-cos-digest-fixtures` で作り直した（生成部は不変）。TS の `ROUTES` に `operator` を足した（以前は throw し、compare が崩れた）。
  - web: `card-model.ts` に `operator`（「外部 operator の対応待ち」、info）。`pendingHumanCount` の `WAITING_STATES` には入れない。試験 `chat_cards_operator_route_is_not_a_human_wait`。
  - 台帳: `RequestRow.operator_waits`・`summary.open_operator_waits`、`HumanWaitSpan` を `RouteWaitSpan` に改名、schema `celeris.eval-requests/2`。試験 `operator_route_is_not_a_human_wait`。
  - 文書: ADR D2 の付記（表示・台帳・拾い方）、ADR 2026-10-07 と ADR 2026-10-27-eval-request-ledger の付記、`docs/ops/mlsys-main-4-ops-r1.md` §8 の 11→12 の行、skill §2 の 2 文を D6（決定に先に答えて判定し直す）と `replan` は `task_scope`・`withdraw` は escalate に書き換え、`docs/ops/external-operator.md` §1 に operator 宛ての件の拾い方、`docs/ops/cos-chat.md` の Discord の確認手順、下の「本番への移行」を昇格の skill の写しと門に合わせた。

## 証拠（提出時の HEAD で実行。attempt 4）
- `bash scripts/dev/test-affected.sh`（開始 load 46、1428 秒）→ exit 100。nextest 8328 件中 8327 件合格、web 712 件・control-ts 134 件合格、script bundle SKIP 0 件。落ちた 1 件は範囲外の task-worker `ssh::tests::run_command_times_out_and_kills_the_ssh_process_group`（偽 ssh の子 process が残ったと判定）。この差分は task-worker に触れておらず、同じ HEAD で単独に 3 回実行して 3 回とも合格（各 2.0 秒）。負荷の高い時の flaky として未解決に書く。
- 同じ HEAD（進捗の証拠の commit の後）で `bash scripts/dev/test-affected.sh` を再実行（開始 load 42、939 秒）→ exit 0。nextest 8328 件すべて合格、web 712 件・control-ts 134 件合格、script bundle SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。`sh scripts/dev/progress-index.sh --check` → exit 0。
- `bash scripts/dev/check-health.sh` → verdict pass、exit 0（size.responsibility-growth は chat module 8000 行・store module と `request_row` は base と同じ行数に収めた）。
- `cargo run -p task-core --example dump-cos-digest-fixtures` で fixture を作り直し（生成部は不変、`hand/decision/*` が 48 行増）、control-ts の `cos_digest.test.ts` 53 件と web の `chat-card.test.tsx` 24 件が合格。

## 未解決
- task-worker の `ssh::tests::run_command_times_out_and_kills_the_ssh_process_group` は load 46〜60 の test-affected で 1 回落ちた（単独では合格）。同じ系統の `remote_command_times_out_and_kills_the_ssh_process_group` も高負荷で落ちることがある。この task の範囲外で、直すなら task-worker の ssh の試験を対象にした別 task にする。
- 本番で送信済みの退避の件（例 `dependents_reopened-01M75722QN630BBASEQ759KG4T-735`）は人に残る。配備の後は、人が受信箱で閉じるか、依存先の後続が動き出すと消える。
- 照合が判定する人に残す件は、件の種類と本文から決まるもの（credential の待ち・cluster の login・ブラウザの待ち・`near_limit:` の計画）だけ。目的の変更・本番 config・有料の試行・研究上の判断・検査を弱める判断は、CoS が方針（常設の節と skill §0）で判定する。

## 本番への移行
1. 昇格（`promote.sh`）が release の `skills/cos-inbox-triage`（v12）を KB の `skills/` に写し、KB の git に commit する（ADR 2026-11-11-cos-skill-sync D2。`docs/ops/selfdeploy.md` の「CoS の skill の写し」）。写しが exit 4 で失敗したときだけ、人が `bash ~/.local/celeris/releases/<sha12>/scripts/sync-skills.sh ~/.local/celeris/releases/<sha12> <knowledge root>` を 1 回実行する。
2. 昇格の直後から、KB の skill の版（12）と `~/.config/celeris/config.toml` の `[cos.triage] policy_version`（今は "11"）が合わないので、daemon は CoS の run を起こさず受信箱に 1 件知らせる。人が `policy_version` を `"12"` にして config を読み直す（`POST /api/v1/reload`）と一次対応が再開する。
3. `GET /api/v1/system/health`（または CoS の run の policy_version）で 12 になったことを確かめる。版が違うと resolve は 422 `cos_policy_version_mismatch`。
4. migration 0124 は daemon の起動で当たる（`cos_notification_routes` の作り直し。行は消えない）。
5. operator 宛ての件は Discord に出ない。operator は `GET /api/v1/cos/inbox?state=escalated` の `addressee=operator` の件を拾う（`docs/ops/external-operator.md` §1）。
6. `celerisctl eval requests` の出力は schema `celeris.eval-requests/2`（`operator_waits`・`open_operator_waits` が増えた）。版 "12" 以降の人の待ちの数は版 "11" 以前と比べない（`docs/ops/mlsys-main-4-ops-r1.md` §8）。

## 提案
