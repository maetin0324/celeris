---
title: 長時間自律化（Project Controller）A0〜A9 の成果まとめ
tasks: [01M6JW13XAQ3169H3TTB3H43ZY]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# 長時間自律化（Project Controller）A0〜A9 の成果まとめ

完了日: 2026-11-05（親の closeout。子 11 件はすべて done で main に入っている。実 LLM の pilot と 24〜72 時間の実稼働は人の gate として残る）

## 結論
- 子 A0〜A9（11 task）はすべて status done で、各子の commit は main に取り込まれている（下の表）。計画書だけで done にした子は無い。
  A0 と A8(設計) は設計の task で、コードの実証はそれぞれ A1〜A9・A8(実行) の raw にある。
- 案件を長期に運営する Project Controller を、既存の Project・root task・events・CoS chat run・control-ts の上に足した
  （[ADR](../adr/2026-11-04-project-controller.md)）。Task は done / failed で終わり、長期の継続は案件の controller の状態が持つ。
  判定は TS、状態遷移・検証・予算・安全は Rust。既定は無効（mode off）で、本番の release・promote・systemctl・config・DB は変えていない。
- **未達（人の gate）**: 実 LLM の pilot（token・quota・実 walltime・LLM の判断の質）は未計測。24〜72 時間の実稼働は未実施。
  2026-11-05 の人の判断 (b) で、A9 の受け入れ条件 5 は仮想時刻の raw（`a9/virtual`）と実 daemon + stub harness の raw（`a9/stub`、LLM 0）で
  受け入れ、実 LLM の pilot は後回しの明示の gate とした。

## 子の task・状態・統合 commit
統合 commit は main の件名で挙げる（hash は書かない）。

| 子 | task id | 状態 | 統合 commit（件名・最後の 1 件） | 進捗 |
|---|---|---|---|---|
| A0 契約・ADR | 01M6JW1EFF381XRN953BX8PG81 | done | `phase 0: Project Controller の ADR と A1〜A9 の契約を固定する`・`phase 0: 進捗に提出時の検査の結果を書く` | [a0](2026-11-04-long-horizon/a0.md) |
| A1 durable wake（Rust） | 01M6JW1EFJ55VST9P08ZTHGKVN | done | `a1 decide-notice-cursor: decide の失敗で止まるとき notice を同じ tx で記録し、案件別 cursor 以下の再配送を wake にしない` | [a1](2026-11-04-long-horizon/a1.md) |
| A2 TS core | 01M6JW1EFMF6T3CVCEH9ATR14D | done | `a2: runner の不変条件の関数型に別名を付ける（clippy）、進捗の件数を直す` | [a2](2026-11-04-long-horizon/a2.md) |
| A3 案件 CoS | 01M6JW1EFPBCJX8J7D2ZACHS3D | done | `a3: refuse external publication and irreversible ops, close the review-failed retry loop, dry_run ignores event wakes, root_done note (attempt 3)` | [a3](2026-11-04-long-horizon/a3.md) |
| A4 統括 CoS の相談 | 01M6JW1EFRVZZNDQ8NYF3G588W | done | `a4: release gate の script_generation_e2e の in_flight 競合を直す（前の段の pass が A に残る間は待つ）` | [a4](2026-11-04-long-horizon/a4.md) |
| A5 Roadmap | 01M6JW1EFT0NX6HP89S55ECNXE | done | `a5: progress a5 with check results` | [a5](2026-11-04-long-horizon/a5.md) |
| A7 authority ledger | 01M6JW1EFW86JH3VKZ274J9GT5 | done | `a7: progress a7 records the script_generation_e2e flake outside the WU paths` | [a7](2026-11-04-long-horizon/a7.md) |
| A6 長時間の運用 | 01M6JW1EFYFTSPMT080KHKW9EY | done | `a6: progress a6 records the check results` | [a6](2026-11-04-long-horizon/a6.md) |
| A8(設計) | 01M6JW1EG0Y7H5REMSDQESKBWA | done | `phase 0: long-horizon ベンチの課題・oracle・隔離の設計を固定する` | [a8a](2026-11-04-long-horizon/a8a.md) |
| A8(実行) | 01M6JW1EG2G677XHN408JCW2JG | done | `a8b: progress a8b records the check results` | [a8b](2026-11-04-long-horizon/a8b.md) |
| A9 統合・画面・pilot | 01M6JW1EG41D198Y5CP65MQJM4 | done | `a9: progress a9, parent summary, ADR appendix A9, SPEC/architecture-map/ops docs` | [a9](2026-11-04-long-horizon/a9.md) |

