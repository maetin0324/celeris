---
tasks: [01M5H8CYQHDBSCB3VQRB60K8PJ]
title: harness の残量選択の復活、Jev の lane を選択に効かせる規則、完了単位の根拠表
---
# harness の残量選択の復活、lane 優先の model 選択、完了単位の根拠表

- 日付: 2026-10-22
- 状態: Accepted（人の決定 2026-10-22。operator の調査に基づく）
- 置換・付記: [ADR-0049](0049-portable-providers-and-codex-usage.md) D2（残量スコアの比較）を**復活**（付記）、[2026-10-11-llm-source-harness-selection](2026-10-11-llm-source-harness-selection.md) の「候補 harness を宣言順に試す」を**有効化したクラスだけ置換**（付記）、[2026-10-11-llm-source-model-routing-evaluation](2026-10-11-llm-source-model-routing-evaluation.md) D2 の根拠表の作り方（run 単位の評価）を**置換**（付記）。

## 背景

1. ADR-0049 D2 は「アカウントプール同士はアダプタをまたいで残量スコアを比較する」だった。2026-10-13 の source 移行で harness は `execution_class_defaults` の宣言順に試し、最初に通ったものを使う形になった。残量比較を廃止する決定は無かったが、実装から消えた。結果、codex は claude の席が満席のときだけ使われた（10-15 以降の codex 108 run は全て `harness_rejections=AccountConcurrency`）。`QuotaSnapshot.source_remaining` に値を入れる本番コードも無く、`routing_evidence.quota_per_run_json` は全行 `{}` だった。
2. claude_code の enforce は `pass_upper >= max(pass_lower)` の候補を同等とし、残量の圧迫度→定価→時間で並べる。lane は順序に入らず、frontier lane でも sonnet が選ばれた。根拠表は 10-14 の評価 1 版のまま、本番の結果から更新されなかった。

## 決定

### D1. harness の試行順は残量スコアで並べ替える（クラスごとに有効化）

- **スコア**。harness h の候補が使う source の account それぞれに、ADR-0024 D3 の `min(h_5h, h_7d, h_1m)`（`h_7d = min(1, (1-u_7d)/max(t_7d, 0.1))`、すなわち 7d リセットまでの残り時間で正規化）を求め（席の使用数の減点は入れない。席は admission の仕事）、除外された account は 0 とする。**harness のスコア = その harness の候補 source の account スコアの最大**。observation の無い account は 1.0。account プールを持たない source（API 直）はスコア不明。
- **並べ替え**。有効なクラスで、全ての候補 harness のスコアが分かるときだけ行う。最大スコアから許容幅 `tolerance`（既定 0.15）以内の harness は「同等」とし、同等の中では宣言順を保つ。同等集合の先頭から順に取り出し、残りで繰り返す（決定的）。スコア不明の harness がある・クラスが無効・guardrail が止めているときは宣言順のまま。
- **有効化**。`[route_selection] harness_balance_classes = ["planner", "review"]`（`coding` も書ける）。**既定は空（全クラス無効）**。有効化は人が決める。品質差（codex sol の pass 下限 0.744、claude 0.862）に備え、初期は planner / review の想定。許容幅は `harness_balance_tolerance`。
- **記録**。`routing_decided` の `route_decision.harness_order`（`HarnessOrderRecord`）に、クラス・有効か・止まっているか・許容幅・宣言順・各 harness のスコア・結果の順・`switched`（結果の先頭が宣言順の先頭と違うか）を残す。
- **admission は従来のまま**。並べ替えは試行順だけを変える。席が埋まれば次の harness へ落ちる。

### D2. 切替の guardrail（悪化したら自動で戻す）

完了単位の記録（D6）に、その単位の run が `switched` だったクラスの一覧を残す。daemon の定期処理が、各有効クラスについて、直近 `guardrail_window` 件の完了単位のうち switched と非 switched の「悪い率」を比べる。悪い = 失敗、または検査失敗・review 差し戻しが 1 回以上。switched が `harness_balance_guard_min`（既定 10）件以上で、`switched の悪い率 − 非 switched の悪い率 > harness_balance_guard_margin`（既定 0.15）なら、そのクラスを `harness_balance_trips` に記録して**自動で止める**（D1 は宣言順に戻る。受信箱に通知）。再開は人が `celerisctl routing balance-reset <class>`。自動で再開しない。LLM は呼ばない。

