---
title: P0-A 事実と契約の再点検・設計（第 3 フェーズの起点）
tasks: [01M6N7XY9E70AH4476VQTSFVJY]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# P0-A 事実と契約の再点検・設計（第 3 フェーズの起点）

完了日: 2026-11-05。親は 01M6N7WD1WXXW0A5FV7RVZ67B7。基準は `git merge-base HEAD main`（起票時の main と同じ。main は更新されていなかった）。

## 結論
- 「達成」を主張する 6 層（controller の phase・Roadmap の Outcome・成果の根拠・oracle・Reviewer・check）は、互いを確かめずに別々の根拠で真になる。A9 の修正（D1〜D6）は各層の誤りを直したが、層の間の確認は無いままである。
- 新しく見つけた欠陥は 2 つ。(1) Rust の `validate` は phase `achieved` を、**TS が申告した `gap.open` が空か**だけで通す。TS を書き換えれば根拠の無い achieved を書ける（共通受入 (b) に反する）。(2) `mark_milestone` は Rust で根拠を検査するのに、効果が無く Roadmap の状態を変えない。
- 再発防止の不変条件 I1〜I12 と試験のケース K1〜K24（K1b を含む）を ADR `agent-docs/adr/2026-11-05-phase3-achievement-invariants.md` に固定した。
- 兄弟 task は新しく起票していない（9 件とも CoS が起票済み。下の分担表）。受入の補足は下の「兄弟の受入の補足」に書いた。
- `docs/ops/project-controller.md` の古い 2 行（`not_git`・`no_commits` を取り込み済みに数える）を A9 D1 の現状に直した。運用手順の全面の見直しは P2-J。

## 照合した資料
`agent-docs/progress/2026-11-04-long-horizon.md`・`a1.md`〜`a9.md`、`2026-11-05-a9-llm-pilot.md`（と同名 dir の WU 5 件）、`2026-11-05-a9-fix-arrival.md`（と WU 3 件）、ADR `2026-11-05-a9-llm-pilot-findings.md`、`docs/ops/project-controller.md`。コードは main の `control-ts/src/domains/project_controller.ts`、`crates/task-core/src/project_proposal.rs`、`crates/task-ops/src/{project_snapshot.rs,roadmap.rs}`、`crates/task-dispatch/src/dispatcher/{project_decider_ts.rs,project_guard.rs,project_wake.rs,review_verdict.rs}`、`crates/task-dispatch/src/review.rs`、`crates/campaign/src/long_horizon.rs`、`scripts/dev/long-horizon-pilot/*`。grep の生記録は `p0a/facts.txt`。

## 齟齬の表（6 層）
| 層 | main で真になる条件 | A9 実 LLM（C06・C08）で起きたこと | main で修正済み（再実装しない） | 未達 | 担当 |
|---|---|---|---|---|---|
| controller の phase `achieved` | TS の `wait{achieved}` が `validate` を通る。`validate` は TS の `gap.open`・`gap.unknown` が空かだけを見る（`project_proposal.rs`）。Rust は Outcome ごとの根拠を数え直さない | 2 試行とも achieved | — | Rust の数え直し（I1）、TS 改変で迂回不可（K1b） | P0-B |
| Roadmap の Outcome `reached` | 人の API（`check_status`→`record_status_tx`）だけが書く。`mark_milestone` は `validate` で `root_satisfies` を見るが効果なし（`next_wake_after_secs = 1`） | Roadmap は使っていない（依頼文の block） | 人の API の検査（A5） | mark_milestone を `record_status_tx` に結ぶ（I2）、二重の書き込み 0（I11） | P0-B |
| 成果の根拠（root） | done・`reviewer_passed`・`integrated`・Outcome を名指すか acceptance を全部含む。`reviewer_passed` は reopen で戻らない | root はすべて done。成果は oracle と不一致 | repo 付き案件は `IntegrationMerged` だけ（root-row）、pilot の git 化（pilot-git）、merge 成功時の `IntegrationMerged`（fix-repo-422） | reopen 前の合格の持ち越し（I3(b)、未確認）、repo の無い案件の根拠の規則（I4） | P0-B |
| oracle | `out/<id>.txt` の sha256 = `hidden/expected.json`（`bench_version` 1） | 到達率 0/2 | C06・C08 の公開手順 `python3 minichunk.py verify`（fix-oracle）、公開契約の試験 3 件 | 検査 script 化・複数の正解表現・`bench_version` 2・C04/C09/C10・leakage の否定（I5・I6） | P0-C |
| Reviewer | LLM の verdict。stub は全基準を無条件 pass（`stub_harness.py`）。pilot の acceptance は Reviewer だけ | C06 の root 2 本が reviewer 合格で done、成果は不一致 | 食い違い event（review-check） | stub の無条件 pass の廃止（I8）、理由を facts・ops に写す（I9） | P0-E |
| check（決定的な検査） | `Check::Command` は reviewer より先に走り、落ちれば done にならない（`review.rs`）。pilot の root は command check を持たない | 検査が無く、落ちる余地が無かった | 優先の規則の試験 `review_check_wins` 2 件 | pilot の root に公開契約の command check（I8）、出典・exit を記録（I9） | P0-E（script の中身は P0-C） |
| （関連）外部効果 | facts の `effects` は `pilot_effect.py` の記録から読む。実 LLM の worker には渡らず、承認は driver の擬似 operator | C06 の effect 0 件で fail | stub harness での配線（fix-effects） | 実 LLM worker への配線、daemon の承認 API、worker が書けない記録（I10） | P0-D |
| （関連）自律復帰 | brief の更新は wake にならない。答えのある問いは `ANSWERED_RECHECK_SECS`=3600 秒ごとに再評価。予算超過・TTL は blocked で人が mode を置き直すまで止まる | — | — | brief の wake、毎時の再評価の削減、予算の翌日の扱い | P1-G |

