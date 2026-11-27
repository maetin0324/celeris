---
title: 戦略層 v1 最終報告（子 9）— Research/Strategy decision layer と人の observability の照合
tasks: [01M7XQA79910VEZ66HWTX4HZVT]
wu: final-report
status: done
updated: 2026-11-27
completed: 2026-11-27
---
# 戦略層 v1 最終報告（子 9）

依頼キー celeris-strategy-dialogue-observability-v1-20261120。親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE、この task 01M7XQA79910VEZ66HWTX4HZVT（子 9）。
基準 main は 7f69057907ad649829ec5a08b5ac7c892b9fe055（2026-11-20）。HEAD は提出時の HEAD で、書いた時点の `git merge-base HEAD main` は本番の release 8c44c114c155 と同じ commit（子 8 の統合の merge）。
材料: 子 9 の WU の記録 [safety-tests.md](safety-tests.md)・[audit.md](audit.md)（[audit.json](audit.json)）・[metrics.md](metrics.md)（[metrics.json](metrics.json)）、子 0〜8 の進捗（`agent-docs/progress/2026-11-2*-strategy-v1-<n>.md`）と ADR。

## 結論

**人が決めること（推奨付き）**

1. 本番の release: 子 0〜8 は、operator が常設の指示で 2026-11-27T13:51:34Z に昇格した release 8c44c114c155 に**すでに入っている**。人が決めるのは、この release を残すか rollback するか。子 9 の差分は文書だけで、昇格する code は無い。推奨は「残す」。理由: 安全の試験 384 件が pass し、親の評価器は本番で動いていない（この案件の controller の行 0、checkpoint は shadow 以下）。
2. 成果物の公開: 公開できる成果物は無い。publish の manifest は本番で作られておらず、署名は 0 件。子 8 の pilot は合格線を満たしていない。推奨は「公開しない」。
3. replan: 親の目的は**未達**。下の R1〜R3 から 1 つ選ぶ（子は増やさない）。推奨は R1（決定的な下限を置き、LLM 費用 0 の replay で先に識別する）。

**判定**（子の done ではなく、子 8 の pilot・試験・DB の監査で測った結果）

| 目的 | 判定 | 主な根拠 |
|---|---|---|
| 改善 A（多数の仮説・代替戦略の比較） | 部分 | 入口の gate は試験で確認済み。本番で使われたかは未確認。`strategy` を付けない起票は止まらない |
| 改善 B（版・引用付きの共有と親の学習） | 部分 | 契約・引用の失効・反証を消せない鎖は試験で確認済み。親が学んで次の版を書いた記録は 0 |
| 改善 C（停滞の検知と pivot） | **未達** | 評価の記録 28 件（子 6 の 4・子 8 の 24）で pivot・識別実験は 0 件、`off_track` も 0 件 |
| 改善 D（局所 review と親の Evaluator の分離） | 部分 | run の分離は確認済み。評価の質は answer key との verdict の一致が 4/12 |
| observability | 部分 | 概要 14 項目・人だけの署名は試験で確認済み。人の判断時間は測っていない。本番の画面では確かめていない |

- 子 8 の方式 b（途中の親 Evaluator）と方式 c（Evidence ＋ 複数戦略）は、どちらも rubric の合格線を満たさなかった（合計点 a 5・b 10・c 11、replay の条件が偽）。合格を示せなかったので rollout していない。
- 4 軸の点は人ではなく `model:claude-fable-5-1` が付けた（人 rmaeda の 2026-11-27 の決定）。人の独立の採点は無い。

## 原目的との照合

各子の done は数えない。新しい型・module・API を足したこと自体も成果に数えない。測定（子 8 の pilot の採点と指標、試験の結果、本番 DB の監査）で判定した。

### 改善 A

判定: **部分達成**。

- 確認済み（試験）: 子 5 の gate は、`strategy` 付きの `task.create` で `selected` の無い版（実験段階）から compound の起票を 422 で拒否し、open の候補ごとに atomic の識別実験 1 件（最大 3 件）だけを通す。子 0 の E-mid の記録だけを使う shadow の試験では、戦略を選ぶまでの起票は 5 件で、アーキv4 の実際の初回の起票は 8 件だった（子 5 の進捗「結論」）。子 9 の safety-tests の 3 本目で `strategy_gate_review_` 9 件・`strategy_review_shadow_` 5 件ほか計 42 件が pass。
- 確認済み（子 8 の pilot）: 方式 c の版 v1 は 4 件とも候補 3 件・識別実験・反証条件・停止条件を持ち、Critic が 4 件中 3 件で実験の欠陥を挙げた。
- 未達・未確認:
  - `strategy` を付けない `task.create` は従来どおり通る（子 5 の ADR の残る穴）。本番で CoS が `strategy` を付けて起票した件数は監査していない（未確認）。
  - 子 8 で版 v1 の承認も v2 も流れていない。候補の比較が固定計画より良い選択につながるかは示せていない。
  - この案件自身は、人が指定した子 0〜9 の固定の 10 子を順に最後まで実行した（子 6 の第二ゲートの後も続けた。下の「段階ゲート」）。