### D3. frontier lane は規則で frontier の model

Jev（decision-assisted）と fixed 規則が決めた lane が `frontier` のとき、候補の先頭は**その lane の `route_defaults`**（`lane_route_defaults[(harness, frontier)]`）とし、根拠表の同等判定・クラスの候補列より優先する。lane の列の後ろに、従来の候補列が続く（admission の fallback と ε の対象）。それ以外の lane は今の選択のまま（根拠表の同等判定→順序）。

### D4. ε 探索は frontier lane の中に寄せる

ε 探索は frontier lane の決定（enforce）の中だけで行い、両 model の実績をそこで貯める。lane が frontier 以外のときは ε を 0 にする（lane が不明な決定は従来どおり）。propensity は従来の式（選ばれた確率）を、frontier lane の探索集合（lane 列 + 候補列）で計算する。

### D5. 根拠表は lane（層）ごと・完了単位

- `routing_evidence` の鍵に `lane` を足す。選択器は `row.lane == 決定の lane` の行だけを読む。旧い行（lane 空）はどの lane にも一致しない（run 単位の旧い根拠は選択に効かない。データは消さない）。
- 行の値は完了単位の集計から作る（D6）。`n`・`pass` = 完了した単位の数・成功した数（propensity の逆数で重み付けした有効数）。`wall_p90_s` = 完了までの経過時間 p90、`usd_per_pass`、`quota_cost_per_pass` = 成功 1 件あたりの quota 加重コスト、`quota_per_run` = 窓ごとの 1 単位あたり消費率。
- **指標**。主 = 「判定に通った task 1 件あたりの quota 加重コスト」（`quota_cost_per_pass`）。制約 = 完了率（Wilson 区間。`pass_upper >= max(pass_lower)` を同等の条件に維持）と経過時間 p90（同等集合の最小 p90 の 1.5 倍以内）。同等集合の順は quota_cost_per_pass → 圧迫度 → usd_per_pass → p90 → 鍵。値の無いものは後ろ。
- quota 加重: 窓の消費率に重み（7d・1 か月 = 1.0、5h と他 = 0.25）を掛けて足す。quota が測れない単位は quota_cost を欠測とし、`usd_per_pass` で比べる。
- **run 単位の合格率・中央値だけの根拠は消す**: `routing-eval aggregate --write-evidence`（評価 trial から根拠表を書く経路）を撤去する。`wall_median_s`/`usd_list_median` は `wall_p90_s`/`usd_per_pass` に置き換える。

### D6. 完了単位の記録

task / WorkUnit が終端（done / failed）になったとき、決定ログの投影（`record_task_decision_log`）の末尾で `completion_outcomes` に 1 行を冪等に upsert する。内容: 単位の種類と ID、genre、最後の worker run の harness・版・source・model、lane（決定時の最終 lane）、Jev の判定（specificity・confidence・lane・分類 status）、最初の決定の propensity、完了までの総 token（in/out/cache read/creation）、定価 USD、quota 消費（窓→率）、wall（最初の run の開始から終端まで）、試行数（worker run 数）、検査失敗数、review 差し戻し数、replan 数、timeout の有無、switched のクラス、Celeris の版（リリース `sha12`。`CELERIS_RELEASE`、無ければ `dev`）、成否。

### D7. 根拠表の更新

daemon の定期処理（6 時間ごと）が `completion_outcomes` の葉の単位（WU、および WU を持たない task）を (genre, harness, harness 版 major, lane, source, model) で畳み、propensity の逆数（上限 20）で重み付けして根拠表の新しい版（`completion-<日時>`）を書く。選択器は最新版を読む。

## 結果

- 有効化したクラスで、claude の残量が codex より少なければ codex が先になる。許容幅内なら宣言順。
- `source_remaining` は dispatcher の account book（観測値）から、`quota_per_run` は D7 から本番の経路で入る。
- 本番 config の変更は進捗の「本番への移行」節。
