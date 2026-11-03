---
title: gen-e2e — script_generation_e2e（新旧併存→切替→途中 error→rollback→再起動）
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
updated: 2026-11-03
completed: 2026-11-03
---
# gen-e2e — script_generation_e2e

## 結論
`crates/celeris/tests/script_generation_e2e.rs` を作った。本物の bundle と試験用の一時 DB の daemon（userns 不要、ADR-0126）で、下の流れを 1 つの試験で通す。40 回連続で合格し、host の遅さによる場面のやり直しは 0 回だった（1 回約 5 秒）。

## 流れ（領域 `cos_digest`）
1. **A の昇格**: A（本物の bundle）を人の token で登録し `compare`。計数不足の `canary` は人の token でも 422（`script_canary_not_ready`）。計数を積んだ後に人の token で `canary`（5000 bps）→ `ts`。
2. **未承認の変更 0**: `canary`・`ts` のそれぞれで次を確かめ、表の行と `script_domain_events` の件数が変わらないことを確かめる。
   - 外部 operator の token: 403 `script_mode_requires_human`。
   - CoS の run credential: `/cos/operations` は 403 `script_mode_requires_human`、直接の PUT は 422 `cos_audit_context_required`。
   - 知らない token: 401。
   - `system:` の主体: token を持てない（`Config::load` が拒む）。store の `script_domain_set_canary`・`script_domain_set_mode` も拒む。
3. **切替と drain**: B（本物の bundle + error を注入する包み）を登録し、daemon の isolate A で pass を始めて op の答えを止める。人の token で `compare` の B へ切り替える。
   - A は `draining`、`in_flight` 1 のまま残る。新しい要求は `generation_gone`。切替の後の run の記録は B。
   - 答えを返すと pass は A で終わり、A は `retired`（actor `system:script-runtime`、`cancelled_in_flight` 0）。
4. **途中の error**: B を `ts` まで上げ、`run-poison-` の run で pass に例外を投げさせる。連続 3 件で script-guard が `rust` へ戻す（`guard_failure_streak`）。その間の digest は Rust が apply する。B は `retired`。
5. **再起動**: 戻しは再起動の後も `rust` のままで、受信箱に項目が 1 件出る（API `/inbox/items` でも確かめる）。人が A を登録し直して `ts` に戻すと項目は閉じる。再起動すると同じ行（mode・generation・version）を新しい isolate で復元し、mode の記録も digest も増えない。
6. **bundle の削除**: bundle を消して再起動すると `rust`（actor `system:script-startup`、`startup_bundle_unusable`）に戻り、受信箱に項目が 1 件出る。Rust が digest を続ける。

## 合格条件の検査（`assert_ledger`）
- **欠落 0**: `script_domain_events` が予定の 11 件と完全に一致し、version が 1 ずつ増える。`script_generation_events` の状態遷移が A（2 周）・B（1 周）で完全に一致する。generation の要る mode の run は `script_compare_log` が 1 件、`rust` の run は 0 件。`events` は lane_rules の task の `script_compare_recorded` と記録の件数が一致する（1 件以上）。
- **二重副作用 0**: 試験が入れた run ごとに digest message が 1 件、どの key も 2 件にならない。`script_compare_log` も同じ run で 2 件にならない。
- **TS の apply**: A・B とも TS の command が apply された run がある。TS の apply は `canary`・`ts` の mode だけで、安全違反も fallback も無い。
- **全体の停止は起きていない**: `script_guard_halted` が `None`。

## daemon の変更
`crates/celeris/src/daemon/run.rs` に試験の入口 `run_with_test_hooks` と `DaemonTestHooks` を足した。本番の入口 `run` は既定（どちらも無し）で動く。
- `generation_registry`: 登録簿を試験と共有する。dispatcher の pass は deadline（200 ms）の内に終わるので、切替をまたぐ in-flight を出来事待ちで作るにはここから口を取る。drain の時計も `ManualClock` で止める（30 秒の打ち切りを起こさない）。
- `no_orphan_takeover`: 試験が store に直接書いた動いている run を、tick が持ち主の居ない run として閉じないようにする。

`script_layer.rs` の restore は直していない（流れは変更なしで通った）。

## 試験の組み方（後の unit への注意）
- **digest の起こし方**: digest pass は「この process の CoS の run が終わった」か「起動した」ときにだけ走る。store に run を書くだけでは走らない。試験は偽の CoS（`[adapters.fake]`）の短い run を project thread に起こす。
- **lane_rules の段は `[cos] enabled = false`**: task の問いが triage の run にならないようにする。
- **昇格の計数は store に積む**: `script_guard_observe` で (f) の 200 件を積む。実の pass を数百回は回さない。数値は変えていない。
- **triage の競合**: script の受信箱の項目から daemon 自身が triage run を起こす。この run も digest されるので、全体の digest の数は決まらない。検査は試験の run ごとの件数と key の重複で行う。試験の item を daemon が先に claim したら、新しい item を入れてやり直す。
- **遅い host**: TS の待ち（1 tick 200 ms）の `timeout` が (f) の 10 分 3 件に当たったときだけ、新しい一時 DB で場面をやり直す（上限 5 回）。

## 証拠（提出時の HEAD で実行、基準は `$CELERIS_WU_BASE`）
EVIDENCE_PLACEHOLDER

## commit
- phase s5: gen-e2e: script_generation_e2e（新旧併存→切替→途中 error→rollback→再起動）と試験の入口

## 未解決
1. `events` の lane_rules の記録は「1 件以上かつ event と記録の件数が一致」までで、1 件ちょうどは確かめていない。task の再試行で dispatch が増えうるため。
2. in-flight は試験が daemon の登録簿の口から送った pass で作る。dispatcher 自身の pass が切替をまたぐ場面は、deadline が 200 ms なので決定的に作れず、この試験に無い。
3. `canary` の Rust 側・TS 側の選択は pass id（ULID）で決まるので、この試験ではどちらの側を通ったかを固定していない。選択の決定性は canary-dispatch の `canary_selection_is_deterministic_per_unit` が確かめる。

## 提案
- 受信箱 `script_control` の項目が CoS の triage にも流れ、daemon が triage run を起こすことをこの試験で確認した。s5-close で、人向けの知らせと CoS の triage の二重の扱いがよいかを付記 S5-7 と突き合わせる。
