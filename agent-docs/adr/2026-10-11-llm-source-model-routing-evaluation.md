---
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
title: LLM source・model の選択を評価に基づく決定的な方式に作り直す
---
# ADR: LLM source・model の選択を評価に基づく決定的な方式に作り直す

- Date: 2026-10-11
- Status: 実装済み（2026-10-14。selector 2026-10-13、評価 runner 2026-10-12、評価の実施 2026-10-14。本番の enforce は運用セッションの人の決定で、手順は [進捗](../progress/2026-10-11-llm-source-model-routing.md) の「本番への移行」）
- 決定者: 人（2026-10-11 の運用セッション「既存のルーティングシステムの評価方法などを改めて調査し、モデルの評価方法の知見を踏まえて、ちゃんと動作させられるようにする」「source・model の選択と harness の選択を別々にする」）
- 先行: 先行 task 01M4M8D4AP の ADR `2026-10-11-llm-source-harness-selection.md`（branch `celeris/01M4M8D4AP4YY4F9TV09HG30SJ`。main 取り込み前のためリンクにしない。provider の廃止、harness の選択、`RoutePolicy` 境界）、[2026-10-11-breaking-changes-by-default.md](2026-10-11-breaking-changes-by-default.md)
- 根拠: 調査報告 `routing-evaluation-survey.md`（task 01M4M8D4AQ の artifacts。コード・本番 DB の実データ・出典 28 件）

## Context

本番 main の source・model routing は、task の tier（frontier / standard / cheap）→ provider の設定順 → adapter の tier binding で決まる。
`[model_routing] mode = "shadow"` は候補を記録するだけで実行せず、1,189 件の shadow 記録に結果が無い。web の `/models` 画面の手動割り当ては
運用者の編集であって評価の根拠ではない。前回 task 01M4F3V643 の評価（28 問 × 1 試行、harness と model の交絡、費用全欠測）は人の決定で
main に取り込まない。

本番 DB の実データ（runs 5,045、2026-09-25〜10-11）からは model の優劣が付かない。判定付き run は worker の 39% で、coding の主要 5 model の
合格率の Wilson 95% 区間はすべて 0.6〜0.75 で重なる。lane・時期と model が 1 対 1 に近く交絡し、記録方策が決定的（propensity 0/1）なので
オフライン方策評価（IPS・SNIPS・DR）は使えない。

先行 ADR は harness を task 構造から先に決め、source・model の選択を `RoutePolicy` の 1 点に閉じ込め、根拠が無い間は `(source_id, model_id)` の
辞書順を暫定既定とした。本 ADR はその `RoutePolicy` を「評価の根拠表に基づく決定的な選択」に差し替え、根拠表をどう作るか（評価方法）、
どう本番に入れるか（段階と止める条件）、何を消すか（legacy）、どう実装を分けるか（評価 runner／選択器）を決める。

## Decision

### D1. 評価方法

**2026-10-11 人の確認による対象・予算改訂。** 評価する model は次の 7 つだけ。opencode-go の model 群は評価・根拠表の対象外とし、`route_defaults` で動かす。ローカル `qwen3.8-27b` はこの 7 model の比較評価から外し、acp/pi の唯一のローカル候補として明示的な route default に残す（将来別決定で評価する）。

| source / account | model | pilot と本番根拠の問題数 | k | planned calls | quota budget |
|---|---|---:|---:|---:|---|
| claude-oauth / claude account | claude-opus-5-5 | 30 | 2 | 60 | account 合算で週 quota の 15% 以下 |
| claude-oauth / claude account | claude-sonnet-5-5 | 30 | 2 | 60 | 同上 |
| claude-oauth / claude account | claude-haiku-5-5 | 30 | 2 | 60 | 同上 |
| claude-oauth / claude account | claude-fable-5-1 | 12（層別部分集合） | **1** | 12 | 同上。5h 窓をまたぐ場合は分割 |
| codex-oauth / codex account | gpt-6.1-sol | 30 | 2 | 60 | account 合算で週 quota の 15% 以下 |
| codex-oauth / codex account | gpt-6-luna | 30 | 2 | 60 | 同上 |
| codex-oauth / codex account | gpt-6-astra | 12（層別部分集合） | **1** | 12 | 同上 |

同じ 30 問の層別集合を 5 model で共有し、高価 model の 12 問は各 genre/kind 層を保った部分集合として事前に manifest で固定する。pilot と本番根拠で同じ設計を使い、評価対象を都合よく差し替えない。最大計画量は 324 worker calls（Claude 192、Codex 132）。各 account の上限は独立に判定し、週 quota 15% を越える場合は model ごとに問題数を減らしてから人に見積りを提示する。上限超過の実行はしない。

**見積り根拠と制限。** 調査報告 §2 と本番 DB（`quota_estimated` の `method=measured` のみ）を基に、Claude の観測は model/account ごとの token・時間・quota 窓で見積もる。基準値は opus 約 0.87 USD/run・7d 0.14%/run、claude_max_lab の実測群は 7d 0.27%/run、fable は 5h 窓を約 10%/run 消費する観測があり、週 quota への外挿はしない。試行配分と実測の計算詳細は `agent-docs/progress/2026-10-11-llm-source-model-routing/adr-revise.md` に記す。Codex の quota 推定は本番 2,127 件で method が unknown であるため、率を捏造しない。評価直前の 10-run dry-run で usage endpoint 差分から account の実測率を出し、15% 以内の呼び出し数に計画を調整し、費用・quota 見積りを人が承認するまで評価を開始しない。定価換算できない Codex は token と quota 使用量を報告する。

**設計と試行回数。** 同一の層別問題に全比較可能候補を当てる paired 設計。通常 5 model は問題ごと k=2、高価な fable/astra は k=1。pilot と本番根拠は同一の問題数設計（通常 30、高価 12）。この小標本では精密な優劣を保証しない。

**比較単位。** harness を固定し、その harness が話せる source×model だけを比べる。harness を跨いで優劣を流用しない。根拠表の鍵は
`(genre, harness_id, harness_version, source_id, model_id)`。harness の版は実行 command の version 出力（例 `claude --version`）を評価 run ごとに記録する。

