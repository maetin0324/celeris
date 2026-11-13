---
title: Discord の通知を人か operator の対応が要ると決まった件だけに絞る
tasks: [01M791DFQGCDG46CCR5KWNDDQA]
status: done
updated: 2026-11-13
---
# Discord の通知を人か operator の対応が要ると決まった件だけに絞る

完了日: 2026-11-13

設計: [ADR 2026-11-12 discord-person-or-operator](../adr/2026-11-12-discord-person-or-operator.md)。
直近 7 日の分類と原因の報告は task の artifacts の `discord-7d-report.md`（台本 `analysis/discord_7d.py`、件ごとの分類 `analysis/discord_7d.json`）。

## 変更
- `phase 1: Discord を人か operator の対応が要る件だけに絞る（古い revision を閉じる、推奨が人の選択肢でなければ CoS が答える、policy 13）`
  - task-core: 新しい revision の判定 1 つ、item の結果の記録で古い revision を閉じる、経路の取り合いで古い revision を断る、
    送る直前の取り下げで `fallback` の item も閉じる。`feed::notice_revision_open`（知らせが同じ revision で開いているか）。
  - task-dispatch: 退避は既読・束が育った知らせを閉じ、経路を取らない。
  - celeris: notifier の送る直前の再確認を上の 2 関数に置き換えた（旧い SQL と 500 件ずつの loop を消した）。`policy_version` の既定 "13"。
  - task-api: escalation packet の `reserved_options`、検査 `cos/inbox/reserved_options.rs`（422 `cos_answer_unreserved_recommendation`）、
    `triage_view::superseded` は store の関数を呼ぶ。schema と web の生成型を再生成。
  - task-worker: CoS の常設の文脈の方針の要点を policy 13 にした（793 字、上限 800）。
  - skill cos-inbox-triage v13（§0・§3・§6・§7）と SOURCE.md、`config/celeris.example.toml`・`docs/ops/cos-chat.md`・
    `docs/ops/external-operator.md` §4・`docs/ops/mlsys-main-4-ops-r1.md` の版の表・`docs/architecture-map.md`。
- `phase 1: 古い revision の判定と閉じる SQL を task_core::cos_revision に移し、health の module と関数の行の悪化を直す`
  - 新しい module `task_core::cos_revision`（`superseded`・`close_if_superseded`・`after_judgment`・`CLOSED_BY_NEWER_REVISION`。
    `cos_fallback` と同じ形）。chat の triage store は自分の transaction の中でこれを呼ぶ。`notify/triage.rs`・`triage_view.rs` は
    `cos_revision::superseded` を呼ぶ。ADR D1 と `docs/architecture-map.md` の参照を直した。
  - health の 3 件（module `task-core/src/chat` の本体の行 8040 > 8000 が new、module `task-dispatch/src/dispatcher` 34968 と
    関数 `cos::resolve` 243 行が worsened）を直した。chat は 7998、dispatcher は 34958（`fallback.rs` は `use` で型の path を短くした）、
    `resolve` は `reserved_options::check` を `validate_packet` に入れて基準の行数に戻した。`resolve_stale` の行の読み出しは
    rusqlite の tuple の `TryFrom<&Row>` にした（意味は同じ）。
- `phase 1: 試験の CoS skill の版を config の既定から取る（ops_r1_measure・script_generation_e2e が版 12 を固定していた）`
  - `ops_r1_measure_cos_path_with_stub_harness` は skill の front matter の今の版を読んで 6 に書き換える。
    `script_generation_e2e_switch_error_rollback_restart` は読み込んだ config の `[cos.triage]` の版で skill の fixture を書く
    （`tests/support/script_compare.rs` と同じ形）。どちらも版を 13 に上げた後に落ちていた（前者は版の不一致で script が拒否、
    後者は skill の不一致で CoS が起動せず 300 秒で失敗）。直した後は単独でも test-affected でも通る。