### 改善 B

判定: **部分達成**。

- 確認済み（試験）: `celeris.strategy/1` の契約（stance・confidence・参照・content_hash・版の鎖・idempotency・入力長の上限）。反証の証拠を次の版で消すと拒否する（`strategy_contract_` 20 件 pass）。chat の引用は revision と sha256 を持ち、元の発言の編集・撤回で失効する（`chat_message_edit_` 15・`chat_message_retract_` 2・`chat_discussion_summary_` 14 件 pass）。外部 LLM との交換の envelope は出典・hash・consent を持つ（`external_exchange_` 28 件 pass）。
- 確認済み（子 8 の pilot）: 評価の記録の `learned`・`refuted` は参照付きで、参照の無い主張は 24 記録で 1 件。研究ケースの反証（11.20）を方式 b・c が `refuted` に記録した。
- 推定: model の epistemic progress は研究ケースで b・c 2、a 0。model の採点なので推定に留める。
- 未確認・未達: 親が反証から学んで次の版を書いた記録は 0（子 8 で v2 は 0 件）。本番の DB で戦略の文書が何件作られたかは監査していない。

### 改善 C

判定: **未達**。

- 確認済み（試験）: checkpoint は 12 時点のうち C-mid・D-done・F-done で発火する（`parent_checkpoint_shadow_fires_at_c_mid_d_done_f_done`）。pivot → v2 → 人の承認 → 子 5 の gate の連鎖は 1 本の試験で通る（入力はスクリプトの評価で、LLM の評価ではない）。
- 確認済み（子 8 の pilot）: A〜H の完走の前に兆し（at_risk ＋ ask_human）を出した反復はある（方式 c の iter-1 の C-mid、方式 b の iter-2 の D-done）。どちらも、もう一方の反復では検知しなかった。
- 未達の根拠:
  - 独立の評価器の評価から pivot・識別実験が選ばれた記録は 0 件（子 6 の 4 run、子 8 の 24 記録）。verdict `off_track` も 0 件。
  - 機械の pivot の時点の点は replay で a・b・c とも 0（2 反復の最小の規則）。
  - 研究ケースでも、反証の後に実験設計を直した版（v2）はどの方式も出していない（next は ask_human）。
- 本番: 案件 01M2WTS3DKNZBSZ2JMVB4CZMBW の `project_controller_state` は 0 行、`project_controller_decisions` は 0 件（metrics.md）。本番では途中の親評価は動いていない。

### 改善 D

判定: **部分達成**（分離は確認済み、評価の質は未達）。

- 確認済み: 親の評価は `SideReviewKind::ParentEvaluation` の reviewer run で、final review と別の run・別の記録（子 6 の本番の起動口の 4 run、子 8 の方式 b・c）。低抽象度の task では reviewer run が増えない（`parent_checkpoint_regression_` 4 件 pass）。Critic は戦略の文書の task の final review で既存の reviewer が判定する（`strategy_critic_` 13 件 pass）。
- 確認済み（子 8）: 評価器の入力に承認済み Roadmap の Outcome と D6 の到達を入れた（子 6 の欠落の修正。production +77 行）。
- 未達: answer key との一致は verdict 4/12・next 6/12（うち 2 は invalid の既定値の一致）。評価器は 24 点中 8 点で作業 tree を調べず prompt だけで判定した。方式 c は 12 点中 4 点が turn の上限 30 で invalid。
- 注意: この案件には承認済み Roadmap の版も Outcome も無い（`project_roadmap_versions` 0 行。audit.md）。本番でこの案件に評価器を使うと、goal の軸は到達を測れない。

### observability

判定: **部分達成**。

- 確認済み（試験）: 案件の概要 14 項目（原目的・戦略・候補・仮説・反証・知見・出典・実験・予算・判断待ち・危険・公開の準備度など）の API と画面（`project_overview_` 23 件 pass）。確認済み・推定・要判断・矛盾の印と強い警告。publish の manifest（claims → evidence → 再現条件 → 限界 → 署名）と人だけの署名・採否（`publish_manifest_` 15・`publish_signoff_` 17・`proposal_review_` 23 件 pass。guard を外すと 11 件が落ちる）。
- 確認済み（本番 DB の監査）: 2026-11-21 以後、外部 LLM・mcp の発言を human と記録した件数は 0（mcp の 2 件は `mcp:chatgpt-rdc-2`）。この木の決定 9 件はすべて needed_before の前に答えられた。
- 未達・未確認:
  - 人の判断にかかる時間は測っていない（子 8 の 0.93 分は model の wall で、人の時間ではない）。
  - 本物の daemon・gateway・本番のデータで画面を確かめていない（e2e は偽 daemon）。本物の gateway は 403 を 502 `daemon_auth` に置き換えるので、拒否の理由が画面に出ない（子 7 の未確認）。
  - 本番で manifest を作る経路が無い（署名の流れは試験の fixture でしか通っていない）。
  - 人の承認は DB では operator の actor でしか残らない（`human:` の actor の決定 0 件。人が選んだと推す 22 件は note の本文による推定）。

