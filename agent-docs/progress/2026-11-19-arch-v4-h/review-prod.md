---
title: アーキ v4 H review-prod — 昇格後の review と設計 review の回数・時間・費用を本番 DB で測り、C の見積もりと比べる
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# review-prod（WorkUnit）

A §7 C11（baseline.md）の query と、人が C に求めた「昇格の後の設計 review の回数と費用」を本番 DB で測った。決定 c11 の答え a（待たない）に従い、
今ある部分窓で測って日あたりに直した。数は [review-prod.tsv](review-prod.tsv)（列 `metric window value unit note`）にある。
本番 DB は `file:/local/celeris/data/db/celeris.sqlite3?mode=ro`（URI の `mode=ro` と `pragma query_only=1`）で開き、読むだけにした。
release の記録（`/local/celeris/state/logs/promote-*.log`、`/local/celeris/state/releases/`）も読むだけにした。crates/・scripts/・web/ は変えていない。

## 0. 結論

- C を含む release `f85df0209504`（main の件名「integrate wu/loc-fix (phase fix)」の commit）は **2026-11-18T12:18:40Z** に本番の active になった
  （`promote-20261118-121753.log`、昇格の開始 12:17:53Z、handoff done 12:18:40Z）。昇格の要約の「入る task」は C（01M7MZTKB29VF5TX7FCQ15XEMW）で、
  task に属さない commit に profile の件名「integrate 01M7MZTKB2 profile: [review] に境界の 6 class と class ごとの不変条件を足す …」がある。
- 昇格の後の窓は **0.987 日（23.7 時間）** しかない。設計 review の要求は 7 件・3 task（D・C の申し送り・F。どれもアーキ v4 の task で、意図して
  高 risk の path を触る）、設計 review の run は 7 件・$14.33。日あたり 7.09 件・$14.52。
- C の見積もり（+14〜+94 回・$28.66〜$192.42／20.9 日 = 日あたり +0.67〜+4.50 回・$1.37〜$9.21）との比較は、数え方で結論が変わる:
  - 見方 1（A の窓の日あたりとの差）: +3.55 回/日・+$8.65/日（20.9 日に直すと +74 回・$180.77）。見積もりの範囲の中で、上限に近い。
  - 見方 2（`declared:new_*` の無い要求 = C の規則で新たに High になったと推定する分）: 6 件・$12.43 → 6.08 回/日・$12.59/日（20.9 日で 127 回・$263.20）。見積もりの上限を超える。
  - どちらも 1 日・3 task の値で、見積もりの単位（commit）とも違う（要求は計画の単位で、差し戻し後の再要求を含む）。**比べられない（未達/要判断）**。
- reviewer run 全体（A §7 C11）は、昇格後の窓で 17 run、wall の中央値 434.5 s（p90 1,614.2 s）、cost_usd の中央値 $2.036（p90 $2.924）、合計 $26.57。
  日あたりは 17.2 run・$26.92 で、A の窓（21.6 run・$36.74）より少ない。窓が短く、仕事の量も違うので、増減とは書かない。
- 本番の判定経路で C の規則の設計 review が始まった証拠: 昇格の後の 7 件の要求の理由は、旧い形（`path_class:<class>+new_*`）でなく
  C の形（`path_class:<class>:<path>` と `invariant:<class>:<不変条件>`）で、7 件中 6 件は `new_*` を宣言していない（§5）。

## 1. 窓

| 名前 | 窓（UTC、半開区間） | 日数 | 使い方 |
|---|---|---|---|
| A | [2026-11-10T00:00, 2026-11-17T15:00) | 7.625 | A の窓の再現（A §5.4・G scope.md §4 と同じ） |
| after | [2026-11-17T15:00, 2026-11-19T12:00) | 1.875 | C11 の after の部分窓（決定 c11 の a）。G の部分窓（〜05:00）を run の時点まで延ばした |
| promo | [2026-11-18T12:18:40, 2026-11-19T12:00) | 0.987 | C を含む release の昇格の後 |
| pre_promo | [2026-11-17T15:00, 2026-11-18T12:18:40) | 0.888 | after のうち昇格の前 |
| old_rules | [2026-11-09T01:36:42, 2026-11-18T12:18:40) | 9.446 | 最初の `design_review_requested` から昇格まで（旧い判定の実績） |

