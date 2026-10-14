---
title: "LLM source・model の routing を評価に基づく選択に作り直す"
tasks: [01M4M8D4AQHBC2KG3H1BPJ7Z0J]
status: done
completed: 2026-10-14
updated: 2026-10-14
---

# LLM source・model の routing を評価に基づく選択に作り直す（2026-10-11）

人の依頼（2026-10-11 運用セッション）: 既存の routing の評価方法を調べ直し、LLM 評価の知見に基づいて本番で実際に効く選択にする。
source・model の選択と、task の内容による harness の選択は別々にする（harness の選択は先行 task 01M4M8D4AP の
ADR `2026-10-11-llm-source-harness-selection.md`〈branch `celeris/01M4M8D4AP4YY4F9TV09HG30SJ`、main 取り込み前のためリンクにしない〉に従い、ここでは変えない）。

- ADR: [2026-10-11-llm-source-model-routing-evaluation.md](../adr/2026-10-11-llm-source-model-routing-evaluation.md)
- 調査報告: task の artifacts `routing-evaluation-survey.md`（`/local/celeris/data/workspaces/01M4M8D4AQHBC2KG3H1BPJ7Z0J/artifacts/`。repo には置かない）
- unit ごとの進捗: [2026-10-11-llm-source-model-routing/](2026-10-11-llm-source-model-routing/)

## 現在地

| 段 | unit | 状態 |
|---|---|---|
| survey | survey-code / survey-data / survey-lit | done |
| design | survey-report | done |
| design | adr | done |
| design | adr-revise / adr-features | done（2026-10-11 人の確認を反映） |
| build | eval-harness / selector | eval-harness done（2026-10-12）、selector done（2026-10-13）、親 branch への統合 done（2026-10-13、下の「統合」） |
| evaluate | eval-run | done（2026-10-14、人の承認の後に投入。結果は [eval-results.md](2026-10-11-llm-source-model-routing/eval-results.md)） |
| land | rollout | done（2026-10-14、下の「本番への移行」。コードは変えていない） |
| land | verify | done（2026-10-14、下の「証拠」の最終検証） |

## 設計の要約

- **評価**: harness を固定し、その harness が話せる source×model を、本番の done WU から作った問題集合（開発 〜10/04 / held-out 10/05〜）に
  paired で問題ごと 2 試行当てる。主指標は決定的 check の pass^1、cell ごとに Wilson 95% 区間、比較は paired McNemar。
  根拠基準は通常 30 問×2、高価 model 12 問×1。k=1 は区間が広く、明確な優越がなくても区間重複候補を低確度の同等集合に残す。費用は定価換算と quota 窓の消費率の両方。
- **選択**: 先行 ADR の `RoutePolicy` を根拠表 `routing_evidence`（鍵 genre・harness・harness 版・source・model）による決定的規則に差し替える。
  区間下限が最大の候補と区間が重なる「同等集合」の中で quota 消費が最小の候補を選ぶ。fallback も根拠あり候補の順位後は `route_defaults` を使う。
  根拠の無い genre は harness×lane の人が指定した `route_defaults` 優先列。enforce 中は ε=0.1 の決定的な探索（同等集合の中だけ）で propensity を記録する。
- **段階**: genre×harness ごとに `shadow` → `enforce`。進むのは人の承認だけ、戻すのは daemon の決定的判定だけ（直近 50 run の合格率区間・失敗率・壁時計・quota・根拠表の版）。
- **legacy**: tier binding・`[model_routing]` 全欄・HeuristicEstimator と sidecar・旧 shadow・`routing_enforce`・手動 `model_assignments` を selector の変更で消す。
  lane は方針ラベルとして残す。events は消さない。前回 branch は取り込まない。
