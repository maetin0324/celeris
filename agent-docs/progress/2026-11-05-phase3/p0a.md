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
- 再発防止の不変条件 I1〜I14 と試験のケース K1〜K43（K1b を含む）を ADR `agent-docs/adr/2026-11-05-phase3-achievement-invariants.md` に固定した。
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
| （関連）外部効果 | facts の `effects` は `pilot_effect.py` の記録から読む。実 LLM の worker には渡らず、承認は driver の擬似 operator | C06 の effect 0 件で fail | stub harness での配線（fix-effects） | 実 LLM worker への配線、daemon の承認 API（rejected を含む）、worker が書けない記録、crash・並行の二重 0（I10）。main の `pilot_effect.py` は記録を読んでから追記し lock が無い | P0-D |
| （関連）自律復帰 | brief の更新は wake にならない。答えのある問いは `ANSWERED_RECHECK_SECS`=3600 秒ごとに再評価。予算超過・TTL は blocked で人が mode を置き直すまで止まる | — | — | brief・authority・Roadmap の編集の wake（編集の handler は wake を書かない）、毎時の再評価の削減、予算の翌日の扱い、chat run の token（`project_run_usage_since` は `runs` だけ。I14） | P1-G |

## 兄弟 task と分担（新規起票なし）
| key | id | 依存 | 主な範囲 |
|---|---|---|---|
| P0-B | 01M6N7XY9H31GK4K0G0J1C6AQY | P0-A | I1〜I4・I11、K1〜K12・K41 |
| P0-C | 01M6N7XY9KX62X9TX696TEHXNF | P0-A | I5・I6・I12、K13〜K16・K23・K42 |
| P0-D | 01M6N7XY9N8J21RRGC8AP6EZ4A | P0-A | I10、K17〜K20・K25〜K28 |
| P0-E | 01M6N7XY9R6FREMRKFFVS4ENMG | P0-A | I7〜I9、K3・K21・K22 |
| P0-F | 01M6N7XY9T4B60EWWRYVYE2EVH | B・C・D・E | 人の許可の後の実 LLM 再試験（I12 の実測） |
| P1-G | 01M6N7XY9W412TA8G2WV0726Z0 | P0-A | wake・予算・human wait、I11 の wake 側・I13・I14、K29〜K40 |
| P1-H | 01M6N7XY9YSB8262BS0BYHZ3Z1 | P0-A | 統括 CoS の実 LLM 評価（この ADR の範囲外） |
| P1-I | 01M6N7XYA1GC9WVPFDSQE39D6V | B・C・D・E・G | 長時間運用と観測 |
| P2-J | 01M6N7XYA31CMJV6RR1KXSQMKH | B〜I | GET・UI・運用手順・rollout、K43 |

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
| `crates/task-api/src/handlers/roadmap.rs` | P0-B（K7 の wake を含む） | P1-G は触らない |
| `crates/task-api/src/handlers/authority.rs`、`crates/task-core/src/store/project_briefs.rs`、`crates/task-core/src/store/project_guard.rs`（`project_run_usage_since`） | P1-G | P0-B は `project_guard.rs` の `sync_phases` に手を入れるなら P1-G の統合時に合わせる |
| `tests/fixtures/long-horizon/`（public・oracle・`MANIFEST.sha256`）、`crates/campaign/src/long_horizon.rs`（採点） | P0-C | P0-D は facts の `effects` を読む既存の関数を変えない。`observation_failed` が要るなら P0-C の採点の入口に合わせ、統合時に後の側が直す |
| `scripts/dev/long-horizon-pilot/pilot_effect.py`、driver の効果と承認の関数、`scripts/dev/long-horizon-pilot-llm.sh` | P0-D | — |
| driver の root の acceptance を組む関数、`stub_harness.py` の review の分岐 | P0-E | 検査 script の中身は P0-C が `public/<case>/` に置く。P0-E の試験は合成の task で書き、P0-C に依存しない |
| `stub_harness.py` の成果を書く分岐 | P0-C | — |
| `docs/ops/project-controller.md` | P2-J | 各 task は自分の事実を進捗に書き、P2-J がまとめる（P0-A は古い 2 行だけ直した） |
| ADR | 各 task が自分の付記 file（`2026-11-05-phase3-<key>.md`）を足す | この ADR の末尾に追記しない（同じ file の末尾の衝突を避ける） |