- 窓の終わり 2026-11-19T12:00Z は、この run が query を走らせた時刻（12:0x Z）の直前の正時。query の時点で `finished_at` の無い reviewer run は 0 件だった。
- 同じ長さの窓（決定 c11 の b）[2026-11-17T15:00, 2026-11-25T06:00) は 2026-11-25T06:00Z の後でないと揃わない（§7）。
- 昇格の前の経緯: C の前提の task（PathClass に invariants の欄を足す）を含む release `b2fe0a5e920f` が 2026-11-18T03:14:53Z に昇格した。
  profile の 6 class は main に 2026-11-18T02:43Z に入った。pre_promo の設計 review 4 件（C 自身の task）は、新しい class（review-gate）に旧い判定（`+new_*` が要る）が
  当たった形で出ている。

## 2. query

reviewer run（A §7 C11 と同じ条件。窓は Python で時刻に直して比べた）:

```sql
select r.usage_json, r.metrics_json, r.started_at from runs r join tasks t on t.id = r.task_id
 where r.role = 'reviewer' and t.project_id = '01M2WTS3DKNZBSZ2JMVB4CZMBW' and r.finished_at is not null
-- started_at が窓の中の run の metrics_json.wall_ms と usage_json.cost_usd の中央値・p90（線形補間）・合計
```

設計 review（agent-platform の task の event。design_reviewed の review_run を runs で引く）:

```sql
select e.id, e.task_id, e.ts, e.json from events e join tasks t on t.id = e.task_id
 where t.project_id = '01M2WTS3DKNZBSZ2JMVB4CZMBW'
   and json_extract(e.json, '$.type') in ('design_review_requested', 'design_reviewed')
-- requested: ts が窓の中の件数・task 数、risk.reasons の種類。reviewed: review_run の run の wall_ms・cost_usd
```

再現コマンド（道具はこの WorkUnit の run の成果物 dir `…/wu/review-prod/artifacts/raw/` にある。使い捨て）:

```sh
ART=/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/review-prod/artifacts
python3 -I $ART/raw/review_prod.py /local/celeris/data/db/celeris.sqlite3 2026-11-19T12:00 2026-11-18T12:18:40 $ART/raw/review-prod.json
python3 -I $ART/raw/tsv_gen.py $ART/raw/review-prod.json > agent-docs/progress/2026-11-19-arch-v4-h/review-prod.tsv
```

生の出力は `$ART/raw/review-prod.txt`（窓ごとの集計と昇格後の要求の全件）と `$ART/raw/review-prod.json`。
`design_reviews_without_new_decl`（6）と その費用（$12.43）は、昇格後の要求 7 件の `risk.reasons` と review_run の cost を 1 件ずつ並べて数えた（§5 の表）。

## 3. reviewer run（A §7 C11）

| 窓 | run | wall の中央値（p90） | cost_usd の中央値（p90） | cost の合計（cost のある run） | 日あたり run・費用 |
|---|---|---|---|---|---|
| A の窓の再現 | 165 | 1,137.5 s（1,887.0 s） | $2.047（$4.247） | $280.17（127） | 21.64・$36.74 |
| A の記録（baseline.md §5.4） | 165 | 1,137.5 s（1,928.6 s） | $2.047（$4.255） | $280.17（127） | — |
| after | 30 | 514.4 s（1,415.9 s） | $2.167（$4.073） | $53.62（27） | 16.00・$28.59 |
| promo | 17 | 434.5 s（1,614.2 s） | $2.036（$2.924） | $26.57（16） | 17.22・$26.92 |
| pre_promo | 13 | 669.1 s（1,238.4 s） | $2.312（$4.238） | $27.05（11） | 14.64・$30.46 |

- A の窓の再現は run の数・中央値・合計が A・G と一致した。p90 の差は分位の計算方法の差（G scope.md §4 と同じ）。
- wall_ms のある run は A 130/165・after 27/30・promo 16/17。中央値は値のある run で取った。
- 設計 review の run も role=reviewer なので、この表の run に含まれる（promo の 17 run のうち 7 run）。

## 4. 設計 review