- **実装**: 評価 runner は crate `routing-eval`、選択器は `task-core/src/routing/`。両者は DB の `routing_evidence` だけで結ぶ。
- **genre**: 対象 7 model を coding の claude-code / codex で評価する。高価な2 modelは k=1 の小部分集合。acp/pi の qwen は既定候補、opencode-go は評価対象外。reviewer は judge 較正。他は harness×lane `route_defaults` で動かし guardrail を記録する。

## 統合（2026-10-13、子 task selector と main を親 branch へ手で統合）

自動統合が 30 file の競合（migration 番号の衝突を含む）で止まったので手で統合した。順序は selector（`celeris/01M4Q53EHAF7BS9V3QMM1WY352`、main の 101aced5 を取り込み済み）→ main（5283bcef）。どちらも `--no-ff`。

**重複の撤去（破壊的変更の方針。互換の分岐は残さない）**:

- `routing_evidence` 表を親の eval-harness（migration 0071 + 0073）と selector（0074）が別々に作っていた。selector の 0074 を残し、評価 runner の欄（bootstrap 区間・`low_confidence`・定価／時間の NULL・CHECK）は新しい migration 0077 で表を作り直して足した。0071・0073 の file は消した。
- 行の型も `task_core::routing_evidence::EvidenceRow`（runner）と `task_core::route_select::EvidenceRow`（selector）の 2 つあった。`task_core::routing_evidence` の 1 つにし（表の型。ADR D5 の「runner → selector は表だけで結ぶ」）、`route_select` 側の定義を消した。`quota_per_run` は窓→率の map（表では JSON object）、`n`・`pass` は u64。
- store の API も 2 組あった。selector の `replace_routing_evidence` / `latest_routing_evidence` を残し、runner の `insert_routing_evidence` / `list_routing_evidence` を消した（`aggregate --write-evidence` は前者で書く）。
- `route_records`（親 0069）は selector が main 取り込みで 0073 に振り直した同じ内容なので、selector 側を残した。
- 定価・時間の無い cell は `None`。選択の同点比較では後に回し、guardrail の壁時計は見ない。API の `RoutingEvidenceRowView`・`EvidenceSnapshot`・`RouteScore.expected_cost_usd` は null を取る（schema・生成型を再生成、web の routing 画面は「—」を出す）。

**migration の振り直し**（main の 0067・0070・0071・0072 と selector の 0073〜0075 は番号を変えない。本番 DB は 70）:

| 統合前（親 branch） | 統合後 |
|---|---|
| 0069_route_records | 削除（selector の 0073_route_records と同じ） |
| 0071_routing_evidence | 削除（selector の 0074_routing_evidence_stage） |
| 0072_eval_trials | 0076_eval_trials |
| 0073_routing_evidence_bootstrap | 削除し 0077_routing_evidence_eval_columns に統合 |

`SCHEMA_VERSION` = 77、`RESERVED_VERSIONS` = [38, 39, 40, 68, 69]（main と同じ）。

**provider 層の撤去への追従（評価 run）**: eval-dispatch は `[[providers]]`・adapter 表・`source_state_of`・`routing_enforce` を使っていたが、selector／main で消えた。評価 run の経路は宣言（`[harnesses]`・`[llm_sources]`）から引き（id は `-`／`_`・大文字小文字を区別しない）、account は本番と同じ `admit_source`（手元の評価 run を合算）で選び、worker に `SelectedLlmRoute` と `Task.harness_id` を渡す。宣言に無い経路の試行は `error`。投入 API（`POST /routing-eval/trials`）の照合先は `HarnessesReader::source_models` と harness 一覧・`[[genres]]` に替えた（routing catalog は無い）。ADR に付記 2026-10-13 を足した。