## 段階ゲート

| ゲート | 判定 | 根拠 |
|---|---|---|
| 第一ゲート（子 1、task 01M7XQ9Q4P4ZQ38Y509J9XYAQ0） | 満たした（2026-11-21） | 子 0 の replay の再現（`replay.py verify` exit 0）、4 領域の修正の負の試験と回帰。試験 9 の本体は operator が host で流し exit 0（人が 11:02Z の決定 first-gate-unmet で決めた手順）。子 9 の 1 本目の nextest で子 1 の 36 件が pass |
| 第二ゲート（子 6、task 01M7XQ9QWK58ESMW42Q1TGT3WZ） | WU close-gate は**未達**と判定。人が 2026-11-23 に案 1「条件付き合格」を選んだ | 独立評価（4 run）と pivot の連鎖（試験）は別々に起きたが、独立評価からの pivot は起きなかった |

第二ゲートの 3 条件のその後:

1. 「評価器の質を子 8 の pilot と rubric で測る」: 測った。結果は合格線を満たさない（上の改善 C・D）。**条件は満たされなかった**。
2. 「本番の checkpoint の評価は shadow のまま」: 守られている。子 8 は mode を変えていない。この案件の controller の行は 0。
3. 「answer key に合わせて prompt を直さない」: 守られている。`crates/task-worker/src/change_rules/` の差分 0（子 8 の検査）。

評価: 第二ゲートは未達を子 6 の時点（10 子のうち 7 番目）で検知し、人に聞いた。アーキv4 では親の最初の評価が 2026-11-20T14:12Z で、検知できた時点 E-mid の約 41.6 時間後だった（子 0 の replay）。この案件では計画の途中で検知した。一方で、人の判断で子 7・8 に進み、子 7・8 は改善 C の未達を解消しなかった。結果として、「未達を認識した後も計画を最後まで進めた」というアーキv4 の形を、人の判断の下で繰り返した。

## 前後の比較

### 口の有無（基準 main と HEAD）

子 0 の inventory が「未実装」とした 8 つの口を、同じ pattern（`inventory.tsv`）で `git grep -P -c` して数えた（`crates/ control-ts/ web/ scripts/`）。口があることは成果に数えない（効果は上の照合で判定した）。

| 子 0 の id | 口 | 基準の行数 | HEAD の行数（file 数） | 備考 |
|---|---|---:|---:|---|
| X1 | 仮説（`hypothes`） | 0 | 186（61） | 子 2 の契約 |
| X2 | 戦略候補 | 0 | 6（4） | 子 2・5 |
| X3 | epistemic | 0 | 85（35） | 子 6 |
| X4 | 親の途中評価（`goal_evaluator` など） | 0 | 0 | 実装の名は `ParentEvaluation`・`parent_checkpoint`（基準 0 file → HEAD 49 file） |
| X5 | discuss-only（`discussion_mode`） | 0 | 301（94） | 子 3 |
| X6 | publish の署名・manifest | 0 | 379（25） | 子 7 |
| X7 | Muse | 0 | 0 | Muse 専用の接続は無い。MCP の共通の 3 道具を使う（`docs` を含めると 0 → 10 file、comment と試験） |
| X8 | 外部の抜粋の取り込み（`excerpt_import` など） | 0 | 0 | 実装の名は `exchange::routes`・`/exchange/import`（0 → 12 file） |

- 足していないもの（git diff で確認）: 新しい trait 0、子の task の migration・DB 表 0（基準から HEAD の migration は `0127_cluster_job_waits_nqsv.sql` の 1 件で、件名「cluster job wait: NQSV の poll を…」の別 task のもの）。

### アーキv4 の replay: 子 0 の基準（固定計画）と子 8 の方式 a・b・c

| 項目 | 史実（子 0 の基準） | 方式 a（固定の Task tree） | 方式 b（途中の親 Evaluator） | 方式 c（Evidence ＋ 複数戦略） |
|---|---|---|---|---|
| 未達の検知（iter-1 / iter-2） | 検知できた時点 E-mid（2026-11-18T20:36Z）、親の最初の評価は 2026-11-20T14:12Z（約 41.6 時間後） | 無し / 無し | 無し / D-done | C-mid / 無し（D-done・F-done は invalid） |
| 検知の後の不要な worker run（iter-1 / iter-2） | 137 run・264.25 USD | 137 / 137 | 137 / 1 run・2.27 USD | 0 / 137 |
| pivot・識別実験 | 無し（仮説を最後まで改めず） | 0 | 0 | 0 |
| 合計点（6 軸、model 4 軸 ＋ 機械 2 軸） | — | replay 3・研究 2（計 5） | replay 3・研究 7（計 10） | replay 4・研究 7（計 11） |
| 合格線 | — | — | 満たさない | 満たさない |