**評価集合。** genre ごとに本番 task の done WU から作る。coding は command check を持つ done WU（1,174 件）を元に、
採点の正しさを再実行で検める（noop は失敗、記録された head_commit は成功）。作成時期で開発（〜2026-10-04）と held-out（2026-10-05〜）に分け、
held-out は採否判定にだけ使う。genre・kind（implement / test / repair / design …）で層別する。集合は manifest（問題 id・出所 WU・check・sha256）に
凍結し、問題の追加は版を上げる。

**汚染対策。** worker には acceptance の command を見せず、採点側だけが持つ。同じ問題を同じ model に同一 session で繰り返し露出させない
（試行ごとに新しい session・新しい worktree）。公開 benchmark は使わない。held-out の問題文は評価 runner の出力以外に書き出さない。

**設計と試行回数。** 同じ問題集合に全候補を当てる対応のある設計（paired）。問題ごと k=2 試行（試行内分散と問題間分散を分ける）。
通常候補は 30 問×2、高価候補は 12 問×1 とする。これらは予算制約下の限定根拠であり、Δ=0.15 の検出力を満たす設計ではない。

**指標。** 主指標は pass^1（1 回で決定的 check が通る率）。副指標は reviewer 判定・壁時計・token・定価換算 USD・quota 消費（5h / 7d / monthly 窓）。
時間切れは失敗と別に数える（`timeout` を独立の結果にする）。費用は定額契約でも「定価換算」と「quota 窓の消費率」の両方を記録する。

**区間。** cell（鍵ごと）に Wilson 95% 区間（問題単位の bootstrap も併記）。2 候補の比較は paired McNemar（p<0.05）かつ差の区間が 0 を含まないとき
だけ「良い」と言う。

**合格の基準（根拠が立つ）。** 30 問×2 の候補は少なくとも 24 問で有効な結果を得ていること、高価 model は 12 問×1 の全問で結果があることを最低条件とする。候補 A を選択可能にするには pass^1 の Wilson 95% 下限が 0.5 以上で、かつ対応 paired 差の区間が他候補より劣ると確定していないこと。差を「優越」と呼ぶのは paired McNemar p<0.05 かつ差の区間が 0 を含まない場合だけ。評価数が小さく差を識別できない候補は「同等集合」に入れて選択可とし、根拠表に低確度（k=1 / 部分集合）を記録する。高価 model は k=1 なので試行内分散を推定できず、広い区間の下限順位だけで選ぶと選ばれにくい。従って同等集合は区間が重なる候補（優越・劣位が確定しない候補）として扱い、その集合内では quota 消費、定価換算、時間の順で選ぶ。根拠不足の候補は優越と見なさず、enforce は人の承認を要する。held-out で層別の選択が反転した場合は根拠不成立。比較は点推定順位だけで決めない。

**judge。** 主判定は決定的 check。LLM reviewer は副指標として、被評価 model と別系（source が違う）の model を使い、本番ログの
「決定的 check との一致率」で較正する。reviewer の model 選択の規則は「被評価と別系」とし、これは根拠表とは別の固定規則とする。

**費用の事前決定。** LLM を呼ぶ評価は、eval-budget の人の回答と、実行直前の具体的な呼び出し数・費用・quota 見積りの受信箱決定の後だけ走らせる。
Codex account は quota の実測が無いので、10 run の dry-run で usage endpoint の前後差を測る。opencode-go は今回評価しない。
評価 run は本番の admission（source / account の同時実行・quota 上限）に従い、評価専用の予約枠を別に持って本番の仕事と取り合わない。

### D2. source・model の選択方式

**候補。** 先行 ADR の境界どおり、`enumerate_candidates` → `constrain_for_harness` → `admit_source` を通った適格候補だけが `RoutePolicy::choose` に渡る。
本 ADR は `RoutePolicy` の実装を「根拠表による選択」に差し替える。dispatcher に LLM 呼び出しを入れない。

**根拠表（evidence table）。** 評価 runner の集計結果を DB の表 `routing_evidence` に入れる。列は鍵 `(genre, harness_id, harness_version, source_id, model_id)`、
`evidence_version`（評価集合の manifest sha と集計日）、`n`、`pass`、`pass_lower`、`pass_upper`、`wall_median_s`、`usd_list_median`、
`quota_per_run`（窓ごと）、`timeout_rate`。表は評価 runner の出力からだけ書き、手で編集しない（手で変えるなら評価をやり直す）。

**決定的な選択規則。** 適格候補のうち:
1. 根拠表に鍵が一致する行（genre・harness_id・source_id・model_id が一致し、harness_version は major が一致）を持つ候補を「根拠あり」とする。
2. 根拠ありの候補の中で `pass_lower` が最大の候補 A を取る。A と区間が重なる（`pass_upper ≥ A.pass_lower`）候補を「同等」集合とする。
3. 同等集合の中で quota 消費（task の lane が許す窓の中で最も逼迫している窓の `quota_per_run` ÷ 残量）が最小の候補を選ぶ。同点は `usd_list_median`、
   次に `wall_median_s`、最後に `(source_id, model_id)` の辞書順。
4. task に `model` の明示があれば、先行 ADR どおり候補を先に絞る（根拠表は見ない）。

**fallback。** 選んだ候補が `admit_source` で quota・cooldown・同時実行で落ちたら、同等集合の次の候補、次に根拠ありの残り（`pass_lower` 順）、
最後に根拠なしの候補を `route_defaults` 順で試す。各段の除外理由を trace に残す。全候補が落ちたら fail closed（先行 ADR と同じ）。別 source へ黙って
落とすことはしない（落とした事実を event に残す）。

**根拠の無い genre の既定と lane。** 根拠表に選択可能候補が無い間は、先行 task 01M4MEEKX485VD5SWKX0P30D4Y の `route_defaults`（harness ごとの明示的候補優先列）を使い、辞書順 fallback は行わない。`route_defaults` は候補の優先順位を供給する唯一の既定経路とし、lane ごとの人の既定もこの設定を拡張した harness×lane の優先列としてここに統合する。別の `[routing.defaults.<genre>]` 設定は設けない。lane は model alias ではなく、frontier / standard / cheap ごとの優先列選択ラベルである。