**main（5283bcef）の取り込み**: selector が取り込んだ 101aced5 以降の main（class 別の route 候補 `classes`・proxy route の wire model・旧 routing event の読み取り試験）と競合した 6 file を解いた。
- 旧 `model_router/policy.rs`・`model_router/tests.rs`（main が `RoutingMode::Legacy` を足した）は selector が消したので消したまま。旧 `routing_decided` event（`optimizer.mode = "legacy"`）は `RoutingRecord` に `optimizer` 欄が無く未知欄として読み飛ばされるので読める。main の試験 `store::tests::legacy_routing_decided_event_loads_from_store` は `RoutingDecided` として読めることを見る形に直し、`celeris` の `legacy_event_startup` はそのまま通す。
- 優先列は CoS → class の列 → harness×lane の列 → harness 全体の列（ADR 付記 2026-10-13 の 6、試験 `route_priority_class_list_wins_over_the_lane_column`）。`RoutePick.wire_model_id` は selector の選択と評価 run の経路の両方で作る。
- main 側に残っていた `DefaultRoutePolicy`／`RoutePolicy` は selector の根拠表選択に置き換わっているので持ち込まない。
- 評価 run の結合試験（`celeris/tests/routing_eval_run.rs`）は main の db_guard（worker run から DB のディレクトリに新しい項目を作れない）に合わせ、stub の記録を別ディレクトリに置いた。

## 証拠