- 研究ケース（BenchFS ISC27 task C）: 方式 b・c は反証の時点 11.20 で at_risk ＋ ask_human を出し `refuted` を記録した。実験設計の修正（v2）は出なかった。方式 a は評価を持たない。
- 子 8 の実 LLM の run は 34 run・8.6335 USD（operator が worker の外で、承認の上限 48 run・25 USD の内）。

### 健全性v2・v3・アーキv4 の教訓に照らした評価

| 教訓 | この案件の結果 | 区分 |
|---|---|---|
| 健全性v2: 本体が約 20,000 行純増 | production +18,105 行（Rust +15,615・TS +2,481）。v2 と同じ桁の純増で、そのうち改善 C の効果は示せていない | 確認済み（測定） |
| v3: 親の目的が先頭 1,500 字しか届かない | 子 1 の `prompt_ancestor_objective_`・`prompt_full_objective_`・`planner_prompt_acceptance_` が pass。この run の prompt にも Project.request（sha256 d7b584adb2c5992856f6b6a721c9ec1598b66b3d7ab1fa3610c4feeab871a699 付き）と祖先の目的・受け入れ条件が「全文」の見出しで載っている | 確認済み（試験とこの run の prompt） |
| アーキv4: 子が全部 done でも親は未達 | 子 0〜8 は done で、親の目的は未達（改善 C） | 確認済み |
| アーキv4: 未達の認識の後も固定計画を遂行 | 第二ゲートで未達を検知したが、人の判断で子 7・8 へ進んだ | 確認済み（記録） |
| 本番 Planner の acceptance 0/126 | 子 1 が修正し試験は pass。本番の再監査は誰もしていない | 本番は未確認 |
| needed_before の書式ミスで事後承認 | この木の決定 9 件はすべて needed_before の前に回答。fail-closed の試験 15 件 pass | 確認済み |
| root の ArtifactExists が WU の成果物を見ない | 子 1 が修正（`root_review_artifact_` 5 件 pass）。子 1 を含む release は 2026-11-21 に昇格 | 確認済み（試験）。本番の root review での効果は未確認 |
| userns の 9 件の本体が未実行 | 試験 9 は operator が 2026-11-21 に host で 1 回流した。release gate では今も流れない（8c44c114c155 の gate.json の `host_tests.sudo.run` は false、`off_by: no_new_privs`） | 一部未確認 |

## 安全の反例

子 9 の WU safety-tests が、計画の 3 本の nextest を提出時の HEAD で流した（詳細は [safety-tests.md](safety-tests.md)）。

| # | 対象 | 結果 |
|---|---|---|
| 1 | 子 1（承認要求の fail-closed・root の成果物・prompt の全文）、子 2（未承認・stale・別案件・出典欠損の戦略）、no-progress の停止 | exit 0、`63 tests run: 63 passed` |
| 2 | 子 3（discuss・propose の拒否、編集・撤回の失効）、子 4（外部 LLM の scope・注入・secret の mask・二重取り込み・入れ子の上限） | exit 0、`115 tests run: 115 passed` |
| 3 | 子 2・3 の gate、子 5（固定計画の先行起票の拒否・Critic）、子 6（checkpoint・親の評価）、子 7（人だけの署名・採否、manifest、概要） | exit 0、`206 tests run: 206 passed` |

再現（repo の root で。`|` は filter の区切り）:

```sh
cargo nextest run -p task-core -p task-dispatch -p task-worker -p task-ops -p celeris-credentiald --no-tests=fail --no-fail-fast -E 'test(/rejected_worker_decision|decision_fail_closed_|root_review_artifact_|prompt_ancestor_objective_|prompt_full_objective_|planner_prompt_acceptance_|strategy_contract_|no_progress/) | test(=probe_found_after_libtest_prefix) | test(=probe_read_from_real_child_run)'
cargo nextest run -p task-core -p task-api -p task-dispatch -p task-worker -p celeris-mcp -p celerisctl --no-tests=fail --no-fail-fast -E 'test(/discussion_mode_|chat_message_edit_|chat_message_retract_|chat_discussion_summary_|chat_exchange_|external_exchange_|mcp_discussion_|mcp_external_scope_|rdc_celeris_chat_/)'
cargo nextest run -p task-core -p task-api -p task-dispatch -p task-worker -p task-ops -p celeris-mcp --no-tests=fail --no-fail-fast -E 'test(/strategy_review_contract_|strategy_critic_|strategy_gate_|strategy_review_shadow_|strategy_review_regression_|parent_checkpoint_|parent_evaluation_|project_controller_checkpoint_|publish_manifest_|publish_signoff_|proposal_review_|project_overview_/)'
```