選択順は (1) task に明示された model/source 制約、(2) enforce 対象なら根拠表の選択可能集合、(3) 同等集合内の quota 順、(4) shadow または根拠無しなら harness×lane の `route_defaults` 順。根拠表と人の既定が競合する場合、enforce では根拠表を使い、人の既定は shadow / 根拠無し期間にのみ適用する。人が enforce 中の既定を変更したい場合は、その genre×harness の段階を shadow に戻して設定・再承認する。trace に根拠表選択か `route_defaults` のどちらを用いたか記録する。

**判断の記録。** 選択は根拠表・`route_defaults`・model 指定のどれで決まっても、[2026-10-11-task-routing-features.md](2026-10-11-task-routing-features.md) の `celeris.route-decision/1`（Input = task の自然言語記述・context・constraints・candidates と根拠表の値、Output = selected・action・候補ごとの score・propensity）で DB に残し、結果（`celeris.route-outcome/1`）と結合して export する。`RoutePolicy` の入力はその ADR D9 の `RouteInput` で、decision model（判定器）は dispatcher の外で判定して DB に書いたものを読むだけにする。

**探索。** enforce 中の genre では ε=0.1 の確率で、同等集合の中から選択規則と違う候補を選ぶ（乱数の種は run_id から決定的に導く。再現可能）。
探索の有無と propensity を `routing_decided` に記録し、以後のオフライン評価（DR）の材料にする。探索は同等集合の外には出ない（劣る候補を本番に出さない）。

### D3. shadow から enforce への段階と止める条件

段階は genre×harness ごとに独立に進める。状態は DB の表 `routing_stage` に `(genre, harness_id, stage, since, evidence_version)` で持ち、
config には書かない（daemon の決定で戻すため）。

| 段階 | 選択 | 記録 | 進む条件 |
|---|---|---|---|
| `shadow` | `route_defaults`（harness×lane の優先順） | 新 selector が選んだはずの候補・同等集合・除外理由を `routing_decided` に記録 | D1 の「根拠が立つ」を満たし、人が受信箱で enforce を承認 |
| `enforce` | 根拠表による選択＋ε探索 | 同上に加え propensity と実際の結果 | guardrail を 2 週間（または 200 run）違反せず、人が承認すれば `enforce` のまま探索を続ける |

**guardrail 指標（直近 50 run の窓、genre×harness ごと）。** (1) 決定的 check の合格率、(2) run 失敗率（error・timeout）、(3) 壁時計の中央値、
(4) account ごとの quota 窓の残量。

**止める条件（daemon が決定的に判定し、`enforce` → `shadow` へ戻す）。**
- 合格率: 直近 50 run の Wilson 95% 区間の上限が、根拠表の選ばれた候補の `pass_lower` を下回る。
- 失敗率: 直近 50 run の失敗率の区間下限が、shadow 期間の失敗率（根拠表の `timeout_rate` と本番 shadow 期間の error 率）の 2 倍を超える。
- 壁時計: 直近 50 run の中央値が根拠表の `wall_median_s` の 2 倍を超える。
- quota: 選択先 source の account の最も逼迫した窓の残量が 10% を切る（この場合は fallback で済むので段階は戻さず、受信箱に知らせる）。
- 根拠表の版が古い: `evidence_version` の harness_version の major が本番の harness の版と違う。

戻したら `routing_stage_changed` event（理由・窓の数値）を追記し、受信箱に通知する。再び `enforce` に進むには人の承認が要る。
自動で `enforce` に進むことはしない（進むのは人の決定だけ、戻すのは daemon の決定だけ）。

**peeking。** 窓は固定長（50 run）のスライドで、閾値は上の固定値。always-valid 区間は使わない（運用で見るのは daemon の判定結果だけで、
人が途中で区間を眺めて止める運用はしない）。

### D4. legacy の扱い（同じ変更で消すもの・残すもの）

[breaking-changes-by-default](2026-10-11-breaking-changes-by-default.md) D1・D3 に従い、selector を入れる変更（build 段 `selector`）で次を消す。

| 現行の仕組み | 決定 | 消す場所（main の所在） |
|---|---|---|
| legacy tier → provider → `tier_models` の binding | 削除（先行 ADR が provider を消す。残る tier binding の経路も消す） | `crates/task-dispatch/src/dispatcher/provider_select.rs` の tier 解決、`crates/task-core/src/model_routing.rs` の `LaneResolution` |
| `[model_routing]` の `mode`・`models`・`deployments`・`policies`・`estimator`・`enforce_routes`・`subscription_windows`・`resource_groups`・`retry`・`escalation`・`shadow`・`context_safety_margin` | 削除。config にあれば読み込みエラーにし、新しい `[routing]`（harness×lane の `route_defaults`、`exploration_epsilon`、`guardrail` 窓と閾値、評価専用予約枠）を案内する | `crates/celeris/src/config/model_routing.rs`、`crates/celeris/src/config/model_catalog.rs` |
| `HeuristicEstimator`・estimator sidecar・optimizer・profiles の `quality.index` | 削除（根拠表に置き換える） | `crates/task-core/src/model_router/{estimator.rs,estimator/,optimizer.rs,profiles.rs,policy.rs}`、`crates/celeris/src/daemon/routing_sidecar*.rs` |
| 旧 shadow（`dispatch-enforce-heuristic-v1`、decision shadow・実行 shadow・proxy shadow・`routing_shadow_reservations`） | 削除。新 shadow は D3 の `shadow` 段で `routing_decided` に記録する（別の event 型を増やさない） | `crates/celeris/src/daemon/routing_shadow*.rs`、`crates/task-dispatch/src/dispatcher/routing_shadow.rs`、`crates/task-core/src/model_router/shadow*`、`crates/task-core/src/store/routing_shadow*.rs` |
| `routing_enforce`（proxy 側の enforce 経路） | 削除（選択は dispatcher の `RoutePolicy` の 1 点） | `crates/task-dispatch/src/dispatcher/routing_enforce.rs`、`crates/llm-proxy/src/selection.rs` の lane 優先規則 |
| 手動割り当て `model_assignments`（web `/models` の role/tier slot、API、CoS ops） | 削除。人が model を固定したいときは task の `model` 欄（先行 ADR）か `route_defaults` の harness×lane 優先列 | `web/features/ops/{model-assignments,model-role-editor,models-screen}*`、`crates/task-api/src/handlers.rs` の assignment handler、`crates/task-api/src/cos/ops/admin.rs` の登録、schema |
| lane（frontier / standard / cheap） | **廃止しない**。task の方針ラベルとして残し、`route_defaults` の harness×lane 優先列を選ぶ。model・source・harness の別名にしない | `crates/task-core` の `Tier` 型は残す |
| 前回 branch `celeris/01M4F3V643Q4A3DXTXPT04J14Y`（estimator/evidence・eval suite v2・tools/routing-eval） | main に取り込まない（人の決定）。評価 runner は本 ADR の D1・D5 で作り直す。流用するのは「凍結 manifest と sha256 で集合を固定する」「noop は失敗・reference は成功を再実行で検める」の 2 つの考え方だけで、コードは持ち込まない | — |
| events `routing_decided`・`routing_shadow_recorded`・`routing_outcome_recorded`・`quota_estimated` | **消さない**（追記専用）。`routing_shadow_recorded`・`quota_estimated` は新規には書かない。`routing_decided` は payload を新 selector の形（候補・同等集合・除外理由・propensity・stage・evidence_version）にし、旧 payload も読めるまま残す。`routing_outcome_recorded` の投影（`crates/task-ops/src/routing_outcome.rs`）は guardrail の入力として残す | — |
| `routing_replay`（frozen export） | 残す（評価集合の出所の抽出に使う）。prompt・応答・credential を出さない方針は変えない | `crates/task-ops/src/routing_replay.rs` |
| web の routing 表示（`routing-state`、`routing-catalog`） | 作り直す: 表示するのは genre×harness ごとの段階・根拠表（区間つき）・guardrail の窓・直近の `routing_stage_changed`。tier slot・estimator 由来の表示は消す | `web/features/ops/routing-*` |