- 最終検証（2026-10-14、verify unit、base b285a5a3、`CARGO_TARGET_DIR` はローカル）: `bash scripts/dev/test-parallel.sh` exit 0（nextest 0.9.146・jobs 8、passed 5085・failed 0・ignored 13、nextest_exit 0・doctest_exit 0）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- 統合（2026-10-13、`CARGO_TARGET_DIR` はローカル）: `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace -- -D warnings` exit 0、`TMPDIR=/tmp bash scripts/dev/test-parallel.sh` exit 0（nextest 5079 passed・12 skipped、doctest 含め passed 5082・failed 0）、`sh scripts/dev/check-adr-numbers.sh` exit 0、`sh scripts/dev/check-doc-links.sh` exit 0、web `pnpm typecheck` exit 0・`vitest run features/ops/routing-screen.test.tsx` 1 passed。migration 番号は 0001〜0077 で重複なし（欠番は `RESERVED_VERSIONS` の 38・39・40・68・69）。
- `sh scripts/dev/progress-index.sh --check`: exit 0（本 file を含む）
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/ gui/`: exit 0（adr unit はコードを変えていない）

## 未解決

- **根拠表の鍵の綴り（enforce の前提、未修正）**: 評価 runner の候補表（`crates/routing-eval/src/candidates.rs`、main も同じ）は `claude-oauth`／`claude-code`／`codex-oauth` の綴りで `routing_evidence` に書く。選択器（`route_select.rs` の `latest_routing_evidence(genre, harness)`）・段階 API の根拠確認・guardrail は宣言の綴り（`claude_code`・`claude_oauth`）で完全一致に引くので、このまま `--write-evidence` しても enforce で根拠行が見つからず、段階 API は `routing_evidence_insufficient`（409）を返す。人の指示（2026-10-13「候補表と書き込みを宣言の綴りに揃える」）がまだ実装されていない。follow-up task に出した。
- **本番 run の outcome 記録が無い**: `route_outcomes` に行を書くのは評価 trial（`dispatcher/eval_trials.rs`）だけで、本番の shadow 判断 124 件（2026-10-14 時点、run は completed 75・failed 25 ほか）は outcome 0 件。task-features ADR D6 の「WU／run の終わりに outcome」が本番経路で未実装で、`celerisctl routing export --stage shadow` は outcome が空の行になる。guardrail は `routing_outcome_recorded` event（同日 412 件）を使うので影響しない。follow-up task に出した。
- class の列と lane の列の優先（class が勝つ）は統合時の判断。本番の `classes.coding = [opus]` があるかぎり coding class では lane の既定が効かない（下の「本番への移行」§3）。
- haiku の 1 問は yielded 3 回連続で有効な結果が無く根拠が立たない。fable は今回 0 問（次の週窓で別承認）。
- `quota_per_run` が全行 `{}`（評価 trial の run に quota 実測 event が無い）。このため同等集合の中の順位は quota を見ず、定価 USD → 壁時計で決まる（codex は USD も無く壁時計だけ）。
- research・planner の問題集合と reviewer の judge 較正は本 task の範囲外（次の task）。

## 提案

- 評価試行の run にも quota 実測 event を残し、`quota_per_run` を根拠表に入れる（codex の同等集合の順位が壁時計だけで決まる問題を解く）。
- 根拠表 API（`GET /routing/evidence`）に bootstrap 区間と `low_confidence` を出す。
- yielded が続く問題（予算内に終わらない）は manifest 凍結時か集計で印を付け、扱い（除外／不合格）を ADR に決める。
- research 2 genre（literature / web-research）と planner の評価集合と reviewer の judge 較正を、coding の enforce が安定した後の別 task にする。
- `routing_decided` に候補集合と propensity が揃った enforce 開始後の本番ログを、DR でオフライン評価の補助根拠に使う（本番 outcome の記録が前提）。

## 本番への移行

実行は運用セッション（人）。worker は本番 config・DB・daemon に触れていない。以下の現状は 2026-10-14 に読み取りで確かめた値。

**本番の現状**: release `64158056`（main の HEAD。selector・評価 runner・error 除外の集計修正 `da02a2dc` を含む）、DB の migration 78、
`[model_routing]` 欄は config に無い（selector の変更で読み込みエラーになる欄なので、残っていないことだけ確かめればよい）。
`routing_stage` は 0 行（全 genre×harness が既定の `shadow`）、`routing_evidence` は 0 行。`route_decisions` は shadow 124 件（selected は opus 88・sonnet 36）、eval 307 件。
`[route_defaults]` は harness 全体の列と `[route_defaults.claude_code.classes.coding] = [opus]`（2026-10-13 人の決定）だけで、lane の列と `[route_selection]` は無い（既定値 ε 0.1・窓 50・quota floor 0.1 で動く）。

### 1. 根拠が立った genre×harness と、enforce したときの選択

根拠表（[eval-results.md](2026-10-11-llm-source-model-routing/eval-results.md)、evidence_version `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7-2026-10-14`）で D1 の「根拠が立つ」（n ≥ 24 かつ Wilson 下限 ≥ 0.5、高価 model は 12 問×1）を満たすのは genre `coding` の 2 組だけ。選択器の規則（D2: 下限最大と区間が重なる同等集合 → quota 圧 → 定価 USD → 壁時計 → 辞書順）を本番の値で当てると:

| genre×harness | 根拠あり候補（下限） | 同等集合 | 規則が選ぶもの（ε=0.1 の探索以外） | 推奨 |
|---|---|---|---|---|
| coding×claude_code | opus 0.862・sonnet 0.862 | opus・sonnet | **sonnet**（`quota_per_run` が空で圧は同点、定価 USD 中央値 0.270 < 0.757） | enforce する（§2、class 列の変更とともに） |
| coding×codex | sol 0.744・astra 0.646・luna 0.543 | sol・astra・luna（上限が 0.744 以上） | **luna**（quota・USD が無く、壁時計 76 s が最短） | **shadow のまま**。held-out では sol − luna が 0.385（共通 13 問）で、区間の重なりだけで luna が同等扱いになる。quota 実測が無いまま壁時計で順位が決まるのは規則の想定外 |

haiku（23/24 で有効問題数不足）・fable（0 問）・qwen・opencode-go は根拠表に行が無いか立たないので、enforce の候補に入れない
（選択器は行の有無だけを見て「根拠が立つ」を再判定しないため、立たない行のある model を enforce 中の候補列に足すと同等集合に入りうる。haiku を claude_code の列に足さない）。

### 2. enforce の前提（この順に満たす）

1. **根拠表の鍵を宣言の綴りにする修正が main に入り release されている**（follow-up task。上の「未解決」の 1 番目）。入る前に `--write-evidence` すると鍵が `claude-code` で入り、段階 API が 409 を返す。
2. 修正後の runner で根拠表を本番 DB に書く（人。[eval-results.md](2026-10-11-llm-source-model-routing/eval-results.md) の「本番の根拠表に書く人の手順」。error 除外の修正はすでに main にある）。
   確認: `sqlite3 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' "select harness_id,source_id,model_id,n,pass,round(pass_lower,3) from routing_evidence where genre='coding' order by 1,2,3"` が 6 行で、harness_id が `claude_code`／`codex`、source_id が `claude_oauth`／`codex_oauth`。集計日で evidence_version の日付部分が変わる（以下 `<EV>`）。