- 変更前に落ちる負の試験: 子 9 は実装前の source に戻して再実行していない。各子の記録（42 行）を safety-tests.md の表に写した。例: 子 3 は同じ試験を基準の src に当てて `discussion_mode` が `1 passed; 10 failed`（基準は discuss でも task を作った）。子 7 は署名の guard を外すと `35 tests run: 24 passed, 11 failed`（operator・CoS・MCP・外部 LLM の署名が通った）。子 4 は scope の変更を戻すと `7 tests run: 2 passed, 5 failed`。exit の記録が無い行もある。
- この 3 本に入らず、HEAD で確かめていない試験: 子 1 の e2e 9 件と `check-skip.sh`、子 2 の `prompt_project_request_`、子 3・7 の web の vitest・playwright、子 6 の `#[ignore]` の 4 件と TS の vitest、試験 9 の本体。
- 子 8 の pilot の指標 7（認可と secret）: secret の形 0・禁じた読み 0・外の読み 0・記録の入力の変更 0（全方式）。

## 出典・会話の認証・人の承認

本番 DB を読み取り専用（`file:/local/celeris/data/db/celeris.sqlite3?mode=ro`）で監査した（2026-11-27T13:34:38Z。詳細は [audit.md](audit.md)）。
再現: `python3 -I agent-docs/progress/2026-11-27-strategy-v1-9/audit_query.py --out <path>`（約 1.4 秒）。

- Project.request: sha256 は d7b584adb2c5992856f6b6a721c9ec1598b66b3d7ab1fa3610c4feeab871a699 で期待値と一致し、`updated_at` は 2026-09-29 のまま。Roadmap の版 0 行・Outcome の event 0 行で、書き換えは無い（承認済み Roadmap はもともと無い）。
- 会話の認証（2026-11-21 以後）: chat の user 発言の author は `human:` 0・`mcp:` 2（どちらも `mcp:chatgpt-rdc-2`。1 件は人の依頼の message 01M7XPWTXR1A0CNWYN7D1HBZWB）・`operator:` 5・author 無し 175（daemon・cron の経路と推定）。外部 LLM・mcp の発言を human と記録した件数は 0。MCP の呼び出しは 111 件ですべて client `chatgpt-rdc-2`。この client の scope に `tasks:decide` は無い。
- 人の承認: この木の決定 9 件（記録上の回答者は operator 8・CoS 1）は、9 件とも needed_before の unit・段が走る前に答えられた。`human:` の actor で答えた決定は 0 件。計画の gate 12 件は operator 11・`human:web` 1。
  - 推定: note の書き出しから、人 rmaeda が operator の対話で選び operator が書き写したものが 22 件（決定 5・gate 11・blocked の問いと途中確認 6）。DB には人の署名が無く、機械的には区別できない。
- 出典の辿り: 成果の claim から生の記録まで辿れる例として、子 8 の採点（tally）→ `pilot-key.json`（sha256 36841f6b189dbecbbb0217108f1118ab235c17c73fe800528ea2b6f8de64aace、写しは `agent-docs/progress/2026-11-24-strategy-v1-8/pilot-key.json`）→ 評価の記録 → 子 0 の replay の slice と answer key（`replay.py verify` で 365 file を byte 単位で再生）。

## 長時間 run の停止

本番 DB の runs と events を読み取り専用で数えた（2026-11-27T13:30Z 時点。詳細は [metrics.md](metrics.md)）。

- wall の最長は 4,207.7 秒（子 3 の WU cite-attach、completed）。上位 10 件はすべて 3,467 秒以上で、6 件は 3,600 秒を超えて終わった。各 run の wall の上限との照合はしていない（未確認）。
- 上限で止まった run は 17 件: turn の上限 16 件、wall の上限 1 件（子 2 の request-prompt、`worker_finished` が無く `wall_ms` 3,609,679 から推定）。
- no-progress の上限（WU の blocked の理由 `limit`）で止まった WU は 0 件。この案件の Project Controller の判断（no-progress guard・予算の停止を含む）は 0 件で、状態の行も無い。USD の上限・quota で止まった task・WU の記録も無い。
- 停止の仕組みの試験: 既存の no-progress の guard 6 件と子 6 の 1 件が pass（safety-tests の 1 本目）。本番でこの guard が発火した実例は、この案件には無い（未確認）。
- task の blocked: `awaiting_plan_approval` 12 回（6 task）・`awaiting_human` 5 回（2 task）・`worker_question` 2 回（1 task）。failed・cancelled は 0。

## コスト