## 兄弟 task と分担（新規起票なし）
| key | id | 依存 | 主な範囲 |
|---|---|---|---|
| P0-B | 01M6N7XY9H31GK4K0G0J1C6AQY | P0-A | I1〜I4・I11、K1〜K12 |
| P0-C | 01M6N7XY9KX62X9TX696TEHXNF | P0-A | I5・I6・I12、K13〜K16・K23 |
| P0-D | 01M6N7XY9N8J21RRGC8AP6EZ4A | P0-A | I10、K17〜K20 |
| P0-E | 01M6N7XY9R6FREMRKFFVS4ENMG | P0-A | I7〜I9、K3・K21・K22 |
| P0-F | 01M6N7XY9T4B60EWWRYVYE2EVH | B・C・D・E | 人の許可の後の実 LLM 再試験（I12 の実測） |
| P1-G | 01M6N7XY9W412TA8G2WV0726Z0 | P0-A | wake・予算・human wait、I11 の wake 側 |
| P1-H | 01M6N7XY9YSB8262BS0BYHZ3Z1 | P0-A | 統括 CoS の実 LLM 評価（この ADR の範囲外） |
| P1-I | 01M6N7XYA1GC9WVPFDSQE39D6V | B・C・D・E・G | 長時間運用と観測 |
| P2-J | 01M6N7XYA31CMJV6RR1KXSQMKH | B〜I | GET・UI・運用手順・rollout |

## 変更範囲と migration の競合
main の `SCHEMA_VERSION` は 114（`0114_authority_ledger.sql`）。

### file の持ち主（並列設計で衝突させないための分け方）
| file | 持ち主 | 他が触るときの規則 |
|---|---|---|
| `crates/task-core/src/project_proposal.rs`（`validate`） | P0-B | P1-G は触らない |
| `crates/task-dispatch/src/dispatcher/project_decider_ts.rs` | P0-B: `Wait`・`MarkMilestone` の分岐と効果 / P1-G: `ANSWERED_RECHECK_SECS` と wake 関連 | 同じ関数を両方が変えない。P1-G は定数と wake の関数だけ |
| `crates/task-ops/src/{project_snapshot.rs,roadmap.rs}`、`crates/task-core/src/store/roadmap.rs` | P0-B | — |
| `crates/task-dispatch/src/dispatcher/{project_guard.rs,project_wake.rs}`、`crates/task-core/src/store/project_wake*.rs`、`crates/task-ops/src/project_budget.rs` | P1-G | P0-B は読むだけ |
| `control-ts/src/domains/project_controller.ts` | P0-B: `satisfies`・achieved の節 / P1-G: `POLICY.humanWaitSecs`・brief の節 | TS だけの変更は verify で cargo 0、p50≤30 秒・p95≤60 秒 |
| `crates/task-dispatch/src/{review.rs,dispatcher/review_verdict.rs}` | P0-E | — |
| `tests/fixtures/long-horizon/`（public・oracle・`MANIFEST.sha256`）、`crates/campaign/src/long_horizon.rs`（採点） | P0-C | P0-D は facts の `effects` を読む既存の関数を変えない。`observation_failed` が要るなら P0-C の採点の入口に合わせ、統合時に後の側が直す |
| `scripts/dev/long-horizon-pilot/pilot_effect.py`、driver の効果と承認の関数、`scripts/dev/long-horizon-pilot-llm.sh` | P0-D | — |
| driver の root の acceptance を組む関数、`stub_harness.py` の review の分岐 | P0-E | 検査 script の中身は P0-C が `public/<case>/` に置く。P0-E の試験は合成の task で書き、P0-C に依存しない |
| `stub_harness.py` の成果を書く分岐 | P0-C | — |
| `docs/ops/project-controller.md` | P2-J | 各 task は自分の事実を進捗に書き、P2-J がまとめる（P0-A は古い 2 行だけ直した） |
| ADR | 各 task が自分の付記 file（`2026-11-05-phase3-<key>.md`）を足す | この ADR の末尾に追記しない（同じ file の末尾の衝突を避ける） |