3. live の harness 版の major が根拠表と同じ: claude-code は `2`、codex は `0`（違えば guardrail が即 shadow に戻す。`claude --version`・`codex --version` で確かめる）。

### 3. config の差分（enforce 用。coding×claude_code）

`~/.config/celeris/config.toml` の `[route_defaults.claude_code.classes.coding]` だけを変える。class の列があると lane の列も harness 全体の列も見ないので、enforce で根拠表が選べるのはこの列の中だけになる。今の `[opus]` のままでは enforce しても opus しか選べない（guardrail が付くだけ）。

```diff
 # class 列は harness 既定の代わりに使い、既定へは落ちない（Opus が不適格なら claude_code を飛ばして Codex）。
 [route_defaults.claude_code.classes.coding]
-candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }]
+# 2026-10-14 根拠表（coding×claude_code、opus・sonnet とも 24/24）で enforce。shadow 中は先頭の opus（10-13 の人の決定のまま）。
+candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }, { source = "claude_oauth", model = "claude-sonnet-5-5" }]
```

- shadow の間は列の先頭（opus）を使うので、通常時の挙動は今と同じ。違いは opus が quota・cooldown で入れないとき codex harness へ移らず sonnet を使う点。
- enforce に進めると規則は sonnet を選び、run_id から決まる 10% で opus を選ぶ（propensity は `routing_decided` に残る）。「coding は opus」という 10-13 の人の決定を評価結果で置き換えることになるので、運用セッションで人が採否を決める。採らないなら差分を入れず、coding×claude_code も shadow のままにする。
- codex は差分なし（shadow のまま。今の `[route_defaults.codex]` の順 sol → astra → luna を使う）。

### 4. 根拠の無い間の既定（route_defaults と lane ごとの人の既定、config 差分）

根拠が無いか shadow の genre×harness は `route_defaults` の順で決まる（辞書順 fallback は無い）。lane（frontier / standard / cheap）ごとの人の既定は `[route_defaults.<harness>.lanes.<lane>]`。lane の列が無ければ harness 全体の列に戻る。standard は harness 全体の列をそのまま使う。