### D5. 実装の分け方

**評価 runner（build 段 `eval-harness`）。** crate `routing-eval`（`crates/routing-eval`、bin `celeris-routing-eval`）。責務:
(1) 評価集合の作成: 本番 DB（読み取り）の done WU から問題を抽出し、再実行で check を検め、開発／held-out に分け manifest を書く。
(2) dry-run: 10 run の quota 実測と、呼び出し数・費用・quota の見積り（受信箱の決定の材料）。
(3) 実行: Celeris の run として評価 run を起こす（harness 固定・source×model 指定・acceptance を worker に見せない・評価専用の予約枠）。
subagent・別の LLM CLI を自分で起動しない。
(4) 集計: cell ごとの Wilson 区間・bootstrap・paired McNemar・Pareto・held-out 検証を出し、`routing_evidence` に書く（`evidence_version` を上げる）。
runner は dispatcher・store に LLM 呼び出しを入れない（LLM を呼ぶのは評価 run の worker だけ）。

**選択器（build 段 `selector`）。** `crates/task-core/src/model_router/` を作り直して `crates/task-core/src/routing/`（`evidence.rs` 根拠表の読み、
`policy.rs` D2 の規則、`stage.rs` D3 の段階と guardrail 判定、`trace.rs` `routing_decided` の payload）とし、dispatcher 側は先行 ADR の
`RoutePolicy` 実装を差し替えるだけにする。guardrail の判定は daemon の tick で決定的に行う（LLM を呼ばない）。
D4 の削除は同じ unit で行う。

**実装の付記（2026-10-13）。** 既存の `crates/task-core/src/routing.rs` は ADR-0061 の harness 骨格として使用中のため、選択器の配置先は名前の衝突を避けて `crates/task-core/src/route_select/` とする。D5 の `routing/` はこのモジュールへ読み替える。

**判断 record と学習データ（build 段 `task-features`）。** task の記述（`celeris.task-features/1`、自然言語の brief が主・aux が補助）・選択肢・判断・結果の表、`celerisctl routing export` の JSONL、`RouteInput`／`RouteOutput` の境界は [2026-10-11-task-routing-features.md](2026-10-11-task-routing-features.md) に従う。評価 runner は manifest 作成時に各問題へ同じ template の features を付け、評価 run の割り当ても同じ判断 record（`stage="eval"`）で残す。

**境界。** runner → selector は DB の `routing_evidence` だけで結ぶ（runner が selector の型に依存しない。selector が runner を呼ばない）。
両方が共有するのは鍵の型 `(genre, harness_id, harness_version, source_id, model_id)` と区間の型だけで、`task-core` に置く。

**試験。** selector は根拠表を固定した単体試験（同等集合・quota 同点・fallback・探索の決定性・guardrail の各停止条件）。runner は manifest の
凍結・held-out 分割・集計（既知の表から区間と McNemar の値が合う）を単体試験し、評価 run の起動は daemon 試験で stub harness に対して
1 問 × 2 試行を回す。外部ネットワークに出ない。CPU を焼かない。

### D6. genre ごとの適用

調査報告 §1c の当てはめを採る。

| genre（harness） | 本 task での扱い |
|---|---|
| coding（claude-code / codex） | 対象 7 model を D1 の設計で評価し、根拠成立時に enforce の対象にする。acp/pi は qwen の route default を使い、opencode-go は対象外 |
| reviewer | judge の較正対象（本番ログで check との一致率）。model 選択は「被評価と別系」の固定規則 |
| literature / web-research / planner / data-analysis / writing / knowledge-curation | 根拠は立てない。harness×lane `route_defaults`。本番で完了率・時間・失敗率を guardrail として記録だけする |
| conversation（CoS）・browser・langmem | 選択点が無いか人の決定事項。評価 run を使わない。conversation は `[routing.defaults.conversation]` に人の既定を書く |

## Consequences

- source・model の選択は 1 点（`RoutePolicy`）・1 入力（根拠表）・1 段階表（`routing_stage`）に定まる。tier binding・手動割り当て・heuristic estimator・
  旧 shadow は無くなり、config の `[model_routing]` は読み込みエラーになる。本番 config の書き換えは進捗 file の「本番への移行」節に書く。
- 根拠が立つまでの選択も harness×lane の明示的 `route_defaults` で一意に決まり、高価な model が辞書順で先頭になる問題を避ける。根拠表と人の既定が別経路に分裂しない。
- 評価は費用と quota を使う。走らせるのは eval-budget の人の回答と実行直前の見積りの承認の後だけ。
- `enforce` へ進むのは人、戻すのは daemon。人の承認なしに本番の経路が変わることはない。
- events は消さない。旧 payload の `routing_decided` と `routing_shadow_recorded` は読み取り API で `legacy` として表示する。

