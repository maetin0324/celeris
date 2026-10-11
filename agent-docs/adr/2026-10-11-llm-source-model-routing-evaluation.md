---
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
title: LLM source・model の選択を評価に基づく決定的な方式に作り直す
---
# ADR: LLM source・model の選択を評価に基づく決定的な方式に作り直す

- Date: 2026-10-11
- Status: Accepted（design 段。実装は build・evaluate・land 段）
- 決定者: 人（2026-10-11 の運用セッション「既存のルーティングシステムの評価方法などを改めて調査し、モデルの評価方法の知見を踏まえて、ちゃんと動作させられるようにする」「source・model の選択と harness の選択を別々にする」）
- 先行: [2026-10-11-llm-source-harness-selection.md](2026-10-11-llm-source-harness-selection.md)（task 01M4M8D4AP。provider の廃止、harness の選択、`RoutePolicy` 境界）、[2026-10-11-breaking-changes-by-default.md](2026-10-11-breaking-changes-by-default.md)
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

**比較単位。** harness を固定し、その harness が話せる source×model だけを比べる。harness を跨いで優劣を流用しない。根拠表の鍵は
`(genre, harness_id, harness_version, source_id, model_id)`。harness の版は実行 command の version 出力（例 `claude --version`）を評価 run ごとに記録する。

**評価集合。** genre ごとに本番 task の done WU から作る。coding は command check を持つ done WU（1,174 件）を元に、
採点の正しさを再実行で検める（noop は失敗、記録された head_commit は成功）。作成時期で開発（〜2026-10-04）と held-out（2026-10-05〜）に分け、
held-out は採否判定にだけ使う。genre・kind（implement / test / repair / design …）で層別する。集合は manifest（問題 id・出所 WU・check・sha256）に
凍結し、問題の追加は版を上げる。

**汚染対策。** worker には acceptance の command を見せず、採点側だけが持つ。同じ問題を同じ model に同一 session で繰り返し露出させない
（試行ごとに新しい session・新しい worktree）。公開 benchmark は使わない。held-out の問題文は評価 runner の出力以外に書き出さない。

**設計と試行回数。** 同じ問題集合に全候補を当てる対応のある設計（paired）。問題ごと k=2 試行（試行内分散と問題間分散を分ける）。
pilot は 60 問、本番根拠は 150 問（検出したい差 Δ=0.15、p≈0.6、α=0.05、検出力 0.8 の近似）。

**指標。** 主指標は pass^1（1 回で決定的 check が通る率）。副指標は reviewer 判定・壁時計・token・定価換算 USD・quota 消費（5h / 7d / monthly 窓）。
時間切れは失敗と別に数える（`timeout` を独立の結果にする）。費用は定額契約でも「定価換算」と「quota 窓の消費率」の両方を記録する。

**区間。** cell（鍵ごと）に Wilson 95% 区間（問題単位の bootstrap も併記）。2 候補の比較は paired McNemar（p<0.05）かつ差の区間が 0 を含まないとき
だけ「良い」と言う。

**合格の基準（根拠が立つ）。** ある genre×harness で次を満たすとき「根拠が立つ」と呼び、enforce の対象にできる:
(a) 開発集合で候補ごとに ≥60 問 × 2 試行が完了、(b) 少なくとも 1 候補の pass^1 の区間下限が 0.5 以上、(c) held-out で開発集合の選択結果を
当てても選ばれる候補が変わらない（区間の下限順位が同じ）。比較は常に**区間の下限**で行う（点推定で並べない）。

**judge。** 主判定は決定的 check。LLM reviewer は副指標として、被評価 model と別系（source が違う）の model を使い、本番ログの
「決定的 check との一致率」で較正する。reviewer の model 選択の規則は「被評価と別系」とし、これは根拠表とは別の固定規則とする。

**費用の事前決定。** LLM を呼ぶ評価は、eval-budget の人の回答と、実行直前の具体的な呼び出し数・費用・quota 見積りの受信箱決定の後だけ走らせる。
codex-oauth と opencode-go は quota の実測が無いので、10 run の dry-run で usage endpoint の前後差から 1 run の消費を測り、それを見積りの材料にする。
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
最後に根拠なしの候補（辞書順）へ落とす。各段の除外理由を trace に残す。全候補が落ちたら fail closed（先行 ADR と同じ）。別 source へ黙って
落とすことはしない（落とした事実を event に残す）。

**根拠の無い genre の既定。** 根拠ありの候補が 1 つも無い genre×harness は、先行 ADR の暫定既定（`(source_id, model_id)` の辞書順）を使う。
ただし genre ごとに人が `[routing.defaults.<genre>] source = …, model = …` を明示できる（conversation の opus-5-5 のように人の決定事項）。
明示は「人の既定」として trace に記録する。人の既定が効くのは `shadow` 段の genre×harness だけで、`enforce` 段の genre×harness に
人の既定があれば config error にする（根拠表と人の既定の二重の経路を作らない。model を固定したい task は `model` 欄を使う）。

**探索。** enforce 中の genre では ε=0.1 の確率で、同等集合の中から選択規則と違う候補を選ぶ（乱数の種は run_id から決定的に導く。再現可能）。
探索の有無と propensity を `routing_decided` に記録し、以後のオフライン評価（DR）の材料にする。探索は同等集合の外には出ない（劣る候補を本番に出さない）。

### D3. shadow から enforce への段階と止める条件

段階は genre×harness ごとに独立に進める。状態は DB の表 `routing_stage` に `(genre, harness_id, stage, since, evidence_version)` で持ち、
config には書かない（daemon の決定で戻すため）。