| 区分 | run | USD | 出典 |
|---|---:|---:|---|
| 子 0〜9 とその子孫の task の runs（role はすべて） | 253 | 639.0749 | metrics.md（DB の `usage_json.cost_usd` の和。cost の無い run 6 件は入っていない） |
| 子 8 の実 LLM の pilot（operator が worker の外で） | 34 | 8.6335 | 子 8 の進捗 |
| 子 6 の本番の起動口の盲検の評価（operator、一時 daemon） | 4 | 0.487 | 子 6 の進捗 |
| 計 | 291 | 648.1954 | 推定（3 つの出典の和。子 9 の残りの run は入っていない） |

- 子ごと（USD）: 子 0 7.49・子 1 110.62・子 2 26.38・子 3 69.31・子 4 55.45・子 5 52.97・子 6 145.41・子 7 86.62・子 8 82.86・子 9 1.95（集計の時点）。親自身の run は 0。
- 比較: アーキv4 の全体は 450.35 USD（list price、子 0 の replay）。この案件はそれより約 198 USD 多い。

## LoC

基準 main と提出時の HEAD を、基準の commit の分類規則で数えた（metrics.md）。

```sh
B=7f69057907ad649829ec5a08b5ac7c892b9fe055
T=$TMPDIR/loc; mkdir -p $T/scripts/dev
git show $B:scripts/dev/source-size-report.py   > $T/scripts/dev/source-size-report.py
git show $B:scripts/dev/source-size-report.toml > $T/scripts/dev/source-size-report.toml
python3 -I $T/scripts/dev/source-size-report.py --root . --config $T/scripts/dev/source-size-report.toml --exclusive-classes --rev $B   --format json > $T/base.json
python3 -I $T/scripts/dev/source-size-report.py --root . --config $T/scripts/dev/source-size-report.toml --exclusive-classes --rev HEAD --format json > $T/head.json
```

| 欄 | 基準 | HEAD | 差 |
|---|---:|---:|---:|
| production 全言語（`views.product_only.lines`） | 376,018 | 394,123 | +18,105 |
| production Rust（`views.product_only.rust_lines`） | 331,327 | 346,942 | +15,615 |
| production TypeScript（`matrix.production.typescript.lines`） | 33,557 | 36,038 | +2,481 |
| 試験を含む自前の総量（`views.own_total_with_tests.lines`） | 1,034,966 | 1,090,145 | +55,179 |

- 子ごとの production 全言語の差: 子 0 0・子 1 +341・子 2 +2,125・子 3 +2,697・子 4 +2,796・子 5 +1,142・子 6 +2,960・子 7 +5,146・子 8 +77（和 +17,284）。残りの +821 は子でない main の commit 82 件と子の base の隙間の分（割り付けは未確認）。
- 試験を含む総量の差 +55,179 から production の差を引いた +37,074 は、試験・検査の script・文書の追加である。
- 依頼は LoC の純減を求めていない。純増の理由は各子の進捗の LoC の節にある。ただし改善 C が未達なので、子 5〜7 の production（計 +9,248）の多くは効果を示せていない。

## 初期戦略の replan の提案

人に判断を求める。子は増やさない。どの案も本番の controller の mode を変えず、HPC・canary・外部公開をしない。

### 論点

- 改善 C を、LLM の評価器の判断（pivot・識別実験を選ぶ）で実現する方式は、28 件の記録で 1 度も pivot を選ばなかった。この方式を続けるか、決定的な規則を主にするか、pivot を人の判断に戻すか。
- 第二ゲートの条件付き合格の後に子 7・8 を進めた結果、改善 C の未達は解消されず、production は +18,105 行増えた。次の計画でも同じ順（仕組みを先に広げ、効果を後で測る）を取るか。

### 失敗の理由（記録から言えること）

1. 評価器（`claude-sonnet-5-5`）は兆しを at_risk ＋ ask_human で人に返し、自分では pivot・識別実験・`off_track` を選ばなかった（24 記録で 0 件）。入力の Outcome を直した後（子 8 の eval-input）も同じだった。
2. 同じ入力でも 2 反復で検知が揃わなかった（replay の方式 b・c とも片方の反復だけ）。
3. 方式 c は作業 tree に版と Critic の判定も置き、12 点中 4 点が turn の上限 30 で invalid になった（同じ上限が方式 c に不利に働いた可能性。推定）。
4. 評価器は 24 点中 8 点で作業 tree を調べず prompt だけで判定した。
5. 実行の入口の gate は `strategy` を付けた起票にしか掛からない。この案件には承認済み Roadmap・Outcome が無く、本番では controller も動いていない。本番の実例が 0 のまま pilot で測った。

### 代替戦略