| 窓 | 要求（task） | 差し戻し（pass=false） | run の wall の中央値（p90） | run の cost の中央値 | run の cost の合計 | 日あたり 要求・費用 |
|---|---|---|---|---|---|---|
| A | 27（6） | 11 | 240.8 s（429.7 s） | $1.666 | $44.77（cost のある 26 run） | 3.541・$5.87 |
| old_rules | 43（10） | 18 | 238.1 s（461.2 s） | $1.674 | $75.37（42 run） | 4.552・$7.98 |
| pre_promo | 4（1: C） | 2 | 329.1 s（616.3 s） | $3.131 | $10.89 | 4.505・$12.27 |
| after | 11（4） | 5 | 300.0 s（514.4 s） | $2.312 | $25.22 | 5.867・$13.45 |
| promo | 7（3） | 3 | 300.0 s（418.6 s） | $2.301 | $14.33 | 7.092・$14.52 |

- 要求 1 件に design_reviewed が 1 件、review_run の run が 1 件ずつ対応した（どの窓も要求 = reviewed = run）。
- old_rules の 43 件は C の metrics.md §5.2 の 39 件（〜2026-11-17T21:45:07Z）に pre_promo の C の 4 件を足した数で、C の値と合う。
- C の見積もりは 1 回を A の reviewer run の中央値（1,137.5 s・$2.047）で置いた。昇格後の設計 review の run の実際は wall の中央値 300.0 s（見積もりの約 1/3.8）、
  cost の中央値 $2.301（約 1.12 倍）。C の申し送り task の 2 件は 14.0 s・$0.117 と 12.9 s・$0.124 で、軽い run もある。

## 5. 本番の判定経路で設計 review が始まった証拠（昇格後の 7 件、例 3 件）

| event | 時刻 | task | 理由（`risk.reasons`。level は 7 件とも high） | 設計 review の run・結果・費用 |
|---|---|---|---|---|
| 960582 | 2026-11-18T12:38:58Z | D（01M7MZTKB4RYM80S6CT3RST2N9） | `path_class:store:crates/task-core/src/store/org.rs` ほか store の path 3 個、`invariant:store:task-state-in-sqlite`、`invariant:store:no-data-loss` | 01M7QANSD90P1QH9J0ZKS8R9P6・差し戻し・$4.556・312.6 s |
| 961175 | 2026-11-18T13:02:46Z | C の申し送り（01M7Q70NDQAK1KKXHY6WT027QZ） | `path_class:review-gate:crates/task-core/src/design_review.rs`、`invariant:review-gate:completion-not-self-reported`、`invariant:review-gate:policy-change-needs-human`、`invariant:review-gate:no-llm-in-dispatcher` | 01M7QC1BNCC0DKSG701ZGGYR2T・合格・$0.117・14.0 s |
| 964589 | 2026-11-18T23:16:12Z | F（01M7MZTKB8A4NJQEEA35Q4G5JV） | `path_class:transaction:crates/task-api/src/knowledge.rs`、`path_class:transaction:crates/task-ops/src/lifecycle.rs`、`invariant:transaction:task-state-in-sqlite`、`invariant:transaction:lifecycle-human-epoch-same-tx` | 01M7RF4M8QKS8N43VTECCJYATB・合格・$2.823・514.4 s |

- 3 件とも `declared:new_*` が無い。旧い判定は「high_risk の path かつ new_state・new_branch・new_fallback・new_port の宣言」が主条件で、A の窓の 27 件の理由
  75 個はすべて `path_class:<class>+new_*` の形だった。昇格後は path と不変条件の名前を挙げる形に変わり、new_* の宣言なしで High になっている。
  review-gate・transaction は C の profile で足した class。
- 昇格後の 7 件の全部（`$ART/raw/review-prod.txt` の examples）:

| event | task | `declared:new_*` | 結果 | cost | wall |
|---|---|---|---|---|---|
| 960582 | D | 無し | 差し戻し | $4.556 | 312.6 s |
| 960771 | D | 無し | 差し戻し | $2.301 | 298.4 s |
| 961175 | C の申し送り | 無し | 合格 | $0.117 | 14.0 s |
| 961342 | D | 無し | 合格 | $2.506 | 354.6 s |
| 962093 | C の申し送り | 無し | 合格 | $0.124 | 12.9 s |
| 964461 | F | new_branch | 差し戻し | $1.905 | 300.0 s |
| 964589 | F | 無し | 合格 | $2.823 | 514.4 s |

- 「new_* の宣言が無い 6 件は旧い判定では High にならなかった」は推定で、旧い判定で再判定はしていない（計画が旧い判定の下なら宣言を変えた可能性もある）。

## 6. C の見積もりとの比較