## 付記 2026-10-12: 評価 run の受け口（eval-dispatch）

D1 の評価 run を Celeris の run 経路で起こす受け口の形を固定する（task 01M4Q7C4X2QR3HYPTTQAG5GXQ9）。後続の実装が食い違えばこの節に具体的に追記する。

1. **表 `eval_trials`（migration 0076。統合前の番号 0072）**: 1 行 = 1 試行（harness 固定・source×model 固定・新 session・新 worktree の評価 run 1 本）。
   列は trial_id（ULID）・batch_id（1 回の投入 = 1 batch）・manifest_sha・problem_id・trial_no・source_id・model_id・harness_id・
   harness_version（起動時に埋める）・repo・base_commit・objective（worker に見せる本文）・check_cmd（採点専用、worker に渡さない）・
   check_timeout_secs・status・run_id・decision_id・worktree_path・check_exit・timed_out・error・created_at・started_at・finished_at。
   UNIQUE(manifest_sha, problem_id, trial_no, source_id, model_id, harness_id) で、同じ試行の再投入は既存行を返す（冪等）。
   型は `task_core::routing_eval_trial`（`EvalTrialSpec`・`EvalTrial`・`EvalTrialStatus`）、store は `SqliteStore::{insert_eval_trials,
   eval_trials_list, eval_trial_get, eval_trial_claim_next, eval_trial_unclaim, eval_trial_mark_started, eval_trial_mark_scoring,
   eval_trial_finish, eval_trial_cancel}`。
   状態遷移: `queued → running`（claim。条件付き UPDATE で二重 claim しない）、`running → queued`（unclaim。admission で起こせず run 未記録のときだけ）、
   `running` のまま run を記録（mark_started。1 回だけ）、`running → scoring`（run 記録後）、`scoring → passed|failed`、`running|scoring → error`、
   `queued|running → cancelled`。それ以外は `StoreError::Invalid`。
2. **API**（task-api `routing_eval`）: `POST /api/v1/routing-eval/trials`（本体 `{manifest_sha, trials:[{problem_id, trial_no, source_id, model_id,
   harness_id, repo, base_commit, objective, check:{cmd, timeout_secs}}]}` → 201 `{batch_id, trials:[{trial_id, problem_id, trial_no, status}]}`。
   空 trials・catalog に無い source/model/harness・repo 不在は 400）、`GET /api/v1/routing-eval/trials?batch_id=&status=`（check_cmd は返さない）、
   `POST /api/v1/routing-eval/trials/{trial_id}/cancel`（200 / 409）。認可は既存の書き込み API と同じ。
   実装（`crates/task-api/src/routing_eval.rs`、2026-10-12 trial-api）の細目: 400 は code `eval_trial_invalid`（`detail` に全理由を `; ` 区切りで並べる）で、
   空 trials・256 件超・空欄（manifest_sha / problem_id / base_commit / objective / check.cmd）・未知の source/model/harness・repo のディレクトリ不在・
   `timeout_secs` が 0 または 86400 超・同じ batch 内の重複試行。id の照合先は daemon の設定（`[[providers]]` の adapter・model・lane 束縛・解決した LLM source）・
   routing catalog・`[[genres]]`＋組み込み harness で、source は `-`/`_` と大文字小文字を区別しない（`claude-oauth` = `claude_oauth`）。
   照合先が空の欄（最小構成の daemon）は検査しない。cancel の 409 は `eval_trial_not_cancellable`、無い id は 404 `eval_trial_not_found`。
   `check.timeout_secs` の既定は 900。GET・cancel の応答の型（`EvalTrialView`）には `check_cmd` だけでなく `objective` の欄も無い（`docs/api/v1/gui-api.md` §3.133）。
3. **評価予約枠**: config `[routing_eval] reserved_runs = <n>`（既定 0 = 評価 run を起こさない）。評価 run は本番の全体並列度に数えず、
   専用枠の中だけで走り、tick では本番の dispatch の後に配る。account の同時実行と quota 上限は本番と同じ判定に従う。
   純関数 `eval_admission(reserved_runs, eval_in_flight, account_in_use, account_limit, quota_ok) -> Result<(), EvalDefer>`
   （`EvalDefer = Disabled | ReservedFull | AccountFull | QuotaCap`）を task-dispatch の capacity に置く。
4. **起動**（dispatcher の tick 段 `eval_trials`）: claim → admission（不可なら unclaim）→ harness の版（実行 file の `--version`。
   `harness_version_major` の major と生の出力を両方記録）→ 新 worktree（`git worktree add --detach <data>/eval/<trial_id>/repo <base_commit>`）→
   RunRequest は harness 固定・source×model 固定・新 session（resume・checkpoint なし）・objective だけで acceptance・checks は空、
   置き場は workspace = `<data>/eval/<trial_id>`・work_dir = その `repo`・artifacts_dir = その `artifacts`（run_dir は repo の外。ADR 2026-10-14-eval-trial-result-placement D1） →
   runs 1 行（role="eval"、task_id=batch_id、work_unit_id NULL）→ route_features（template "eval"、subject_kind "eval_problem"）・route_options（候補 1 件）・
   route_decisions 1 行（stage="eval"、subject_id=problem_id、input_json `{manifest_sha, trial_no, candidate}`、
   output_json `{selected:{source_id, model_id, harness_id, harness_version}}`）。role="eval" の run は task 行が無くても回収・統計・受信箱の経路を壊さない。
5. **採点**: worker 終了 → scoring → worktree で check_cmd を `sh -c`（timeout check_timeout_secs、WU check と同じ環境の絞り方）→
   route_outcomes 1 行（labels に passed・check_exit・timed_out・wall_secs・usage・usd_list・run の終了状態）→ passed/failed（異常は error。終了理由の本文を error に残し、runs 行は終端どおりの状態にする。ADR 2026-10-14-eval-trial-result-placement D2/D3）→ worktree 削除。
   daemon 再起動で running/scoring のまま run が消えた試行は理由つきで error にする。