### migration
- 要りそうな task: P0-B（Outcome の状態の重複を防ぐ一意の索引や版の欄が要るなら）、P1-G（wake の期限・予算の再評価の欄が要るなら）。P0-C・P0-D・P0-E は migration を足さない（effect の記録は file、reviewer の理由は既存 event）。
- 番号の予約: P0-B は 0115、P1-G は 0116。後から main に入る側が、先の側の番号を見て振り直す。
- 振り直しで直す所（過去の記憶から）: `SCHEMA_VERSION` の `assert_eq!` 17 か所（`grep -rnE 'assert_eq!\(.*SCHEMA_VERSION.*\b114\b' crates` の実数。`store/tests.rs` に 8 か所、ほかに 8 file で 9 か所）、`crates/campaign/src/ops_r1_trace.rs` の `SUPPORTED_SCHEMA_VERSIONS`、`delivery.rs` の `migration_0042_fills_gaps_*` の版の一覧、`store/tests.rs` の `open_rejects_db_with_schema_version_newer_than_supported`（`SCHEMA_VERSION+1`）、`roadmap_tests.rs` のような「前の版の DB」を作る試験。
- 研究 K2/K4 の測定 binary が読む schema の版（`ops_r1_trace.rs`）は版を足すだけで、凍結評価の値は変えない。

## 受入条件とテストの対応表
### P0-A（この task）
| 受入 | 対応 |
|---|---|
| 0 齟齬の表と対応表 | この file の「齟齬の表」と本表 |
| 1 不変条件とケースを ADR に固定 | `agent-docs/adr/2026-11-05-phase3-achievement-invariants.md` の I1〜I14・K1〜K43 |
| 2 変更範囲・migration・分担、重複起票なし | 「兄弟 task と分担」「変更範囲と migration の競合」。`delegate.json`・`followups.json` は書いていない |
| 3 `test -f agent-docs/progress/2026-11-05-phase3/p0a.md` | 下の証拠 |