## 依頼の受け入れと raw（すべて LLM 0、非本番の一時 DB）
| 依頼の受け入れ | raw | 値 |
|---|---|---|
| 同一 event を 10 回再配送して次 Task 1 つ | `a1/redelivery.jsonl` | deliveries 10・roots 1・wakes 1・decides 1・idempotency_keys 1・unprocessed_wakes 0・llm_launches 0 |
| crash の各境界から回復し未処理なし | `a1/crash.jsonl`（8 行） | CreateRoot の B1〜B6 と QueueCosInput の B4・B5。各行 roots ≤ 1・未処理 0 |
| 待機中の LLM の無駄起動 0 | `a1/idle.jsonl` | 仮想 168 時間で wake なし decides 0、cron のみ decides 168。どちらも llm_launches 0・roots 0 |
| root1 → root2 → 最終目標で不要な起票 0、同時 event で重複 0 | `a2/e2e/project_controller_core_s2_s3_root1_to_root2_to_achieved.jsonl`・`…_s4_concurrent_events_create_one_root.jsonl` | 達成後の wait 5 回で roots_total 2 のまま。同時 8 評価で root 1 |
| 相談 task 0・読取完了・修正 task 1 | A4 の script が再生成（repo には置かない。a4.md の「fixture・raw records の場所」） | 親の closeout で再実行: consult 0・read 0・delegate 1、reads 1、要約は元の thread、problems [] |
| TS だけの変更で cargo 0 回 | `a2/ts-verify-warm.jsonl` | 20 試行すべて cargo_invocations 0・link_count 0。warm p50 8.715 秒（基準約 7.5 秒より +1.2 秒） |
| 旧 CoS と新 Manager の比較（再生） | `a8b/replay` | new 10/10・old.passive 3/10・old.proactive 8/10・ctl.null 2/10、avoidable 0、invalid_init 0 |
| 長時間・画面 | `a9/virtual`（72 時間の仮想時刻 7 シナリオ・API の門 7 行）・`a9/stub`（実 daemon + stub harness）・`a9/screens` | 本番の門（promote・secret・mode 上げ）は CoS に 422/403 |

本番の変更・人の gate の迂回は無い。各子の進捗の「本番への移行」は人の手順として書かれ、実行していない。

## 止まった gate と次に要る人の判断
1. **実 LLM の pilot**（A9、人の判断 (b) で後回し）: task 完了後に外部 operator が worker の外で
   `scripts/dev/long-horizon-pilot-llm.sh --harness config` を回す（C06・C08 の new 腕の 2 試行、上限は計 200 万 token・8 時間）。
   raw は別の task で `a9/llm/` に取り込む。人がすること: 測定専用 account の login と実行の承認。
2. **24〜72 時間の実稼働**: 人の承認が無く未実施。実 LLM pilot の結果を見てから決める。
3. **本番への導入**: `docs/ops/project-controller.md` の手順（generation・script_domain_modes の ts、案件 mode を shadow から）を人が実行するか決める。

## 未達・未解決（子から親に残るもの）
1. 実 LLM の試行の sandbox（`ops-r2-sandbox.sh` の中の daemon、probe-read）と DB→facts の collector は未実装（A8(実行)・A9）。
2. brief の更新は controller の wake にならない。escalate・予算超過・TTL の blocked は人が mode を置き直すまで戻らない（A6・A8）。
3. GET controller の budget/guards と GET /metrics/project-controller、report の route、projects_section の controller 状態、
   受信箱の種類 project_controller（A3・A6）。
4. controller の mark_milestone を Roadmap の Outcome の状態の記録に結ぶ効果は未実装（A5）。snapshot の authority.grants に取り下げた Outcome の許可が載る（A7 の未解決 8）。
5. `celeris::script_generation_e2e` は負荷時に in_flight の確認とすれ違うことがある。A4 が待ちを足したが、負荷時の再現は試していない（A4・A7）。
6. web e2e の board の試験は main でも落ちる（A9 の未解決、全体の e2e は release gate だけで走る）。

## 証拠（親の closeout。提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-general-cos-consult-e2e.sh --out <artifacts>/gcc-raw` → exit 0（13 passed、raw 10 件、problems []）
- `test -s agent-docs/progress/2026-11-04-long-horizon.md && sh scripts/dev/progress-index.sh --check` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（この branch の差分は進捗 file だけ）

## 提案
- 実 LLM の pilot の raw を見てから、本番の 1 案件で mode `shadow` から始める（人の判断）。
- 未解決 2 の「brief 更新を wake にする」と「blocked の日替わり復帰」は、pilot の結果を見て別の task にする。