| 案 | 内容 | 最小の識別実験 | 反証条件 | 費用 | 停止条件 |
|---|---|---|---|---|---|
| R1（推奨）決定的な下限を主にする | 第二ゲートの案 2。D6 の到達（Outcome 未達）と子の終端の数・no-progress の連続から、決定的な規則で `escalate`（人への判断要求）を出す。LLM の評価器は識別実験の案を書くだけにする | 子 0 の replay の 12 slice と研究ケースの 7 slice に規則を当て、検知の時点と誤検知を数える（LLM を呼ばない。repo の script だけ） | E-mid より前（A-done〜C-done）で 2 回以上発火する、または F-done までに発火しない | LLM 0 USD。worker の WU 1〜2 本（1 本 3〜6 USD の実績） | 反証条件に当たったら規則の案を捨て、R3 に移る |
| R2 評価器の入力と予算だけを直して測り直す | prompt の文面は変えない（子 6 の条件 3）。turn の上限 30 → 60、評価の前に slice の file を読ませる入力の形、反復を 3 回にする | replay の C-mid・D-done・F-done × 方式 b・c × 3 反復（方式 b 9 run・方式 c 15 run の計 24 run）と、人が 1 事例を採点 | 18 記録で pivot・識別実験が 0 件、または verdict の一致が 4/12 から上がらない | 約 6〜11 USD（推定。子 8 の replay の 1 反復の実績 b 0.38・c 1.42 USD × 3 反復、turn の上限を倍にして最大 2 倍）と人の採点の時間 | 15 USD を超えたら止める。反証条件に当たったら LLM の評価器を助言だけに下げる |
| R3 縮小: pivot は人が決める | 評価器の出口を ask_human に固定し、人が概要の画面で判断する。改善 C は「人への判断要求を早く正しく出す」に狭める。効果の無い経路（方式 c の版 v2 の自動の流れなど）の削除を検討する | 人の承認の下で 1 案件の shadow の記録を、子 7 の概要の画面で人が読み、判断までの時間と見落としを記録する | 人が at_risk の記録に気付かない、または判断に 10 分以上かかる | LLM は shadow の評価だけ（1 時点 3 USD 以下の上限）。人の時間 | 1 案件・2 週間で効果が見えなければ止める |

## release と publish

本番の release は 8c44c114c155（2026-11-27T13:51:34Z 昇格、actor `external_operator:claude-code-ops-session`、policy_ref `memory:promote-without-asking`）。この commit は main の先端で、子 9 の `git merge-base HEAD main` と同じ。
この昇格は監査（13:34:38Z）の後なので、audit.md の昇格の 18 回には入っていない。

各子の進捗 file を最後に変えた main の commit が、本番の release の祖先かを `git merge-base --is-ancestor <commit> 8c44c114c155` で確かめた（exit 0 = 入っている）。

| 子 | main に入れた統合 commit の件名 | is-ancestor | 最初に含んだ release（昇格の時刻） |
|---|---|---:|---|
| 0 | integrate wu/repro (phase evidence)（最後の commit「strategy-v1 0: 進捗に提出時の検査の結果を書く」） | 0 | 147c84783273（2026-11-21T01:12:57Z） |
| 1 | integrate wu/gate (phase close) | 0 | 53a88490a5d9（2026-11-21T14:05:55Z） |
| 2 | integrate wu/close (phase close) | 0 | 9e33fe4f46f4（2026-11-21T21:01:17Z） |
| 3 | integrate wu/close (phase close) | 0 | 3c1a45964d22（2026-11-22T09:32:09Z） |
| 4 | integrate wu/reclose-progress (phase reclose) | 0 | bc8ad17cbaa3（2026-11-22T20:48:45Z） |
| 5 | integrate wu/adr-scope (phase adr-fix) | 0 | 45ab21a81960（2026-11-22T19:48:26Z） |
| 6 | integrate wu/stale-names (phase rename) | 0 | f7677970b32a（2026-11-23T17:48:59Z） |
| 7 | integrate wu/reclose-verify (phase reclose) | 0 | 3acae8dec315（2026-11-24T09:20:24Z） |
| 8 | integrate wu/close-verify (phase close) | 0 | 8c44c114c155（2026-11-27T13:51:34Z） |
| 9 | integrate wu/safety-tests (phase verify) ほか（未統合） | 1 | 入っていない（文書だけ） |

- 2026-11-20 以後の昇格は、release の記録で 18 回、すべて actor `external_operator:claude-code-ops-session`（ほかに 2026-11-20T15:43Z の 1 回は actor が未確認）。worker・CoS・mcp の actor の昇格は 0。
- 依頼の規則「release の本番昇格は人だけ」に対し、昇格は operator が人の常設の指示で行っている。指示そのものは DB・台帳に無い（audit.md）。
- publish: 署名の決定（`publish-signoff-v<n>`）は DB 全体で 0 件。成果物の公開・外部公開はされていない。
- 子 9（この WU を含む）は、昇格・publish・外部公開・本番 DB への書き込みをしていない。`release.sh`・`verify.sh`・`promote.sh` も実行していない。

人が行う release の手順（`docs/ops/selfdeploy.md` §1・§2）:

1. この release を残す場合: 何もしない。子 9 の文書を release に入れたいときは、子 9 が main に入った後に `scripts/selfdeploy/release.sh <main の sha>` → `scripts/selfdeploy/verify.sh <sha12>` を流し、GUI の「リリース」画面（`/releases`）で昇格する。
2. rollback する場合: `scripts/selfdeploy/rollback.sh`（`docs/ops/selfdeploy.md` §1）で直前の版 cb5da040629b に戻す。この版は子 0〜7 を含み、子 8 を含まない。子 1 より前の版まで戻すと、子 1 の安全の修正（fail-closed・root の成果物・prompt の全文）も外れる。
3. 確認: `/releases` の画面で現在の版と verify の結果を見る。

## 画面のリンク

path は Web UI の origin からの相対。出典は web の router の定義。

| 画面 | path | 出典 |
|---|---|---|
| 案件の概要（原目的・戦略・判断待ち・公開の準備度の 14 項目） | `/projects/01M2WTS3DKNZBSZ2JMVB4CZMBW` | `web/routes/projects.$id.index.tsx:6`、`web/routeTree.gen.ts:217`。14 項目の節は `web/features/projects/project-detail-view.tsx:342` |
| CoS thread（人の依頼の発言へ移る） | `/?thread=01M5JJTVPW15CKFRCA0FAYCHK4&message=01M7XPWTXR1A0CNWYN7D1HBZWB` | `web/routes/index.tsx:5`（search の `thread`・`message`）、`web/routeTree.gen.ts:57` |
| 親 task | `/tasks/01M7XQ9PS0AFKKMRJ7P5ZMKQQE` | `web/routes/tasks.$id.index.tsx:7`、`web/routeTree.gen.ts:228` |
| 子 9（この報告。成果物の tab） | `/tasks/01M7XQA79910VEZ66HWTX4HZVT?tab=artifacts` | 同上。tab は `web/features/tasks/task-detail-tabs.ts:3` |
| 子 8（pilot の結果） | `/tasks/01M7XQA74FP70G3T3KSTP02S0W` | 同上 |
| 子 6（第二ゲート） | `/tasks/01M7XQ9QWK58ESMW42Q1TGT3WZ` | 同上 |
| 子 0〜5・7 | `/tasks/01M7XQ9PZQ3DQZMYKNFNPTPBQJ`・`/tasks/01M7XQ9Q4P4ZQ38Y509J9XYAQ0`・`/tasks/01M7XQ9Q9EB3CH59TSNYM9HBC8`・`/tasks/01M7XQ9QE8STND208NXG90F743`・`/tasks/01M7XQ9QK1KC34Q2A6PJ960TJB`・`/tasks/01M7XQ9QQXY1DY56EXE0TT8X6W`・`/tasks/01M7XQ9R1EHA8GQ0TZVRM4FBVX` | 同上 |
| リリース（現在の版・昇格・rollback） | `/releases` | `web/routes/releases.tsx:5`、`web/routeTree.gen.ts:122` |
| 受信箱（この案件の判断待ち） | `/inbox?project=01M2WTS3DKNZBSZ2JMVB4CZMBW` | `web/routes/inbox.tsx:7`、`web/routeTree.gen.ts:107` |

- publish の manifest の画面 `/tasks/$id/publish/$manifestId`（`web/routes/tasks.$id.publish.$manifestId.tsx:7`）は、本番に manifest が無いので開ける対象が無い。

## 未確認の事項

- 人の独立の採点（4 軸は model の採点）。同じ provider の model で評価器を採点した偏り。採点者が `data.json` だけを読んだこと（operator の申告）。
- 人の判断にかかる時間（observability の効果）。本物の daemon・gateway・本番のデータでの概要・manifest の画面。本番の大きさでの概要の応答時間。
- 評価器が pivot・識別実験・`off_track` を選べるか（28 記録で 0 件）。方式 c の invalid 4 点の答え（turn の上限を上げた場合）。
- 本番で CoS が `strategy` を付けて起票した件数、戦略の文書の件数（監査の範囲外）。
- 本番 Planner の acceptance の再監査（子 1 の修正の本番での効果）。root review が本番で WU の成果物を見つけた実例。
- 試験 9 の本体の release gate での実行（gate は NoNewPrivs で流さない。子 1 の U2、別 task 01M7YWAC1BVNG09WKDQJ6CA86Z）。
- 子 9 で実装前に戻した負の試験の再現（子の記録だけ）。safety-tests の 3 本に入らない試験（子 1 の e2e、子 2 の `prompt_project_request_`、web の vitest・playwright など）の HEAD での結果。
- 2026-11-20T15:43Z の昇格の actor。人が選んだと推す 22 件で、人が実際にその選択肢を選んだこと。
- cost の無い run 6 件の費用。上位の長時間 run の wall の上限との照合。LoC の +821 行の割り付け。
- 監査（13:34:38Z）・集計（13:30Z）の後の event（子 9 の残りの WU、親の closeout）。