### 兄弟の受入 → ケース → 試験（既存は名前、新規は担当が名前を決める）
| task | 受入 | ケース | 既存の試験（保つ） | 新規 |
|---|---|---|---|---|
| P0-B | 未取り込み・root 無し・reviewer だけ pass・検査 fail・違う案件・一部だけ到達で achieved を拒否し、全部で達成 | K1・K1b・K2〜K6 | `root_row_*` 5 件（task-ops）、`project_controller_core_s5_not_integrated_continues`・`…_s5b_no_commits_root_is_not_achieved`（celeris） | P0-B |
| P0-B | 版の差し替え・重複 event・reopen・並行 approval/CAS・再起動で二重 0 | K7〜K11 | A1 の `project_wake_stale_epoch_*`・`project_wake_cursor` の型 | P0-B |
| P0-B | repo 付き root の integrated は `IntegrationMerged` だけ | K12 | `root_row_repo_*` 4 件 | — |
| P0-B | repo の無い調査案件の根拠の規則（objective） | K41 | — | P0-B |
| P0-C | v2 で全 10 課題の再採点と陰性、複数の正解表現が合格、不十分は fail | K13・K14・K23 | `long_horizon_public_contract_*` 3 件（v1 の振る舞い。v2 で置き換える） | P0-C |
| P0-C | v1 の raw を改変しない、grader の隔離と leakage の否定 | K15・K16 | — | P0-C |
| P0-C | 合否が LLM の回答を読まず script で決まる | K13・K14・K42 | — | P0-C |
| P0-C | 全 10 課題の replay の再採点、同じ初期状態と oracle で old/new の比較 | K42 | — | P0-C |
| P0-D | 承認前は効果 0 で拒否 | K17 | `long_horizon_bench_oracle_negative_effect_before_approval`、段 2b の陰性 stub（`test-long-horizon-pilot.sh`） | P0-D |
| P0-D | approved で exactly once | K18 | 段 2b の正例 | P0-D |
| P0-D | rejected で 0 | K25 | — | P0-D |
| P0-D | crash で二重 0 | K26 | — | P0-D |
| P0-D | 重複で二重 0 | K18・K27 | — | P0-D |
| P0-D | worker が audit/effects を書けない | K19・K20 | — | P0-D |
| P0-D | stub 配線を保ち config harness で通る | K28 | `bash scripts/dev/test-long-horizon-pilot.sh` | P0-D |
| P0-E | check fail が reviewer pass に優先し root done・達成にならない | K3 | `failed_command_check_wins_over_reviewer_pass`・`reviewer_only_task_is_unchanged` | 達成まで届かない e2e は P0-E |
| P0-E | 理由・出典・exit を facts/ops/event に残す | K22 | — | P0-E |
| P0-E | stub reviewer が検証できない基準を pass しない | K21 | — | P0-E |
| P0-F | 人の許可の記録、v2 raw、achieved と oracle の一致 | I12 | — | 実測（人の許可が要る） |
| P1-G | 3 日の時計飛び | K31 | `project_controller_replay_suspend_clock_jump_3days` | 重なりの確認は P1-G |
| P1-G | restart/crash | K29・K30 | `project_controller_replay_crash_each_stage`、`project_wake_crash_b1`〜`b6`・`crash_cos_b4`・`b5` | brief の wake は P1-G |
| P1-G | 同じ失敗 | K32 | `project_controller_replay_worker_fails_same_way_5x` | 解消の後の再発は P1-G |
| P1-G | brief/authority の同時編集 | K33 | — | P1-G |
| P1-G | clear（許可・原因の解消で再評価） | K34 | — | P1-G |
| P1-G | deny（却下・取り消しで効果 0、再質問なし） | K35 | `authority_ledger_revoked_after_decision_creates_no_root`・`authority_ledger_answered_question_is_not_asked_again`・`authority_ledger_same_question_not_asked_twice`、`project_controller_replay_human_escalation_repeat` | 却下の後の再評価は P1-G |
| P1-G | 7 日以上の virtual clock | K36 | `project_controller_replay_external_never_replies`（240 時間） | K29〜K35 の複合は P1-G |
| P1-G | pause/cancel・未解決 human_required・危険な予算超過を自動解除しない | K37 | `project_controller_replay_paused_project`・`..._cancelled_project`・`..._budget_day_cap`・`..._token_cap`、`authority_ledger_human_required_cannot_be_lifted` | 時間を流しても解けない確認は P1-G |
| P1-G | human wait だけの毎時の再評価の削減を数値で | K38 | — | P1-G |
| P1-G | CoS chat run が予算と計測に入る | K39 | `project_controller_replay_token_cap`（`runs` の token だけ） | P1-G |
| P1-G | 日次予算の翌日の分類、永続許可なしの root 増殖の禁止（objective） | K40 | `project_controller_replay_budget_day_cap` | P1-G |
| 全員 | 旧 milestones API は 410 | K24 | `milestone_lifecycle_endpoints_are_gone`・`patch_cannot_set_paused_or_cancelled_on_a_project_and_milestones_are_gone`（task-api lifecycle） | — |

| P1-H | 実モデルの評価と旧 CoS との比較、最小修正、拒否と人の権限を保つ | ADR の K の範囲外（A4 の 3 分類を使う） | `bash scripts/dev/test-general-cos-consult-e2e.sh`（接頭辞 `general_cos_consult_`） | P1-H（実 LLM は人の許可が要る） |
| P1-I | 計画と virtual time・replay の結果、RSS の比較、実稼働は人の承認があるときだけ | ADR の K の範囲外 | `project_controller_replay_resource_loop`・`project_controller_replay_resource_v8_100_projects` | P1-I |
| P2-J | GET と UI、ops の文書と checklist、本番の操作をしない | K43 | — | P2-J |

実行した既存試験は下の「証拠」にある。P1-G 向けの一覧は `p0a/existing-tests-p1g.txt`。表の「既存」は保つ試験で、受入を満たす新しい試験の代わりにはならない。