### migration
- 要りそうな task: P0-B（Outcome の状態の重複を防ぐ一意の索引や版の欄が要るなら）、P1-G（wake の期限・予算の再評価の欄が要るなら）。P0-C・P0-D・P0-E は migration を足さない（effect の記録は file、reviewer の理由は既存 event）。
- 番号の予約: P0-B は 0115、P1-G は 0116。後から main に入る側が、先の側の番号を見て振り直す。
- 振り直しで直す所（過去の記憶から）: `SCHEMA_VERSION` の `assert_eq!` 8 か所（`grep '\b114\b'` で探す）、`crates/campaign/src/ops_r1_trace.rs` の `SUPPORTED_SCHEMA_VERSIONS`、`delivery.rs` の `migration_0042_fills_gaps_*` の版の一覧、`store/tests.rs` の `open_rejects_db_with_schema_version_newer_than_supported`（`SCHEMA_VERSION+1`）、`roadmap_tests.rs` のような「前の版の DB」を作る試験。
- 研究 K2/K4 の測定 binary が読む schema の版（`ops_r1_trace.rs`）は版を足すだけで、凍結評価の値は変えない。

## 受入条件とテストの対応表
### P0-A（この task）
| 受入 | 対応 |
|---|---|
| 0 齟齬の表と対応表 | この file の「齟齬の表」と本表 |
| 1 不変条件とケースを ADR に固定 | `agent-docs/adr/2026-11-05-phase3-achievement-invariants.md` の I1〜I12・K1〜K24 |
| 2 変更範囲・migration・分担、重複起票なし | 「兄弟 task と分担」「変更範囲と migration の競合」。`delegate.json`・`followups.json` は書いていない |
| 3 `test -f agent-docs/progress/2026-11-05-phase3/p0a.md` | 下の証拠 |

### 兄弟の受入 → ケース → 試験（既存は名前、新規は担当が名前を決める）
| task | 受入 | ケース | 既存の試験（保つ） | 新規 |
|---|---|---|---|---|
| P0-B | 未取り込み・root 無し・reviewer だけ pass・検査 fail・違う案件・一部だけ到達で achieved を拒否し、全部で達成 | K1・K1b・K2〜K6 | `root_row_*` 5 件（task-ops）、`project_controller_core_s5_not_integrated_continues`・`…_s5b_no_commits_root_is_not_achieved`（celeris） | P0-B |
| P0-B | 版の差し替え・重複 event・reopen・並行 approval/CAS・再起動で二重 0 | K7〜K11 | A1 の `project_wake_stale_epoch_*`・`project_wake_cursor` の型 | P0-B |
| P0-B | repo 付き root の integrated は `IntegrationMerged` だけ | K12 | `root_row_repo_*` 4 件 | — |
| P0-C | v2 で全 10 課題の再採点と陰性、複数の正解表現が合格、不十分は fail | K13・K14・K23 | `long_horizon_public_contract_*` 3 件（v1 の振る舞い。v2 で置き換える） | P0-C |
| P0-C | v1 の raw を改変しない、grader の隔離と leakage の否定 | K15・K16 | — | P0-C |
| P0-C | 合否が LLM の回答を読まず script で決まる | K13・K14 | — | P0-C |
| P0-D | 承認前 0・approved で 1 回・rejected で 0・crash と重複で二重 0 | K17・K18 | fix-effects の陰性 stub | P0-D |
| P0-D | worker が audit/effects を書けない | K19・K20 | — | P0-D |
| P0-D | stub 配線を保ち config harness で通る | — | `bash scripts/dev/test-long-horizon-pilot.sh` | P0-D |
| P0-E | check fail が reviewer pass に優先し root done・達成にならない | K3 | `failed_command_check_wins_over_reviewer_pass`・`reviewer_only_task_is_unchanged` | 達成まで届かない e2e は P0-E |
| P0-E | 理由・出典・exit を facts/ops/event に残す | K22 | — | P0-E |
| P0-E | stub reviewer が検証できない基準を pass しない | K21 | — | P0-E |
| P0-F | 人の許可の記録、v2 raw、achieved と oracle の一致 | I12 | — | 実測（人の許可が要る） |
| P1-G | 3 日の時計飛び・restart・brief/authority の同時編集・7 日以上の virtual clock | I11（wake 側） | `project_controller_replay_*`（A6）、A1 の crash 試験 | P1-G |
| P1-G | pause/cancel・human_required・危険な予算超過を自動解除しない | — | A6 の guard の試験 | P1-G |
| P1-G | human wait だけの毎時の再評価の削減を数値で | `ANSWERED_RECHECK_SECS`=3600 | — | P1-G |
| 全員 | 旧 milestones API は 410 | K24 | `milestone_lifecycle_endpoints_are_gone`・`patch_cannot_set_paused_or_cancelled_on_a_project_and_milestones_are_gone`（task-api lifecycle） | — |