6. **CLI**: `celeris-routing-eval run --api <url> --manifest <file> --candidates <id,...> [--token-file <path>] [--repo <path>] [--wait] [--out <file>]`。
   check の command は POST にだけ載せ、標準出力・要約に出さない。`--wait` は GET の間隔 poll で全試行の終端を待つ。
7. **通し試験**: `crates/celeris/tests/routing_eval_run.rs` が一時 DB・一時 git repo・stub harness・reserved_runs=1 で 1 問 × 2 試行を回し、
   runs(role=eval)・route_decisions(stage=eval、harness_version あり)・route_outcomes が各 2、worktree・session が試行ごとに別、
   stub の prompt に check_cmd が無い、同時に 2 本走らないことを確かめる。

## 付記 2026-10-12: 評価 runner の実装（eval-harness）

D5 の評価 runner（crate `routing-eval`、bin `celeris-routing-eval`）と根拠表の型・表を実装した（task 01M4Q53E9WQEKMAS448GZMWTFB）。後続の変更が食い違えばこの節に具体的に追記する。

1. **harness_version の major（D1 の未解決を確定）**: `task_core::routing_evidence::harness_version_major(harness_id, version_output)`。
   `claude-code`・`codex` は version 出力の中の最初の `X.Y.Z` 形の語の major（`v` 接頭辞は外す。例 `Claude Code 1.8.2` → `1`）。
   それ以外（acp・pi など）は version 出力を trim した全体。major が取れないときも全体を使う。根拠表の鍵は major、生の出力は `eval_trials`（付記 2026-10-12 の 1）側に残す。
   試験は `task-core` の routing_evidence 単体試験が固定する。
2. **評価集合 manifest（`celeris.routing-eval-manifest/1`）**: `manifest.json` の欄は `schema`・`version`（既定 `1`）・`cutoff`（既定 `2026-10-05`。作成時期で dev < cutoff ≤ held-out）・`seed`・`problems[]`・`sha256`（全体）。
   問題の欄は `id`・`task_id`・`work_unit_id`・`created_at`・`genre`・`kind`・`command`（採点用の check。manifest 以外には出さない）・`split`・`stratum`・`subset_12`・`base_commit`・`head_commit`・`features`（`celeris.task-features/1`、作成時点 as_of）・`features_sha256`・問題ごとの `sha256`。
   `recheck` は各問題の check を base で失敗・head で成功と再実行で検め、結果を manifest の `recheck` 欄に書く（除外理由つき）。同じ `version` で sha256 が違う上書きは拒否し、`--version` を上げさせる。
   集合の内訳は ADR D1 の表どおり: 通常 5 model は 30 問（k=2）、fable・astra は 30 問の層を保つ 12 問（k=1）。候補の表は `candidates.rs` の `CANDIDATES` 定数 7 件だけ。
3. **根拠表 `routing_evidence`（統合後は migration 0074 + 0077。付記 2026-10-13 を見よ）**: 列は `genre`・`harness_id`・`harness_version`（major）・`source_id`・`model_id`・`evidence_version`・`n`・`pass`・`pass_lower`・`pass_upper`（Wilson 95%）・`pass_boot_lower`・`pass_boot_upper`（問題単位 bootstrap 95%、seed 42、2000 resamples）・`wall_median_s`・`usd_list_median`・`quota_per_run_json`（窓→率の JSON）・`timeout_rate`・`low_confidence`（k=1・部分集合）。根拠表 Markdown、aggregate JSON、特徴量つき trial export にも Wilson と bootstrap を併記する。
   主キーは鍵 + `evidence_version`。`evidence_version` は `<manifest sha256>-<集計日 UTC>`。同じ版は丸ごと置換し、別の版は追記する（store の `replace_routing_evidence` / `latest_routing_evidence`。付記 2026-10-13）。
   書くのは `aggregate --write-evidence` だけ。API と GUI に編集経路は作らない（ADR D2 の「手で編集しない」）。
4. **評価 run の DB 契約（付記 2026-10-12 の 4 と同じ。aggregate が読む側）**: 1 試行 = `route_decisions` 1 行（stage `eval`、subject_kind `eval_problem`、input `{manifest_sha, trial_no, candidate}`、output `{selected:{source_id, model_id, harness_id, harness_version}}`）＋ `runs` 1 行（role `eval`、run_id で結合）＋ `route_outcomes`（passed・check_exit・timed_out・wall_secs・usage・usd_list）。aggregate は決定的 check の判定・timeout・token・quota を集め、同じ鍵の試行を問題単位で束ねて `stats` に渡す。
5. **dry-run の見積り**: 呼び出し数は問題数 × k、account ごとに合算する。定価換算 USD は本番 runs の model 別の token 中央値に単価表（`dryrun.rs` の定数。claude 4 model のみ）を掛ける。codex の 2 model は単価が無いので USD を出さず「未算定」と書く。quota は `quota_estimated` のうち `method=measured` の行だけを率にする（無ければ「未測定」）。`--measured` は 10 run 実測の JSON（`runs` = 10 が必須）で率を置き換える。週窓の率が 15% を越える account は縮小案を出す。
6. **統計（`stats`）**: Wilson 95%・問題単位の seed 固定 bootstrap・exact McNemar・paired 差の区間・Pareto・根拠成立（30×2 は有効 24 問以上、12×1 は全問）・同等集合（区間の重なり）・`may_claim_superiority`（k=1 は常に偽）・held-out で選択の符号が反転したら不成立。CLI の `stats` は単独入力を持たず、`aggregate` が呼ぶ（CLI から呼ぶと説明つきの失敗を返す）。
7. **本番で評価を走らせる手順は人の決定待ち**: LLM を呼ぶ評価（`run` の本番実行）は、eval-budget の人の回答と dry-run の受信箱決定（呼び出し数・費用・quota の見積り）の後にだけ行う。今回の run では実行せず、試験は stub harness と一時 DB だけで行った（`crates/routing-eval/tests/pipeline_stub.rs`）。

## 付記 2026-10-12（v4）: 候補ごとの harness 固定と cell bootstrap 区間

差し戻し（final review v3）への対応。D1 の候補表に harness 列を足し、評価 run の起動と根拠行の区間を揃えた。