## 兄弟の受入の補足（親へのコメントとして出す文面）
- P0-B: 受入 1 に K1b（改変した TS が `gap` を偽っても Rust が achieved を拒否する）を含める。`validate` の `achieved_with_open_outcomes` を TS の `gap` でなく snapshot からの数え直しにする。
- P0-B: reopen の前の `reviewer_passed` の持ち越し（`root_row`）を K9 で確かめる。
- P0-D: 受入の rejected は K25、crash は K26、重複は K18・K27 で固定した。K27 は `pilot_effect.py` に lock が無いことから足した（赤かどうかは未確認）。
- P1-G: 受入の「同じ失敗」は K32、「clear と deny」は K34・K35、「CoS chat run が予算と計測に入る」は K39。clear と deny の語は記録に定義が無く、ADR の「語の読み方」で読んだ。別の意味なら P1-G の ADR で直す。
- P0-C: v2 の採点は I12 のため、試行ごとに controller の achieved と oracle の合否の不一致件数を summary に出す。
- P0-E: 検査 script の中身は P0-C が持つ。P0-E は acceptance への結線と記録だけを持ち、試験は合成の task で書く。
- 全員: ADR はこの ADR の末尾に追記せず、`2026-11-05-phase3-<key>.md` を足す。migration は P0-B が 0115、P1-G が 0116 を予約し、後から入る側が振り直す。

## 変更（commit の件名）
- `p0a: 達成判定の 6 層の齟齬と不変条件 I1〜I12 を ADR と進捗に固定する`
- `p0a: 進捗に検査の結果を書く`
- `p0a: 差し戻しに対応する（rejected・crash・wake 側のケース、分担の訂正、ops の wake の文）`

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 既存の修正の試験の再実行（生記録 `p0a/existing-tests-summary.txt`、対象の一覧 `p0a/existing-tests-list.txt`）:
  - `cargo nextest run -p task-ops -p task-dispatch -p celeris-campaign -E 'test(/root_row_/) | test(/review_check_wins/) | test(/long_horizon_public_contract|public_contract/) | test(/^roadmap::/)' --no-fail-fast` → exit 0、10 passed
  - `cargo nextest run -p celeris --test project_controller_e2e -E 'test(/core_s5/)'` → exit 0、2 passed（control-ts を `pnpm install --offline --frozen-lockfile` した後）
  - `cargo nextest run -p task-api --test lifecycle -E 'test(/milestone_lifecycle_endpoints_are_gone|milestones_are_gone/)'` → exit 0、2 passed
  - P1-G の対応表に足した既存試験（filter と一覧は `p0a/existing-tests-p1g.txt`）→ exit 0、21 passed
- `bash scripts/dev/test-affected.sh` → exit 0（差分が文書だけなので「試験対象の変更なし」。文書検査 check-human-text 0 findings・progress-index・commit-hashes・doc-links は ok）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok
- `test -f agent-docs/progress/2026-11-05-phase3/p0a.md` → exit 0

## 失敗ケース
- この task で落ちた検査は無い。発見した欠陥（gap の申告を信じる `validate`、効果の無い `mark_milestone`）は既存の試験では検出されない（陰性の試験が無い）。試験は P0-B が K1b・K6 で足す。

## 未解決
- I3(b) の reopen の前の合格の持ち越しが実害になる経路があるかは未確認（P0-B）。
- repo の無い案件の根拠の規則（I4）の具体は未決（P0-B が決める）。
- 親へのコメントは投稿できなかった。`celerisctl api-request POST /api/v1/tasks/01M6N7WD1WXXW0A5FV7RVZ67B7/comments` は exit 1、`error: api-request requires CELERIS_COS_RUN_CREDENTIAL` で、worker の run にこの credential は無い。この file の「兄弟の受入の補足」を正本とする。親へ写すのは CoS の作業として残る。
- K27 が修正前に赤になるかは未確認（P0-D が確かめる）。
- 「clear と deny」の意味は記録に定義が無く、ADR の読み方は推定である（P1-G が確かめる）。

## 次の gate
- P0-B・P0-C・P0-D・P0-E・P1-G・P1-H は P0-A の完了で並列に着手できる。P0-F は B〜E の完了後、人の明示の許可（上限・account・費用）が要る。

## 提案
- P2-J の GET・UI は、phase が achieved にならない理由として `pc/achieved-unverified` と未達の Outcome の id を出す。