| | 回数 | 費用 | 20.9 日に直した回数 | 20.9 日に直した費用 |
|---|---|---|---|---|
| C の見積もり（下限〜上限、commit の単位） | +0.670〜+4.498 回/日 | +$1.37〜+$9.21/日 | +14〜+94 | +$28.66〜+$192.42 |
| 見方 1: promo の日あたり − A の日あたり | +3.551 回/日 | +$8.65/日 | +74.2 | +$180.77 |
| 見方 1': promo − old_rules（参考） | +2.540 回/日 | +$6.54/日 | +53.1 | +$136.72 |
| 見方 2: promo の new_* の宣言の無い要求 | 6.08 回/日（6 件） | $12.59/日（$12.43） | 127.0 | $263.20 |

- 見方 1 は見積もりの範囲の中、見方 2 は上限を超える。見方 2 は「C が無ければ設計 review が無かった」とみなす上限側の数え方。
- 見積もりとの単位の違い: 見積もりは main の commit の単位、実績は計画の単位の要求（差し戻しの後の再要求を含む。D は 3 件のうち 2 件が差し戻し）。
- 偏り: 昇格後の 3 task はどれもアーキ v4 の task で、目的として高 risk の path（store・review-gate・transaction）を触る。通常の機能追加の割合は分からない。
- final review の中の設計の節（C の D3）は reviewer run の数を増やさない。節が足す費用は測っていない（C の §5.5 と同じ）。

## 7. 未達・要判断

| 項目 | 今の値 | 理由 | 次の手順 |
|---|---|---|---|
| C11: A と同じ長さの窓の reviewer run の比較 | after 1.875 日・30 run | 同じ長さ（7 日 15 時間）の窓 [2026-11-17T15:00, 2026-11-25T06:00) は 2026-11-25T06:00Z の後に揃う | §7.1 の手順を 2026-11-25T06:00Z の後（その窓に始まった reviewer run が全部終わった後）に走らせ、A の窓と比べる |
| 昇格後の設計 review の回数と費用と C の見積もりの比較 | promo 0.987 日・7 件・3 task | 窓が 1 日で、task がアーキ v4 の 3 件に偏る。見方 1 は範囲内、見方 2 は上限超えで、結論が出ない | 同じく §7.1。見直しの判断は人（決定 unmet）。誰が比べるかも決定 unmet で人が決める |

### 7.1 2026-11-25T06:00Z の後に同じ query で比べる手順

1. 本番 DB を `mode=ro` で開き、§2 の query を次の窓に当てる（道具を使うなら第 2 引数を `2026-11-25T06:00`、第 3 引数を昇格の時刻 `2026-11-18T12:18:40` にする）:
   after = [2026-11-17T15:00, 2026-11-25T06:00)（7.625 日、A と同じ長さ）、promo = [2026-11-18T12:18:40, 2026-11-25T06:00)。
2. 先に `select count(*) from runs r join tasks t on t.id=r.task_id where r.role='reviewer' and t.project_id='01M2WTS3DKNZBSZ2JMVB4CZMBW' and r.finished_at is null`
   が 0 か、窓の中に始まった未終了の run が無いことを確かめる。
3. 比べる値: reviewer run の数・wall と cost_usd の中央値・p90・合計を A の窓（165 run・1,137.5 s・$2.047・$280.17）と並べる。
   設計 review は promo の日あたりの要求数と run の費用から A の日あたり（3.541 件・$5.87）を引き、20.9 日に直して C の見積もり（+14〜+94 回・$28.66〜$192.42）と並べる。
   new_* の宣言の無い要求の数（見方 2）と、要求の task のうちアーキ v4 の task の数も並べる。
4. 窓の中に C の後の risk の規則の変更（`crates/task-core/src/design_review.rs`・profile の `[review]`）を含む release の昇格があれば、その時刻で窓を分ける。

## 再現と検査

- この WorkUnit の commit（件名）: 「arch-v4 H review-prod: 昇格後の review と設計 review の回数・時間・費用を本番 DB（mode=ro）で測り、C の見積もりと並べる」。
- 本番 DB を開いたのは `review_prod.py` と確認用の短い python だけで、どれも `file:…?mode=ro`（`uri=True`）で開いた。書き込みの文は使っていない。
- 回帰条件: A の窓の再現が 165 run・wall の中央値 1,137.5 s・cost の中央値 $2.047・合計 $280.17 に一致すること（query の条件が A と同じことの確認）。
- リリースゲート: この WorkUnit は docs だけを変える。統合後の release の gate は未実施（H の統合の後に release.sh が走らせる）。