1. **候補表の harness 列**（`crates/routing-eval/src/candidates.rs` の `CANDIDATES`、run の `make_trials` はこの欄を読む）:

   | source / account | harness_id | model |
   |---|---|---|
   | claude-oauth / claude | claude-code | claude-opus-5-5、claude-sonnet-5-5、claude-haiku-5-5、claude-fable-5-1（k=1・12 問） |
   | codex-oauth / codex | codex | gpt-6.1-sol、gpt-6-luna、gpt-6-astra（k=1・12 問） |

   評価 run の trial は候補ごとに固定の harness で起こす（claude 4 候補は claude-code、codex 3 候補は codex）。`eval_trials` の UNIQUE 鍵と `route_decisions` の output に harness_id を残す。
   比較は harness の中だけで行う（`aggregate` の paired 比較は harness が違う組を作らない）。試験は `run::tests::candidates_expand_k_and_subset_and_keep_checks_out_of_objective` と `aggregate::tests::paired_comparisons_never_cross_harnesses` が固定する。
2. **cell の区間は Wilson と問題単位 bootstrap の 2 本**: 根拠行（`EvidenceRow`）の `pass_lower`・`pass_upper`（Wilson 95%）と並べて `pass_boot_lower`・`pass_boot_upper`（問題単位の bootstrap 95%、seed 42・2,000 resamples、各問題の最初の試行を 1 標本とする）を持つ。`routing_evidence`（統合後は migration 0077）・aggregate の JSON と Markdown 根拠表・JSONL export に同じ欄で出す。bootstrap は cell 単位の区間であり、paired 差の区間（比較用）の代わりにはしない。
3. **dry-run の扱いは変えない**: 呼び出し数・定価換算・quota の見積りは dry-run（`celeris-routing-eval dry-run`）の出力のままで、LLM を呼ぶ評価 run は本 ADR の付記 2026-10-12（eval-dispatch）の 7 と同じく人の決定待ち。

## 付記 2026-10-13: 選択器（selector）と評価 runner の統合

子 task selector（01M4Q53EHAF7BS9V3QMM1WY352）と親 task の評価 runner（eval-harness）が同じ表 `routing_evidence` を別々に作っていたので、統合で 1 つにした。

1. **表は 1 つ**: 作るのは selector の migration 0074（`routing_evidence`・`routing_stage`）。評価 runner の欄（`pass_boot_lower`・`pass_boot_upper`・`low_confidence`、`wall_median_s`・`usd_list_median` の NULL 許容、`n`・`pass`・`low_confidence` の CHECK）は migration 0077 で表を作り直して足す。runner 側の旧 migration（0071 routing_evidence・0073 routing_evidence_bootstrap）は消した。評価試行の表 `eval_trials` は 0072 から 0076 に振り直した（route_records は selector の 0073 のまま）。
2. **行の型は 1 つ**: `task_core::routing_evidence::{EvidenceKey, EvidenceRow}`（表の行。runner が書き、選択器・guardrail・API が読む。D5 の「runner → selector は表だけで結ぶ」の表の型）。`route_select` は型を持たない。`n`・`pass` は u64、`quota_per_run` は窓→率の map（表では JSON object）。
3. **store の API は 1 つ**: `TaskStore::replace_routing_evidence(version, rows)`（同じ版は置換）と `latest_routing_evidence(genre, harness_id)`。runner の `aggregate --write-evidence` も前者で書く。旧 `insert_routing_evidence` / `list_routing_evidence` は消した。
4. **値の無い定価・時間は `None`**: 選択の同点比較（D2 の 3）では値のある候補より後に回す。guardrail の壁時計（D3）は根拠表に時間が無ければ見ない。`RouteScore.expected_cost_usd`・`EvidenceSnapshot` と API の `RoutingEvidenceRowView` の `wall_median_s`・`usd_list_median` も null を取る（過去の event の数値はそのまま読める）。
5. **評価 run の経路**: provider 層の撤去に合わせ、評価 run の harness・source・model は宣言（`[harnesses]`・`[llm_sources]`）から引き（`-` と `_`・大文字小文字を区別しない）、account は本番と同じ `admit_source`（手元の評価 run の `(source, account)` も合算）で選ぶ。worker には `SelectedLlmRoute` を渡す。宣言に無い経路の試行は `error`。投入 API の照合先も宣言（`HarnessesReader::source_models` と harness 一覧・`[[genres]]`）に替えた（routing catalog・`[[providers]]` は無い）。
6. **execution class の候補列と lane の候補列**: main の `[route_defaults.<h>].classes.<class>`（ADR 2026-10-11-llm-source-harness-selection 付記 2026-10-13 §5。宣言した class の列は他の列に戻らない）と本 ADR の `[route_defaults.<h>].lanes.<lane>` を一緒に持つ。優先列の決め方は CoS → class の列（宣言があれば、それだけ）→ harness×lane の列 → harness 全体の列（`task_core::llm_route::route_priority`）。lane 列も harness 全体の列と同じ検証（重複・未宣言・互換・proxy 必須）を通す。class×lane の組の列は作らない。class の列を宣言した execution class では lane による既定の切り替えは効かない（根拠表による選択はその列の中で行う）。

## 付記 2026-10-13: 評価 trial の問題文（eval-prompt）

凍結 manifest の各 `problems[]` は必須の `prompt` を持つ。本文は WU の `objective`・`done_when` と親 task の `title`・`objective` から作り、acceptance と checks は含めない。問題文に check command の文字列が含まれる場合は `[check command redacted]` に置換する。`prompt` は問題 SHA と manifest SHA の双方に含める。旧 manifest は `prompt` が無いため読み込みエラーとし、互換分岐は設けない。

`make_trials` は「評価問題であること」「評価 checks を実行・要求しないこと」を伝える短い前置きに `prompt` を続けて worker objective にする。採点 command は引き続き別欄で runner に渡し、worker 本文やログへ含めない。

## 付記 2026-10-13: 評価の実施（eval-run）

実施日: 2026-10-14（pilot やり直し・本評価の投入・再投入・集計。計画は 2026-10-13）。task 01M4VAAKFJHRSVKZ3996NFKSKS。結果の本文は [eval-results.md](../progress/2026-10-11-llm-source-model-routing/eval-results.md)、経過は [eval-run/](../progress/2026-10-11-llm-source-model-routing/eval-run/)。