```diff
 [route_defaults.claude_code]
 candidates = [{ source = "claude_oauth", model = "claude-sonnet-5-5" }, { source = "claude_oauth", model = "claude-opus-5-5" }, { source = "claude_oauth", model = "claude-fable-5-1" }]
+
+# 2026-10-14 lane ごとの人の既定。cheap は sonnet だけ（haiku は評価で定価 USD 中央値 0.349 > sonnet 0.268・壁時計も長く、安くならない）。
+[route_defaults.claude_code.lanes.cheap]
+candidates = [{ source = "claude_oauth", model = "claude-sonnet-5-5" }]
+
+[route_defaults.claude_code.lanes.frontier]
+candidates = [{ source = "claude_oauth", model = "claude-opus-5-5" }, { source = "claude_oauth", model = "claude-fable-5-1" }, { source = "claude_oauth", model = "claude-sonnet-5-5" }]
 
 [route_defaults.codex]
 candidates = [{ source = "codex_oauth", model = "gpt-6.1-sol" }, { source = "codex_oauth", model = "gpt-6-astra" }, { source = "codex_oauth", model = "gpt-6-luna" }]
+
+# 2026-10-14 cheap は luna を先頭（今の運用どおり）。luna 21/29・下限 0.543 で根拠は立つが sol より明確に低いので、2 番目に sol を置く。
+[route_defaults.codex.lanes.cheap]
+candidates = [{ source = "codex_oauth", model = "gpt-6-luna" }, { source = "codex_oauth", model = "gpt-6.1-sol" }]
+
+# astra は quota が重い。frontier だけで先頭にする。
+[route_defaults.codex.lanes.frontier]
+candidates = [{ source = "codex_oauth", model = "gpt-6-astra" }, { source = "codex_oauth", model = "gpt-6.1-sol" }]
+
+# 選択器の設定（値は既定と同じ。明示して運用で見えるようにする）。評価 run を流さない間は予約枠を置かない。
+[route_selection]
+exploration_epsilon = 0.1
+guardrail_window = 50
+guardrail_quota_floor = 0.1
```

- pi・opencode・local-deep-research・paperqa・langmem の列は変えない（qwen を先頭に置く今の列が既定。opencode-go は評価対象外で pi の既定のまま）。
- coding class の task は class 列が勝つので lane の列は効かない（統合時の判断、上の「未解決」）。lane の列が効くのは class の列が無い仕事（planner・review 以外の claude_code／codex の run で class 未指定のもの）。
- lane の列は harness 全体の列と同じ検証（重複・未宣言・互換）を読み込み時に通るので、書き間違いは daemon が起動しない形で出る。

### 5. 切替手順（運用セッション）

1. `cp ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-$(date +%Y%m%d)-routing` で控えを取り、§4 の差分（と、人が採るなら §3）を入れる。
2. daemon を再起動して読み込む（いつもの selfdeploy の再起動手順。release の差し替えは要らない）。起動しなければ journal の config エラーを見て控えに戻す。
3. shadow のまま 1 日程度動かし、`route_decisions` の shadow 行で lane ごとに既定どおり選ばれていることを確かめる:
   `sqlite3 'file:/local/celeris/data/db/celeris.sqlite3?mode=ro' "select json_extract(output_json,'$.selected.model_id'),count(*) from route_decisions where stage='shadow' and created_at>'<切替時刻>' group by 1"`
4. §2 の前提が揃ったら、coding×claude_code を enforce にする（人の決定。CoS は代行できない `human_only`）:
   `curl -sS -X POST -H "Authorization: Bearer $(cat ~/.config/celeris/api.token)" -H 'Content-Type: application/json' http://127.0.0.1:7710/api/v1/routing/stages -d '{"task_id":"01M4M8D4AQHBC2KG3H1BPJ7Z0J","genre":"coding","harness_id":"claude_code","stage":"enforce","reason":"evidence <EV>: opus 24/24, sonnet 24/24 (Wilson lower 0.862)"}'`
   409 `routing_evidence_insufficient` なら §2 の 1・2 が済んでいない。

### 6. 確認方法

- 段階: `celerisctl routing stage list --db /local/celeris/data/db/celeris.sqlite3 --genre coding --json` で `claude_code` が `enforce`・`evidence_version` が `<EV>`・`last_change.actor = human`。
- 選択: enforce 後の `route_decisions`（`stage='enforce'`）で selected が sonnet ≒ 90%・opus ≒ 10%、`output_json` に同等集合と propensity（0.9／0.1）がある。
- guardrail: `routing_stage_changed` event が出ていないこと（`select ts,json from events where json like '%"routing_stage_changed"%' order by id desc limit 5`）と、受信箱に段階の戻しの通知が無いこと。
- quota: `/api/v1/llm/accounts` の claude account の 7d 使用率が切替前の週と比べて増えていない（sonnet に寄るので減るはず）。