| 段階 | 選択 | 記録 | 進む条件 |
|---|---|---|---|
| `shadow` | 先行 ADR の暫定既定（辞書順）または人の既定 | 新 selector が選んだはずの候補・同等集合・除外理由を `routing_decided` に記録 | D1 の「根拠が立つ」を満たし、人が受信箱で enforce を承認 |
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
| `[model_routing]` の `mode`・`models`・`deployments`・`policies`・`estimator`・`enforce_routes`・`subscription_windows`・`resource_groups`・`retry`・`escalation`・`shadow`・`context_safety_margin` | 削除。config にあれば読み込みエラーにし、新しい `[routing]`（`defaults.<genre>`、`exploration_epsilon`、`guardrail` 窓と閾値、評価専用予約枠）を案内する | `crates/celeris/src/config/model_routing.rs`、`crates/celeris/src/config/model_catalog.rs` |
| `HeuristicEstimator`・estimator sidecar・optimizer・profiles の `quality.index` | 削除（根拠表に置き換える） | `crates/task-core/src/model_router/{estimator.rs,estimator/,optimizer.rs,profiles.rs,policy.rs}`、`crates/celeris/src/daemon/routing_sidecar*.rs` |
| 旧 shadow（`dispatch-enforce-heuristic-v1`、decision shadow・実行 shadow・proxy shadow・`routing_shadow_reservations`） | 削除。新 shadow は D3 の `shadow` 段で `routing_decided` に記録する（別の event 型を増やさない） | `crates/celeris/src/daemon/routing_shadow*.rs`、`crates/task-dispatch/src/dispatcher/routing_shadow.rs`、`crates/task-core/src/model_router/shadow*`、`crates/task-core/src/store/routing_shadow*.rs` |
| `routing_enforce`（proxy 側の enforce 経路） | 削除（選択は dispatcher の `RoutePolicy` の 1 点） | `crates/task-dispatch/src/dispatcher/routing_enforce.rs`、`crates/llm-proxy/src/selection.rs` の lane 優先規則 |
| 手動割り当て `model_assignments`（web `/models` の role/tier slot、API、CoS ops） | 削除。人が model を固定したいときは task の `model` 欄（先行 ADR）か `[routing.defaults.<genre>]` | `web/features/ops/{model-assignments,model-role-editor,models-screen}*`、`crates/task-api/src/handlers.rs` の assignment handler、`crates/task-api/src/cos/ops/admin.rs` の登録、schema |
| lane（frontier / standard / cheap） | **廃止しない**。task の予算・方針ラベルとして残し、D2 の quota 窓の選び方にだけ使う。model・source・harness の別名にしない | `crates/task-core` の `Tier` 型は残す |
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

**境界。** runner → selector は DB の `routing_evidence` だけで結ぶ（runner が selector の型に依存しない。selector が runner を呼ばない）。
両方が共有するのは鍵の型 `(genre, harness_id, harness_version, source_id, model_id)` と区間の型だけで、`task-core` に置く。

**試験。** selector は根拠表を固定した単体試験（同等集合・quota 同点・fallback・探索の決定性・guardrail の各停止条件）。runner は manifest の
凍結・held-out 分割・集計（既知の表から区間と McNemar の値が合う）を単体試験し、評価 run の起動は daemon 試験で stub harness に対して
1 問 × 2 試行を回す。外部ネットワークに出ない。CPU を焼かない。

### D6. genre ごとの適用

調査報告 §1c の当てはめを採る。

| genre（harness） | 本 task での扱い |
|---|---|
| coding（claude-code / codex / acp・pi） | 評価して根拠を立て、enforce の対象にする。pilot 60 問 × 2 試行、本番根拠 150 問 |
| reviewer | judge の較正対象（本番ログで check との一致率）。model 選択は「被評価と別系」の固定規則 |
| literature / web-research / planner / data-analysis / writing / knowledge-curation | 根拠は立てない。暫定既定（辞書順）または人の既定。本番で完了率・時間・失敗率を guardrail として記録だけする |
| conversation（CoS）・browser・langmem | 選択点が無いか人の決定事項。評価 run を使わない。conversation は `[routing.defaults.conversation]` に人の既定を書く |

## Consequences

- source・model の選択は 1 点（`RoutePolicy`）・1 入力（根拠表）・1 段階表（`routing_stage`）に定まる。tier binding・手動割り当て・heuristic estimator・
  旧 shadow は無くなり、config の `[model_routing]` は読み込みエラーになる。本番 config の書き換えは進捗 file の「本番への移行」節に書く。
- 根拠が立つまで coding 以外は辞書順か人の既定で動く。これは現状（tier 固定）より良くも悪くもない決定的な既定で、根拠を増やす経路（評価集合と judge 較正）が残る。
- 評価は費用と quota を使う。走らせるのは eval-budget の人の回答と実行直前の見積りの承認の後だけ。
- `enforce` へ進むのは人、戻すのは daemon。人の承認なしに本番の経路が変わることはない。
- events は消さない。旧 payload の `routing_decided` と `routing_shadow_recorded` は読み取り API で `legacy` として表示する。

## 未解決（evaluate・land 段で決める）

- pilot の model 集合: fable-5-1・gpt-6-astra は費用 Pareto で支配される見込みなので pilot から外す（調査報告 §2）。pilot の結果で差が出なければ評価しない。
- codex-oauth・opencode-go の 1 run の quota 消費は dry-run で測るまで不明。
- harness_version の major の決め方（claude-code・codex は semver、acp・pi は command の version 文字列）は eval-harness で確定する。