- manifest: manifest-v2（問題 30 件、cutoff 2026-10-05）、sha256 `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7`。evidence_version `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14`。
- 人の承認: 見積り（estimate.md）を受信箱で承認（2026-10-14）してから投入した。claude 144 試行・7d 15%、codex 132 試行・7d 13.2%（pilot 10 を含む 276 試行）。fable は 0（次の週窓で別承認）、opencode-go は対象外、qwen は評価せず route_defaults のまま。codex は 7d が見積りより速く増えたため queued 93 試行を cancel し、人の指示で codex の上限を 21% に上げて 93 試行を新しい batch で再投入した。claude の上限 15% は投入前の使用率に足す分（9% + 15% = 24%）と人が示した。
- 試行数と状態（旧 pilot を含む全 batch、全部終端）: passed 251・failed 20・error 36・cancelled 93。error は仕組み側（worktree 消失・dispatcher stopped・daemon recovery・yielded）に限り試行ごと最大 2 回再投入した（1 回目 18、2 回目 3）。error・cancelled は n に数えない。学習データ export は 271 行（passed 251・failed 20）。
- cell（genre coding、n・pass・Wilson 95%）: claude-opus-5-5 24/24 0.862–1.000、claude-sonnet-5-5 24/24 0.862–1.000、claude-haiku-5-5 22/23 0.790–0.992（有効問題数 23/24 で根拠が立たない）、gpt-6.1-sol 27/30 0.744–0.965、gpt-6-astra 11/12 0.646–0.985（k=1 部分集合）、gpt-6-luna 21/29 0.543–0.853。全 cell が low_confidence。
- 費用と quota の実績: claude-code の定価換算 USD 合計は opus 41.42・sonnet 20.46・haiku 17.46（codex は USD を報告しない）。7d 使用率は投入前（04:07Z）→ 集計時で claude_max_lab 0.09→0.18、claude_max_personal 0.09→0.19、chatgpt_plus_personal 0.02→0.20（同じ account の評価以外の run を含む上限値）。どれも人の示した上限（claude 24%・codex 21%）の内。
- 集計の runner は outcome の無い試行（error・cancelled）を不合格に数える不具合があり、JOIN に直した手元の runner で集計した。修正は follow-up task（差分は task の artifacts `eval/aggregate-error-exclude.patch`）。

未解決:
- 本番の `routing_evidence` への書き込み（`aggregate --write-evidence`）は人の手順で、上の runner 修正が main に入った後に流す（eval-results.md の手順）。
- haiku の 1 問は yielded が 3 回続き有効な結果が無い。予算を上げた 1 試行か、yielded を不合格に数える人の判断で根拠が立つ。
- fable（12 問×1）は次の週窓で別承認。run ごとの quota（quota_per_run）は試行の run に quota 実測 event が無く空（{}）。

## 改訂履歴

- 2026-10-11: 人の確認を反映。評価対象を 7 model に限定し、fable-5-1 / gpt-6-astra を k=1・層別部分集合、通常 5 model を k=2・30 問に縮小。Codex の quota は実測前に率を仮定せず、10-run dry-run と週 15% gate を必須化。opencode-go は根拠表対象外、qwen はローカル既定候補として明記。辞書順 fallback と `[routing.defaults.<genre>]` を除き、lane ごとの人の既定を harness×lane `route_defaults` に統合。

- 2026-10-11: 人の補足（decision model は自然言語の入力・選択肢・確率）と人の補足 2（Input/Output の形）を受け、判断の記録と学習データの export を [2026-10-11-task-routing-features.md](2026-10-11-task-routing-features.md) に分けて決め、D2・D5 から参照した。
- 2026-10-12: aggregate の根拠行にも問題単位 bootstrap 95% 区間を Wilson と並べ、routing_evidence および Markdown / JSON / JSONL export に保存する（migration 0073、seed 42・2000 resamples）。

## 未解決（2026-10-14 land 段で整理）

- k=1 の高価 model と harness_version の major（claude-code `2`・codex `0`）は評価の実施で決着した（付記 eval-run）。Codex の 1 run の quota は 7d 率でしか測れず、`quota_per_run` は空。
- 根拠表の鍵の綴り: 評価 runner は `claude-code`／`claude-oauth` の綴りで書き、選択器・段階 API は宣言の綴り（`claude_code`）で完全一致に引く。候補表を宣言の綴りに揃えるまで enforce できない（follow-up task）。
- 同等集合の順位: `quota_per_run` が空だと D2 の 3 は定価 USD → 壁時計で決まり、coding×codex では合格率の低い luna が選ばれる。quota の実測が入るまで coding×codex は shadow に置く（進捗の「本番への移行」§1・§8）。
- 本番 run の outcome（`route_outcomes`）は評価 trial にしか書かれていない（task-routing-features ADR D6 の本番経路が未実装。follow-up task）。

### 2026-10-13 selector lane defaults implementation

選択器の設定は `[route_defaults.<harness>]` に harness 共通 `candidates`、lane ごとに `[route_defaults.<harness>.lanes.<lane>]` の `candidates` を置く。lane 列があれば優先し、なければ harness 共通列へ戻る。`[route_selection]` は `exploration_epsilon`（0.1）、`guardrail_window`（50）、`guardrail_quota_floor`（0.1）、source ごとの `evaluation_reservations`（同時 run 数）を持つ。guardrail の品質閾値は D3 の固定条件（pass Wilson 下限対 pass_lower、失敗率 2 倍、wall median 2 倍、quota 10%）であり、設定可能なのは窓と quota floor のみ。

config reader は lane 列の harness/source/model と互換 route を検査し、予約枠の source も宣言必須にする。route-selection 値は epsilon / quota floor が 0..=1、window が正であることを検査する。

検査: `cargo test -p celeris config::llm_route`（この WorkUnit で実行）。

## 付記 2026-10-22: 根拠表は lane ごと・完了単位の本番結果から作る

根拠表の作り方（run 単位の評価 trial の合格率・中央値）は、[2026-10-22-harness-balance-and-lane-first-routing](2026-10-22-harness-balance-and-lane-first-routing.md) D5〜D7 で置き換わる。鍵に `lane` が入り、行は task / WU の完了単位の本番結果（propensity 重み付け）から daemon が定期的に書く。`routing-eval aggregate --write-evidence` は撤去した（評価 trial の集計・レポートは残る）。