P1-H・P1-I・P2-J の受入はこの ADR の不変条件と直接の対応が無い（P2-J は I1 の未達の理由を GET・UI に出すときに `pc/achieved-unverified` を使う）。

## 兄弟の受入の補足（親へのコメントとして出す文面）
- P0-B: 受入 1 に K1b（改変した TS が `gap` を偽っても Rust が achieved を拒否する）を含める。`validate` の `achieved_with_open_outcomes` を TS の `gap` でなく snapshot からの数え直しにする。
- P0-B: reopen の前の `reviewer_passed` の持ち越し（`root_row`）を K9 で確かめる。
- P0-C: v2 の採点は I12 のため、試行ごとに controller の achieved と oracle の合否の不一致件数を summary に出す。
- P0-E: 検査 script の中身は P0-C が持つ。P0-E は acceptance への結線と記録だけを持ち、試験は合成の task で書く。
- 全員: ADR はこの ADR の末尾に追記せず、`2026-11-05-phase3-<key>.md` を足す。migration は P0-B が 0115、P1-G が 0116 を予約し、後から入る側が振り直す。

## 変更（commit の件名）
- `p0a: 達成判定の 6 層の齟齬と不変条件 I1〜I12 を ADR と進捗に固定する`

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 既存の修正の試験の再実行（生記録 `p0a/existing-tests-summary.txt`、対象の一覧 `p0a/existing-tests-list.txt`）:
  - `cargo nextest run -p task-ops -p task-dispatch -p celeris-campaign -E 'test(/root_row_/) | test(/review_check_wins/) | test(/long_horizon_public_contract|public_contract/) | test(/^roadmap::/)' --no-fail-fast` → exit 0、10 passed
  - `cargo nextest run -p celeris --test project_controller_e2e -E 'test(/core_s5/)'` → exit 0、2 passed（control-ts を `pnpm install --offline --frozen-lockfile` した後）
  - `cargo nextest run -p task-api --test lifecycle -E 'test(/milestone_lifecycle_endpoints_are_gone|milestones_are_gone/)'` → exit 0、2 passed
- 検査 3 本と `test -f` の結果は報告本文（result.json の evidence）に書く。

## 失敗ケース
- この task で落ちた検査は無い。発見した欠陥（gap の申告を信じる `validate`、効果の無い `mark_milestone`）は既存の試験では検出されない（陰性の試験が無い）。試験は P0-B が K1b・K6 で足す。

## 未解決
- I3(b) の reopen の前の合格の持ち越しが実害になる経路があるかは未確認（P0-B）。
- repo の無い案件の根拠の規則（I4）の具体は未決（P0-B が決める）。
- 親へのコメントの投稿: worker の run から他の task へのコメントを書く手段は CoS の操作 API だけで、この run の credential では使えるか未確認。この file の「兄弟の受入の補足」を正本とし、この task のコメントにも同じ文面を残した。

## 次の gate
- P0-B・P0-C・P0-D・P0-E・P1-G・P1-H は P0-A の完了で並列に着手できる。P0-F は B〜E の完了後、人の明示の許可（上限・account・費用）が要る。

## 提案
- P2-J の GET・UI は、phase が achieved にならない理由として `pc/achieved-unverified` と未達の Outcome の id を出す。