### 7. 止める条件と戻し方

- **daemon が自動で shadow に戻す**（D3。窓は直近 50 run）: 合格率の Wilson 上限 < 選ばれた候補の `pass_lower`（sonnet なら 0.862）、失敗率の区間下限 > shadow 期の 2 倍、壁時計中央値 > 根拠表の 2 倍（sonnet 103 s → 206 s）、harness 版の major が根拠表と違う。quota の残量 10% 未満は段階を戻さず fallback と受信箱の通知だけ。戻ると `routing_stage_changed` と受信箱の通知が出る。自動で enforce に戻ることは無い。
- **人が止める条件**: 戻しの通知が来た、review の差し戻しや再試行が目に見えて増えた、claude の 7d 使用率が想定外に増えた、のどれか。
- **戻し方**: 段階は `POST /api/v1/routing/stages` に `"stage":"shadow"`（理由つき）。config は控えへ `cp` して daemon を再起動（§3 の class 列を戻すと coding は再び opus だけ）。DB の行・events は消さない（追記のみ。根拠表は次の評価の版で置き換わる）。

### 8. 根拠が立たない genre と、何を足せば立つか

| genre×harness | 既定に置く理由 | 根拠を立てるのに足すもの |
|---|---|---|
| coding×codex | 根拠は立つが、同等集合が sol・astra・luna の 3 つに広がり規則が壁時計で luna を選ぶ。held-out で sol と luna の差 0.385 | (a) 評価 trial の run に quota 実測 event を残して `quota_per_run` を入れる、(b) sol・luna の問題を増やす。合格率が今のまま（0.90 と 0.72）なら Wilson 区間は約 90 問で分かれる（60 問: sol 下限 0.799・luna 上限 0.815、90 問: 0.821・0.804）。足すまで shadow |
| coding×claude_code の haiku | 23/24（1 問 yielded 3 回） | 予算を上げてその 1 問を 1 試行するか、yielded を不合格に数える人の判断（22/24、下限 0.742） |
| coding の fable | 0 問（quota が重く人の承認で今回は回さない） | 次の週窓で 12 問×1 を別承認 |
| coding の qwen（opencode・pi）・opencode-go | 人の決定で評価対象外（2026-10-11） | 対象に入れる人の決定と、同じ manifest での試行 |
| reviewer | judge の較正対象で model 選択は「被評価と別系」の固定規則 | 本番の review と決定的 check の一致率の集計（次の task） |
| literature / web-research / planner / data-analysis / writing / knowledge-curation | 問題集合が無い（決定的 check で採点できる done WU が足りない） | 人が検めた問いと決定的な形式 check の評価集合（次の task）。それまで guardrail の記録だけ |
| conversation・browser・langmem | 選択点が無いか人の決定事項 | 対象外 |

### 9. 学習データ export の取り方

- 本番の判断（shadow・enforce）と評価の判断を同じ形（`celeris.route-training/1`、1 行 1 判断、`decision_id` 順で決定的）で出す。DB は読み取り専用で開く:
  `celerisctl routing export --db /local/celeris/data/db/celeris.sqlite3 --out ~/routing-export/$(date +%Y%m%d).jsonl [--since 2026-10-14T00:00:00Z] [--stage shadow|enforce|eval] [--text brief|none]`
- `--text none` は自然言語の brief を落として hash と aux だけ出す（repo や外へ出すときに使う）。held-out の問題文を含むので、評価分（`--stage eval`）は repo に置かない。
- 評価の結果つきの行（`celeris.routing-eval-result/1`、271 行）は評価 runner の `export` の出力（eval-results.md の path）。
- 今の制約: 本番の判断には outcome（label）がまだ付かない（上の「未解決」）。結果つきで学習に使えるのは eval の 271 行だけ。本番分は outcome の記録が入ってから、その後の判断を出す（過去の判断は backfill しない、task-features ADR D6）。