## 試験（足した・直したもの）
- 足した（直す前に落ち、直した後に通る。直す前は src だけを基準の版に戻して実行し 7 件とも FAIL）:
  - task-core `cos_triage_judging_a_newer_revision_closes_the_older_ones`（直す前: 古い rev が `running` のまま）、
    `cos_triage_outbox_claim_refuses_an_older_revision`（直す前: 古い rev に退避の経路が取れた）、
    `cos_triage_withdrawn_fallback_closes_its_item`（直す前: `fallback` のまま）。
  - task-dispatch `cos_chat_triage_fallback_older_notice_revision_is_closed_not_handed_over`（事例 2 の形。直す前: 退避の行 1）、
    `cos_chat_triage_fallback_read_notice_is_closed_not_handed_over`（直す前: 退避の行 1）。
  - task-api `answer_first_case1_in_frame_recommendation_is_answered_and_not_sent`（事例 1 の形。直す前: 400 unknown field）、
    `answer_first_reserved_matters_still_reach_the_person`（枠を超える推奨・研究上の判断の推奨・`human_required` は人に送る。直す前: 400）。
- 直した既存の試験: 人宛ての escalate の packet 7 か所に `reserved_options` を足した（契約の変更。D2）。skill の fixture の版を
  "12" から "13" に（`credential_wait_notify`・`root_done_summary`・`classify_tick/e2e`。config の既定と合わないと skill の不一致の
  知らせが出て CoS が止まる）。`cos_chat_triage_fallback_notice_title_and_body_stay_together` は元のまま通る（rev 1 は束が育つ前に退避する）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh`: exit 0（約 13 分、開始 load 18）。check-health は verdict=pass（fail 級 0、new 1 は
  optional の dup.state の warning: 表 `cos_inbox_items` を `task-core/src`（`cos_revision`）と `task-core/src/chat` が書く。
  `notifications` を `cos_fallback` と chat が書く既存の finding と同じ形で、書く関数は chat の triage store の transaction の中だけで
  呼ばれる）。web は tsc -b と unit（103 file、713 件）合格、ts-verify 20 passed、nextest 8378 件 passed（23 skipped、14 crate）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 1 回目の test-affected（版 12 を固定した試験を直す前）は nextest 8376 passed・2 failed（上の 2 件）で exit 100。

## 本番への移行
1. この task の release を昇格した後、運用者が `~/.config/celeris/config.toml` の `[cos.triage]` の `policy_version = "12"` を
   `policy_version = "13"` に書き換え、daemon を再起動する。行が無ければ既定の "13" になる。
2. 確認: `celerisctl` か `GET /api/v1/health` の `cos_policy_version` が "13"、KB の `skills/cos-inbox-triage/SKILL.md` の
   `metadata.version` が "13"（昇格で skills/ が同期される。ADR 2026-11-11-cos-skill-sync）。config だけを先に上げると CoS の
   claim の門が CoS を止める。
3. 既存の `fallback` のまま残った古い revision の item（事例 2 の rev 2 など）を、この変更は遡って閉じない。CoS が同じ元の待ちの
   新しい revision を判定したときに閉じる（D1）。手で閉じる作業は要らない。

## 未解決
- D2 の判定は CoS が書く `reserved_options` に依る。CoS が推奨を人の選択肢に入れれば送られる。実際に減ったかは、版 13 の配備後に
  `artifacts/analysis/discord_7d.py` で 7 日を数え直して確かめる（見積もり: 人以外が答えた件への送信 73 → 35 以下）。
- 再確認と POST の間の秒単位の競合（7 日間で 1 件）は残る。

## 提案
- 知らせ（notice）の退避を Discord に送るかを決める。7 日間の退避 25 件のうち 18 件は知らせで、どれも後で新しい revision に置き換わった
  （旧い 120 秒の規則による送信。task 01M756VNCZ の後の数は未確認）。知らせは人の判断の待ちではないので、退避でも Discord に送らず
  web の通知に残す案がある。
