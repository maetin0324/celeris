# 対照 pilot の盲検の採点材料（戦略層 v1 子 8）

<!-- pilot-scoring: celeris.pilot-scoring/1 repo-copy -->

## この文書について

- 方式の名を、事例ごとに乱数の id（8 文字）に替えた。対応表は pilot-key.json にあり、採点の後まで開かない。
- pilot-key.json の sha256: `36841f6b189dbecbbb0217108f1118ab235c17c73fe800528ea2b6f8de64aace`
- 採点者は人（rubric の手順）。実装者・CoS・operator は採点しない。採点欄の点・根拠・採点者（`human:<名>`）・レビュー時間（分）を埋める。
- 既知の限界: 伏せたのは名前だけで、記録の内容（親の途中評価の本文の有無・戦略の版の有無）から方式を推測できる。
- 記録の文の中の方式の名に当たる語は ［伏せ字］ に替えた。answer key の値と史実の参照の点は書いていない。
- この写しは repo 用で、研究ケースの理由・主張・版の文を除いた（（研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る））。

## 採点の手順（rubric.md から）

<!-- rubric:採点の手順 -->
1. 採点者は人 1 名以上。方式の名を伏せた記録（各時点の判定・根拠・次の手・費用の記録）を受け取る。伏せ方: 記録の file 名を乱数の id に替え、方式の対応表は採点の後まで渡さない。
2. 採点者は answer key（`replay/answer_key.json` と研究ケースの参照表）を**採点の後に**見る。先に自分の判定を書き、後で差を記録する。
3. 6 つの軸を下の表の基準で 0〜2 点で付け、根拠（slice の file と行、event の (task, seq)）を 1 行書く。根拠を書けない点は 0 にする。
4. 2 名で採点したら軸ごとの差を記録し、差が 1 点を超えた軸は話し合って 1 つにする（話し合いの前の点も残す）。
5. 結果は `agent-docs/progress/` の子 8 の進捗 file に表で残す（方式 × 軸 × 採点者）。
<!-- /rubric -->

## 軸と基準（rubric.md から）

<!-- rubric:軸と基準 -->
| 軸 | 測るもの | 2 点 | 1 点 | 0 点 | 決定的な補助の数値 |
|---|---|---|---|---|---|
| execution success | 子の仕事が受け入れ条件どおりに終わったか | 全子が reviewer と command check で合格し、差し戻しの後も同じ不具合が戻らない | 合格したが差し戻し 2 回以上か、受け入れ条件の一部を未達と明記 | 不合格のまま、または自己申告だけで done | `review_verdict` の pass の比、`reviewer_passed` の有無 |
| epistemic progress | 仮説・反証・未確定が増えて整理されたか | 各時点で仮説を明記し、反証の後に改め、未確定を未確定と書く | 仮説はあるが、反証の後の改めが遅い（2 時点以上） | 仮説が無いか、反証を無視して同じ主張を続ける | 改めた仮説の数、反証から改めまでの時点の数 |
| parent goal achievement | 親の目的（原文の受入条件）の実測の改善 | 親の指標が改善し、実測の根拠がある | 一部の指標が改善、残りは未達と明記 | 改善が無いのに達成と書く、または前後比較が無い | 親の指標の before/after（アーキv4 は循環 17・SCC・S1/S2・LoC） |
| unsupported claims | 根拠の無い主張の数 | 0 件 | 1〜2 件で、後の時点で自ら訂正 | 3 件以上か、訂正が無い | 主張ごとに slice の中の根拠の有無を数える（無い = 1 件） |
| pivot の時点 | 未達を検知して手を変えた時点 | 最初に検知できた時点（replay は E-mid）か 1 時点後までに discriminating_experiment・pivot・ask_human のどれか | 2〜3 時点後 | 4 時点以上後か、最後まで変えない（史実のアーキv4 はこれ） | 検知の時点の順位の差、誤った pivot（answer key で on_track の時点の pivot）の数 |
| コスト | 結果を得るまでの費用 | 検知の後の worker の run と費用が史実の 25 % 以下 | 25〜75 % | 75 % を超える | 史実: 検知の後 137 run・264.25 USD（list price）。人の採点・review の時間（分）も書く |
<!-- /rubric -->

## 記録

### 事例 replay

#### 時点 C-mid

##### 記録 `dny1h1fe`（反復 iter-2）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠:
  - [local_review] 子 A の局所の review が合格（史実の記録） — 根拠: task_id=01M870CB299NSS1BY2PY7GBJ3V
  - [local_review] 子 B の局所の review が合格（史実の記録） — 根拠: task_id=01M870CB2D3NS71HGKKDR4JV5C
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

##### 記録 `xt8ed4vg`（反復 iter-2）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: 終端の子は 2/8（A・B）で、どちらも基準の計測と検査器の整備が中心。Outcome の指標を改善する実装（D〜F）は未着手で、reached=false。戦略を変える根拠はまだ無いので、依存順に C へ進める。
- 主張と根拠:
  - [measured] B の検査器変更で health の時間が悪化した（review の報告にある値。成果物では未確認） — 根拠: task_id=01M888QFVS37HE39HXJJ284GAZ
  - [learned] A が基準 bc665168 の計測コマンド（baseline.md の C7b・C10 など）を固定し、再実行で記載値（S1 92/369/12、S2 6/14/4）が出ると review で確認された。B 以降が同じ規則で before/after を測る土台はできた。 — 根拠: task_id=01M888QFVM79BGV9Q87Q8F0W70
  - [learned] B は repo-health を既存 SCC 内の新しい辺と #[path] にも効かせる変更として done になった。ただし #[path] の宣言行を数え過ぎる既知の限界が残る。 — 根拠: task_id=01M888QFVS37HE39HXJJ284GAZ
  - [uncertainty] Outcome の主指標（循環内辺、store/config 参照ファイル数、HTTP/CoS 循環、SQL 公開面、LoC）は、コードを変える子 D・E・F がすべて draft のため、改善の実測がまだ無い。 — 根拠: 参照無し
  - [uncertainty] B の health 時間の悪化（約 7%）を許容するかは人の判断待ち。 — 根拠: 参照無し
  - [uncertainty] C は ready で reviewer 未合格。6 ケースの実証は未確認。 — 根拠: 参照無し
- 戦略の版 v1（この記録の評価の作業 tree に置いた文書）:
  - 候補 `guard-first`: 。仮説 。識別実験: source 改変 0 の読み取りの実験。基準 main bc665168 の作業 tree で、(1) 10/28 から bc665168 までの production の commit を 1 件ずつ前後で module 依存を計測し（A の baseline.md の計測コマンドと規則をそのまま使う）、既存 SCC の node 数を変えずに内部辺が増えた commit の件数と辺の数を数える。(2) #[path] の 6 例について、物理 path と論理 module の両方で size と依存の検査を数え、結果が変わる例の数を数える。(3) 同じ期間の commit のうち、高リスク path の候補（task-api/knowledge.rs・cos/operations.rs・dispatcher/worker_finish.rs・task-worker/browser_policy.rs）に触れ、assess_risk が Low を返したものの件数を数える。結果を表（commit、増えた内部辺、検査の判定）で記録する。
  - 候補 `state-owner`: 。仮説 。識別実験: (1) 読み取り: 10/28 以降の production 修正 195 件のうち複数領域へ波及したものを、波及の経路で分類する（共有状態: self.store・self.config・SqliteStore の method・raw Connection を介した同時変更 / 循環内の辺の両端 / その他）。分類規則は実行前に固定して記録する。(2) 使い捨て branch（main に入れない）で、dispatcher の work_units 1 領域だけ、既存の owner・Decision・Coordination の狭い操作を呼ぶ形にし、dispatcher 非試験 file の self.store・self.config 参照 file 数（A と同じ scope と分母）、本体 LoC、A のシナリオ 2（WorkUnit 終了処理の変更）で触る file 数を before/after で測る。lease・restart・parallel WU の既存試験を回す。
  - 候補 `cycle-break`: 。仮説 。識別実験: 使い捨て branch（main に入れない）で、handlers/org.rs と cos/ops/admin.rs に混ざった業務ロジックのうち 1 操作だけを既存の task-ops か task-api 内の共有 use-case に寄せ、HTTP と CoS の adapter から一方向に呼ぶ形にする。A の計測コマンドで、handlers↔cos の辺の数、task-api の SCC の単位数と内部辺の数（基準 49 単位・261 辺）、本体 LoC、A のシナリオ 1（新しい CoS 業務操作）で触る file 数を before/after で測る。既存の org・admin の HTTP と CoS の試験（認可・拒否理由・response・同一 transaction）を回す。
- 費用: LLM run 1・0.1725844 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 1

##### 記録 `xt8ed4vg`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=succeeded、goal=not_met
- 理由: 終端の子 A・B は reviewer の合格つきで done になった。A は before を確定し、B は検査器の見逃しを実測で直した。しかし親 Outcome の構造の指標 (1)〜(4) は 1 つも改善していない。production の差分は 0 行で、health の時間は約 7〜8% 悪化した。戦略 v1 は「B〜H を次の版で選ぶまで止める」と置いたが、実際には固定計画どおり B・C が進んでいる。どの pilot（D・E・F）に費用をかけるか、C の policy 差分を承認するか、health の時間の悪化を受け入れるかは、人の判断が要る。このため ask_human を選んだ。
- 主張と根拠:
  - [measured] 旧い deps.cycle は既存 SCC の内部に辺を 1 本足す変更（m1: task-api/src/releases.rs に use crate::knowledge::KnowledgeItem を追加）を見逃していた。B の修正後は同じ変更を fail にする。 — 根拠: task_id=01M887M2VGZR5NZ9ZHS1TQVG54
  - [measured] モジュール循環の件数は B の前後で変わらない。B は production のコードを変えていない。内部の組の数が増えたのは、#[path] を論理 module として数え直したためで、結合が悪化したわけではない。 — 根拠: task_id=01M887M2VGZR5NZ9ZHS1TQVG54
  - [measured] B の差分は検査器と試験だけで、production の LoC は変わらない。crates・web・Cargo は bc665168 と同一。 — 根拠: task_id=01M887M2VGZR5NZ9ZHS1TQVG54
  - [measured] 新しい検査器では、health の所要時間が 2 シナリオとも約 7〜8% 延びた。 — 根拠: task_id=01M887M2VGZR5NZ9ZHS1TQVG54
  - [measured] 依頼の監査値を A が bc665168 で再計測し、reviewer の再実行でも一致した。これが before の基準になる。 — 根拠: task_id=01M887M2VCE9DE0KY19XW91P8Y、name=baseline.md sha256=f3804ffa7c48812b6f01dc5b590bb0ecdeb9c9a05b0d2d345b6574fc006a84d6 task_id=01M887M2VCE9DE0KY19XW91P8Y
  - [measured] 変更波及の before を A の C10 で固定した。after はまだ無い。 — 根拠: task_id=01M887M2VCE9DE0KY19XW91P8Y、name=baseline.md sha256=f3804ffa7c48812b6f01dc5b590bb0ecdeb9c9a05b0d2d345b6574fc006a84d6 task_id=01M887M2VCE9DE0KY19XW91P8Y
  - [measured] 論理 module の単位で数え直すと、cos→handlers の辺は増えて見える。コードは同じなので、実際の改善でも悪化でもない。 — 根拠: task_id=01M887M2VGZR5NZ9ZHS1TQVG54
  - [learned] h-gate-blind の前半は支持された。旧い deps.cycle は、ノード数を変えない既存 SCC 内の新しい辺を見逃していた（m1・m2 は旧で pass、新で fail）。#[path] で物理の置き場所を変えると、size と deps の検査をすり抜けられた。B は汎用 fixture の試験 8 件で、新しい内部辺の fail・既存内部辺の保持・辺削除の pass・合法な型 import の pass を対照で確かめた。ただし後半（検査を直せば以後の悪化が止まり、局所 pilot より先に効く）は未検証。 — 根拠: task_id=01M887M2VGZR5NZ9ZHS1TQVG54
  - [learned] held assumption a-baseline-valid は支持された。依頼の監査値（循環 17、SCC 55/210・49/261、self.store 44 file、self.config 35 file、size.responsibility-growth 94、dup.contract 210、wrong_owner 31）は、A の再計測と reviewer の再実行で一致した。修正の波及率も 46.9%〜49.6% で、依頼の約 49% と食い違わない。 — 根拠: task_id=01M887M2VCE9DE0KY19XW91P8Y、name=baseline.md sha256=f3804ffa7c48812b6f01dc5b590bb0ecdeb9c9a05b0d2d345b6574fc006a84d6 task_id=01M887M2VCE9DE0KY19XW91P8Y
  - [learned] 検査を直すだけでは、親の (1)〜(4) の after は改善しない。B は production を 0 行変えた。循環は 17→17 件で、内部の組と cos→handlers の辺は数え方の変更で増えて見える。このため、構造の改善は D〜F の pilot に依存する。 — 根拠: task_id=01M887M2VGZR5NZ9ZHS1TQVG54
  - [refuted] 戦略 v1 は「子 B〜H は次の版で選ぶまで dispatch しない」と置いた。しかし B は A の完了後に固定計画の順で実行されて done になり、C も dispatch されて running/ready で WU を進めている（phase_integrated impl、2026-11-18T07:41Z）。戦略の前提と実際の進行は一致していない。 — 根拠: task_id=01M887M2VGZR5NZ9ZHS1TQVG54、task_id=01M887M2VM6GF2EEW6GZ69NNYH
  - [uncertainty] 構造の指標（循環内部の辺、self.store・self.config を参照する file 数、handlers↔cos の import、波及率）は、まだ 1 つも改善していない。D・E・F が実際に純減を出せるかは不明。 — 根拠: 参照無し
  - [uncertainty] C（6 ケースの実証）は reviewer の合格が無い。policy の差分の人の承認と main に入れる順が決まるまで、leaf profile と integrate-impl は止まる。C の時点で、risk 判定が高リスク変更を止め、普通の変更を通すかは未確認。 — 根拠: 参照無し
  - [uncertainty] health の所要時間が約 7〜8% 延びた（S1 86.56→92.81 s、S2 81.92→88.68 s）。(5) の悪化として受け入れるか、人の判断が出ていない。 — 根拠: 参照無し
  - [uncertainty] #[path] の宣言の行を module_lines に数え過ぎる既知の限界がある。このため、本体の行を変えない移動でも size.responsibility-growth が fail になりうる。人は followup-policy-task を選んだが、その task の作成と完了は未確認。 — 根拠: 参照無し
  - [uncertainty] h-gate-blind の後半「検査を直せば以後の悪化が止まる」は未検証。B の replay（実 commit 10 件）で新しい検査器が何件を新たに止めたかは、slice に詳細が無く未確認。 — 根拠: 参照無し
  - [uncertainty] 統合後の release の gate は未実施。D〜F で安全不変条件（同一 transaction、lease、ACL など）が保たれるかは未確認。 — 根拠: 参照無し
- 戦略の版 v1（この記録の評価の作業 tree に置いた文書）:
  - 候補 `gate-first`: 。仮説 。識別実験: source 改変 0 で 2 つを測る。(a) bc665168 上の使い捨ての fixture branch で、task-core の既存 SCC 内に新しい module 辺を 1 本足し、#[path] の module を 1 つ別 dir へ移し、現行の repo-health（deps.cycle・size・lang_deps）と assess_risk の結果を記録する。(b) 10/28 以降の production 修正 195 件の各 commit について、既存 SCC の内部辺の増減・#[path] の module への変更・認可/lease/tx/event 判定の行の変更の有無を git diff と A の計測コマンドで数え、現行の検査が何件を見逃したかを表にする。
  - 候補 `owner-narrowing`: 。仮説 。識別実験: 使い捨ての branch（main に merge しない）で、dispatcher の work_units 領域だけについて、self.store・self.config を直接使う非試験 file を既存の owner の狭い操作の呼び出しに置き換える試作をする。A と同じ計測コマンドと scope（非試験 69 file）で、self.store 参照 file 数・self.config 参照 file 数・本体の LoC・module 辺を before/after で記録し、既存の lease・restart の試験を実行する。合わせて 195 件の修正のうち dispatcher の self.store/self.config 参照 file を 2 つ以上同時に変えたものの割合を数える。
  - 候補 `adapter-cycle-break`: 。仮説 。識別実験: source 改変 0 で、handlers/org.rs と cos/ops/admin.rs の間の相互 import と、両方にある業務ロジックを関数単位で列挙し、共有 use-case へ集約したときに消える import 辺の数と重複行数を見積もる。合わせて 195 件の修正のうち task-api の handlers と cos の両方を同時に変えたものの割合を数える。
- 費用: LLM run 1・0.34725490000000003 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 9

##### 記録 `39qsqvyx`（反復 iter-2）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=succeeded、goal=not_met
- 理由: 終端の子は A・B の 2/8 で、どちらも reviewer 合格つきの done。Outcome は reached=false で、改善の after 値はまだ無い。C 以降の完了と実測を待つため、戦略は変えずに続ける。
- 主張と根拠:
  - [learned] A（基準確定）と B（repo-health の既存 SCC 内の新規辺・#[path] への対応）は reviewer の合格つきで done。A の再レビューでは差し戻し 2 点（import 数と波及の測定コマンド不足）が直った経緯があり、B・C 以降が使う測定の土台ができたことは、done と reviewer 合格という task の状態から言える。ただし after の数値は未確認。 — 根拠: task_id=01M870DCW2EWRGZMJBFD41JX4J、task_id=01M870DCW6JKEY876PEAK4Y9HJ
  - [uncertainty] 親の指標（循環内辺・store 参照 file 数・LoC・build/health 時間）の after 値は、この評価では artifact の参照つきで確かめていない。B では health 時間が S1 86.56→92.81 s、S2 81.92→88.68 s と悪化したと子が報告しており、受け入れるかは人の判断待ち。 — 根拠: 参照無し
  - [uncertainty] C は ready のまま reviewer 未合格で、6 ケースの実証が済んでいるか未確認。D〜H は draft で、親 Outcome の主要な改善（循環解消・Dispatcher・Store 境界）は未着手。 — 根拠: 参照無し
- 費用: LLM run 1・0.11053170000000001 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 1

##### 記録 `39qsqvyx`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=succeeded、goal=not_met
- 理由: 終端の子は A・B の 2 件で、どちらも検査の土台づくりだ。Outcome の指標（循環内辺・store 参照・変更波及）を改善する D・E・F は未着手で、reached は false。計画の順序どおりに進んでいて、今の戦略を変える根拠は無いので continue を選ぶ。ただし改善の実測が無いので at_risk とする。
- 主張と根拠:
  - [learned] 子 A（done、reviewer 合格）で before の基準と計測コマンド（baseline.md、進捗 file）が固定された。B 以降が同じ規則で前後比較できる前提は整った。数値そのものは子の報告で、この評価では再計測していない。 — 根拠: task_id=01M87053PE7S6K6MGF8DGQ99P8
  - [learned] 子 B（done、reviewer 合格）で repo-health の deps.cycle・size・lang_deps の検査器の改修が main に入った。共通受入条件の (1) に効く検査の土台であり、Outcome の改善ではない。 — 根拠: task_id=01M87053PK7G48BZSQHHNTM7BS
  - [uncertainty] B の reviewer が人の判断事項として挙げた health 時間の悪化（約 6 秒）を受け入れるか。また #[path] 宣言の行を数え過ぎる既知の限界の扱い。どちらも子の報告の申告で、未確認。 — 根拠: 参照無し
  - [uncertainty] 責務境界・循環内辺・store 参照数など Outcome の本体指標の after は、D・E・F が未着手（draft）のため 1 件も無い。C も ready で reviewer 合格が無い。 — 根拠: 参照無し
- 費用: LLM run 1・0.1113399 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 1

##### 記録 `dny1h1fe`（反復 iter-1）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠:
  - [local_review] 子 A の局所の review が合格（史実の記録） — 根拠: task_id=01M87041NHEECCCY3DMTHNW8YE
  - [local_review] 子 B の局所の review が合格（史実の記録） — 根拠: task_id=01M87041NP5A0F2A04BZGFYEVW
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

#### 時点 D-done

##### 記録 `dny1h1fe`（反復 iter-1）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠:
  - [local_review] 子 A の局所の review が合格（史実の記録） — 根拠: task_id=01M8706N6FDM77NJCCKESCJ717
  - [local_review] 子 B の局所の review が合格（史実の記録） — 根拠: task_id=01M8706N6MG76773FC389N7C5F
  - [local_review] 子 C の局所の review が合格（史実の記録） — 根拠: task_id=01M8706N6S33AS74AHSDRYREWP
  - [local_review] 子 D の局所の review が合格（史実の記録） — 根拠: task_id=01M8706N7A8VTXE61WDT2QFYKX
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

##### 記録 `xt8ed4vg`（反復 iter-2）

- 評価の記録の状態: invalid（run: budget_exhausted、shape parent_evaluation: 節が無い）。判定と次の手は gate の既定値で、評価器の答えではない
- 判定（verdict）: unknown
- 次の手（next）: ask_human
- 主張と根拠: 無し
- 戦略の版 v1（この記録の評価の作業 tree に置いた文書）:
  - 候補 `guard-first`: 。仮説 。識別実験: source 改変 0 の読み取りの実験。基準 main bc665168 の作業 tree で、(1) 10/28 から bc665168 までの production の commit を 1 件ずつ前後で module 依存を計測し（A の baseline.md の計測コマンドと規則をそのまま使う）、既存 SCC の node 数を変えずに内部辺が増えた commit の件数と辺の数を数える。(2) #[path] の 6 例について、物理 path と論理 module の両方で size と依存の検査を数え、結果が変わる例の数を数える。(3) 同じ期間の commit のうち、高リスク path の候補（task-api/knowledge.rs・cos/operations.rs・dispatcher/worker_finish.rs・task-worker/browser_policy.rs）に触れ、assess_risk が Low を返したものの件数を数える。結果を表（commit、増えた内部辺、検査の判定）で記録する。
  - 候補 `state-owner`: 。仮説 。識別実験: (1) 読み取り: 10/28 以降の production 修正 195 件のうち複数領域へ波及したものを、波及の経路で分類する（共有状態: self.store・self.config・SqliteStore の method・raw Connection を介した同時変更 / 循環内の辺の両端 / その他）。分類規則は実行前に固定して記録する。(2) 使い捨て branch（main に入れない）で、dispatcher の work_units 1 領域だけ、既存の owner・Decision・Coordination の狭い操作を呼ぶ形にし、dispatcher 非試験 file の self.store・self.config 参照 file 数（A と同じ scope と分母）、本体 LoC、A のシナリオ 2（WorkUnit 終了処理の変更）で触る file 数を before/after で測る。lease・restart・parallel WU の既存試験を回す。
  - 候補 `cycle-break`: 。仮説 。識別実験: 使い捨て branch（main に入れない）で、handlers/org.rs と cos/ops/admin.rs に混ざった業務ロジックのうち 1 操作だけを既存の task-ops か task-api 内の共有 use-case に寄せ、HTTP と CoS の adapter から一方向に呼ぶ形にする。A の計測コマンドで、handlers↔cos の辺の数、task-api の SCC の単位数と内部辺の数（基準 49 単位・261 辺）、本体 LoC、A のシナリオ 1（新しい CoS 業務操作）で触る file 数を before/after で測る。既存の org・admin の HTTP と CoS の試験（認可・拒否理由・response・同一 transaction）を回す。
- 費用: LLM run 1・0.3527431 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 10

##### 記録 `39qsqvyx`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=succeeded、goal=not_met
- 理由: 終端の 4 子は合格で done だが、Outcome の指標の改善はまだ実測されていない。D は循環を減らせず、C は LoC を増やした。E〜H が残るため、現戦略を続けて H の再計測で判定する。D の未達の扱いは人の判断が要る。
- 主張と根拠:
  - [measured] A が baseline を確定した。B の検査器は旧版と比べ health 時間が悪化した（reviewer の報告に基づく。私は再測定していない） — 根拠: task_id=01M8707R0ZF8638P147R0PJ54T
  - [measured] C の統合後、本体 LoC は純増した。共通規則の「純減を成果にしない」ので成果に数えない（reviewer が測り直した値） — 根拠: task_id=01M8707R14FAZ18MGFR40ZW100
  - [measured] D の pilot 後も handlers と cos の単位の循環は残る（reviewer の報告） — 根拠: task_id=01M8707R1MS0D16HW7G90N74HX
  - [learned] A〜D は reviewer の合格つきで done になり、計測規則・検査器の差分検出・設計リスク判定の土台は入った。ただし親 Outcome の指標（循環内辺・store 参照 file 数・変更波及）の改善にはまだ効いていない。 — 根拠: task_id=01M8707R0T1WASBHJPDH86AZZN、task_id=01M8707R0ZF8638P147R0PJ54T、task_id=01M8707R14FAZ18MGFR40ZW100
  - [refuted] D の reviewer 報告では、組 261・単位 49 が不変で、S1 の変更波及も減っていない。この仮説は今のところ支持されない。 — 根拠: task_id=01M8707R1MS0D16HW7G90N74HX
  - [uncertainty] E・F・G・H が未完で、(2) Dispatcher の store/config 参照 file 数、(5) 時間と費用の 3 回比較、(6) LoC の最終値は未測定。 — 根拠: 参照無し
  - [uncertainty] B で health 時間が悪化した。D で循環が残った。これらを受け入れるかは人の判断待ち。本評価は子の報告を再検証していない。 — 根拠: 参照無し
- 費用: LLM run 1・0.12245910000000002 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 1

##### 記録 `xt8ed4vg`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=succeeded、goal=not_met
- 理由: A〜D は reviewer 合格の done だが、Outcome の中心の指標（循環・store 参照・変更波及）はまだ減っていない。D は指標が不変だった。E〜H は未完で、今は戦略を変える根拠が足りないため、戦略を続け、E で実際に減るかを確かめる。D の未達の扱いは人の判断を要する。
- 主張と根拠:
  - [measured] B: repo-health の health 時間が悪化した（検査器の強化の代償） — 根拠: task_id=01M887Q907EE53VGCCX143YB4C
  - [measured] C: production LoC は純増。C は検査・規約の改善で、削減が目的ではない — 根拠: task_id=01M887Q90BKSVAS1BK2BMF4JW2
  - [measured] D: 循環の組数・単位数は不変。S1 の変更波及も減っていない。D 自身が未達/要判断と申告している — 根拠: task_id=01M887Q90S0QBSC84ZTEERQR9E
  - [learned] A の baseline（C7b・C10 など再現コマンド付き）が固定され、B と D は同じ規則で before/after を測れた。計測の土台の仮説は支持された。 — 根拠: task_id=01M887Q903K1YD0ESRJA4NYARN
  - [learned] B で、既存 SCC 内の新しい辺と #[path] のすり抜けに汎用 repo-health が効くようになったことを、実 commit 10 件の replay で確かめた（B の報告。私は再実行していない）。 — 根拠: task_id=01M887Q907EE53VGCCX143YB4C
  - [refuted] D は done で reviewer も合格だが、循環の組 261・単位 49 は不変で、rusqlite を使う file が 1 つ増えた。集約の pilot だけでは循環の指標は動かなかった。 — 根拠: task_id=01M887Q90S0QBSC84ZTEERQR9E
  - [uncertainty] Outcome の中心の指標（循環内辺、store/config を使う file 数、変更波及、Store の公開面）は、E〜H が未完のため、改善が実測されていない。D の指標は不変だった。 — 根拠: 参照無し
  - [uncertainty] D の未達/要判断（循環が残ること、S1 の波及が減らないこと）を人が受け入れるか未決。 — 根拠: 参照無し
  - [uncertainty] B で health 時間が約 6 秒（約 7%）悪化した。受け入れるかは人の判断待ち。#[path] の宣言の行を数え過ぎる既知の限界も残る。 — 根拠: 参照無し
  - [uncertainty] 私は DB と成果物を再実行していない。上の数値は各子の報告に基づくので、報告の自己申告の域を出ない。 — 根拠: 参照無し
- 戦略の版 v1（この記録の評価の作業 tree に置いた文書）:
  - 候補 `gate-first`: 。仮説 。識別実験: source 改変 0 で 2 つを測る。(a) bc665168 上の使い捨ての fixture branch で、task-core の既存 SCC 内に新しい module 辺を 1 本足し、#[path] の module を 1 つ別 dir へ移し、現行の repo-health（deps.cycle・size・lang_deps）と assess_risk の結果を記録する。(b) 10/28 以降の production 修正 195 件の各 commit について、既存 SCC の内部辺の増減・#[path] の module への変更・認可/lease/tx/event 判定の行の変更の有無を git diff と A の計測コマンドで数え、現行の検査が何件を見逃したかを表にする。
  - 候補 `owner-narrowing`: 。仮説 。識別実験: 使い捨ての branch（main に merge しない）で、dispatcher の work_units 領域だけについて、self.store・self.config を直接使う非試験 file を既存の owner の狭い操作の呼び出しに置き換える試作をする。A と同じ計測コマンドと scope（非試験 69 file）で、self.store 参照 file 数・self.config 参照 file 数・本体の LoC・module 辺を before/after で記録し、既存の lease・restart の試験を実行する。合わせて 195 件の修正のうち dispatcher の self.store/self.config 参照 file を 2 つ以上同時に変えたものの割合を数える。
  - 候補 `adapter-cycle-break`: 。仮説 。識別実験: source 改変 0 で、handlers/org.rs と cos/ops/admin.rs の間の相互 import と、両方にある業務ロジックを関数単位で列挙し、共有 use-case へ集約したときに消える import 辺の数と重複行数を見積もる。合わせて 195 件の修正のうち task-api の handlers と cos の両方を同時に変えたものの割合を数える。
- 費用: LLM run 1・0.20515339999999999 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 2

##### 記録 `39qsqvyx`（反復 iter-2）

- 判定（verdict）: at_risk
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=succeeded、goal=not_met
- 理由: 終端の子 4 件は reviewer 合格の done だが、Outcome の acceptance を満たす実測の改善はまだ無い（C は +664 行、D は循環不変）。E 以降も未完で、D の未達の扱いに人の判断が要るため ask_human を選ぶ。
- 主張と根拠:
  - [measured] B: repo-health の health 時間は B の変更で悪化（B の review が人の判断項目として挙げた値） — 根拠: task_id=01M870G05SZBQ6Q51V9STQES8V
  - [measured] C: production LoC は純増（設計 review の仕組みを足した変更）。本体純減ではない — 根拠: task_id=01M870G05YB52XN66CVPVEQWHD
  - [measured] D: handlers と cos の単位の循環は残り、SCC の規模は不変（worker の未達/要判断の申告） — 根拠: task_id=01M870G06FCNDRQ8ES5VC7FCR2
  - [learned] A の baseline と計測コマンドが固定され、B〜D は同じ規則で before/after を測れている。ただし本評価は子の成果物の本文を直接は確かめておらず、数値は子の報告に依る。 — 根拠: task_id=01M870G05MF8S7TW07J8FTJZ4S
  - [learned] D の pilot では use-case 集約だけでは handlers⇄cos の単位循環も S1 の変更波及も減らなかった。局所的な集約では循環の指標が動かない可能性が示された。 — 根拠: task_id=01M870G06FCNDRQ8ES5VC7FCR2
  - [uncertainty] Outcome の主指標（循環内辺、store 参照 file 数、変更波及、LoC 純減）のうち、改善が実測された値がまだ無い。E・F・G・H が未完。 — 根拠: 参照無し
  - [uncertainty] B で health 時間が約 6 秒悪化。受け入れるか未決。D の循環が残ったまま F 以降に進んでよいかも未決。 — 根拠: 参照無し
  - [uncertainty] release gate は統合後に未実施。 — 根拠: 参照無し
- 費用: LLM run 1・0.13755669999999998 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 3

##### 記録 `dny1h1fe`（反復 iter-2）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠:
  - [local_review] 子 A の局所の review が合格（史実の記録） — 根拠: task_id=01M870EYB761YCZAF6B5BCZVCW
  - [local_review] 子 B の局所の review が合格（史実の記録） — 根拠: task_id=01M870EYBCK09E6EVB2N8EGKA2
  - [local_review] 子 C の局所の review が合格（史実の記録） — 根拠: task_id=01M870EYBH9MS58D3K4XWDXRN7
  - [local_review] 子 D の局所の review が合格（史実の記録） — 根拠: task_id=01M870EYCTPSQ6SP3H92W2KEA2
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

#### 時点 F-done

##### 記録 `dny1h1fe`（反復 iter-2）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠:
  - [local_review] 子 A の局所の review が合格（史実の記録） — 根拠: task_id=01M870HQHXA5H7180Z7H4H08D3
  - [local_review] 子 B の局所の review が合格（史実の記録） — 根拠: task_id=01M870HQJ1Q0GFVCTBK3K0X3E5
  - [local_review] 子 C の局所の review が合格（史実の記録） — 根拠: task_id=01M870HQJ7GRCZX10KTY1YJ4JX
  - [local_review] 子 D の局所の review が合格（史実の記録） — 根拠: task_id=01M870HQJQP2PQF7JKVSBZADJP
  - [local_review] 子 E の局所の review が合格（史実の記録） — 根拠: task_id=01M870HQK0CH0VT1WKP79E111A
  - [local_review] 子 F の局所の review が合格（史実の記録） — 根拠: task_id=01M870HQK3VF91S2ZFNY1C332S
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

##### 記録 `39qsqvyx`（反復 iter-2）

- 判定（verdict）: at_risk
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: 8 子のうち 6 子が reviewer 合格つきの done だが、G・H が未着手で、受入条件 (4)(5) の after が無く、D の循環は残る。Outcome は未達（reached=false）。測定値は報告由来で、自分では再計測していない。D・E の扱いと B の時間悪化の許容は人の判断が要るため ask_human を選ぶ。
- 主張と根拠:
  - [measured] E: self.store を書く dispatcher の file 数（分母 69、出現数 633 は不変）。reviewer が A の C3 で数え直した値 — 根拠: task_id=01M870JTM23SCVDHK3WRS9D5YS
  - [measured] C: 提出時の HEAD での production LoC は増えた（C は設計リスク判定の改善で、減らす task ではない） — 根拠: task_id=01M870JTM86WM7ZRZDTAH2JDNK
  - [learned] B・C・F は reviewer 合格つきの done。B は repo-health の既存 SCC 内の新辺と #[path] への対応、C は 6 ケースの実証、F は knowledge.rs/lifecycle.rs の tx 境界の整理を扱う。ただし本評価は成果物の実体を自分で再計測しておらず、これらは報告と reviewer の記述に基づく（未確認）。 — 根拠: task_id=01M870JTK3RH2HCCYC066W09PC、task_id=01M870JTK86WM7ZRZDTAH2JDNK、task_id=01M870JTM566Z0E3G12MJKWJ84
  - [refuted] D の reviewer 報告では、単位レベルの handlers⇄cos の循環が残り（組 261・単位 49 は不変）、S1 の変更波及も減っていない。worker 自身が未達/要判断と申告している。 — 根拠: task_id=01M870JTKSRP93ZZ14K7J7XEX1
  - [uncertainty] G（build・変更局所性・時間と費用の 3 回比較）と H（最終統合検査）が draft で、受入条件 (4)(5) の after が未測定。B の health 時間は S1 86.56→92.81 s、S2 81.92→88.68 s と悪化している。 — 根拠: 参照無し
  - [uncertainty] D の循環未解消と E の目安（store 5 file 以上・config 3 file 以上の純減）に対する実績（store 6 純減は達成、config 4 純減は達成）の、人が求める水準との適合は未確認。出現数は減っていない。 — 根拠: 参照無し
- 費用: LLM run 1・0.143106 USD
- 決定的な指標: 根拠の無い claim 1（参照無し 0・slice の外 1）、secret の形 0、記録の mutation 有、読んだ path 1

##### 記録 `xt8ed4vg`（反復 iter-1）

- 評価の記録の状態: invalid（run: budget_exhausted、shape parent_evaluation: 節が無い）。判定と次の手は gate の既定値で、評価器の答えではない
- 判定（verdict）: unknown
- 次の手（next）: ask_human
- 主張と根拠: 無し
- 戦略の版 v1（この記録の評価の作業 tree に置いた文書）:
  - 候補 `gate-first`: 。仮説 。識別実験: source 改変 0 で 2 つを測る。(a) bc665168 上の使い捨ての fixture branch で、task-core の既存 SCC 内に新しい module 辺を 1 本足し、#[path] の module を 1 つ別 dir へ移し、現行の repo-health（deps.cycle・size・lang_deps）と assess_risk の結果を記録する。(b) 10/28 以降の production 修正 195 件の各 commit について、既存 SCC の内部辺の増減・#[path] の module への変更・認可/lease/tx/event 判定の行の変更の有無を git diff と A の計測コマンドで数え、現行の検査が何件を見逃したかを表にする。
  - 候補 `owner-narrowing`: 。仮説 。識別実験: 使い捨ての branch（main に merge しない）で、dispatcher の work_units 領域だけについて、self.store・self.config を直接使う非試験 file を既存の owner の狭い操作の呼び出しに置き換える試作をする。A と同じ計測コマンドと scope（非試験 69 file）で、self.store 参照 file 数・self.config 参照 file 数・本体の LoC・module 辺を before/after で記録し、既存の lease・restart の試験を実行する。合わせて 195 件の修正のうち dispatcher の self.store/self.config 参照 file を 2 つ以上同時に変えたものの割合を数える。
  - 候補 `adapter-cycle-break`: 。仮説 。識別実験: source 改変 0 で、handlers/org.rs と cos/ops/admin.rs の間の相互 import と、両方にある業務ロジックを関数単位で列挙し、共有 use-case へ集約したときに消える import 辺の数と重複行数を見積もる。合わせて 195 件の修正のうち task-api の handlers と cos の両方を同時に変えたものの割合を数える。
- 費用: LLM run 1・0.2949775 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 10

##### 記録 `dny1h1fe`（反復 iter-1）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠:
  - [local_review] 子 A の局所の review が合格（史実の記録） — 根拠: task_id=01M8709DE9JZXNN6TZTT7Q1YFX
  - [local_review] 子 B の局所の review が合格（史実の記録） — 根拠: task_id=01M8709DEDANS3VV2Q9A0WPF8S
  - [local_review] 子 C の局所の review が合格（史実の記録） — 根拠: task_id=01M8709DEKHG7SX2491ECKHJ49
  - [local_review] 子 D の局所の review が合格（史実の記録） — 根拠: task_id=01M8709DF3EVWHGHEW2GG07RVX
  - [local_review] 子 E の局所の review が合格（史実の記録） — 根拠: task_id=01M8709DFC3M5SWBSJ064M7WKQ
  - [local_review] 子 F の局所の review が合格（史実の記録） — 根拠: task_id=01M8709DFGZDDMNBS4P0YF4ES3
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

##### 記録 `39qsqvyx`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: 終端の子は 6/8 で、B〜F は reviewer 合格つきの done。E は store・config 参照の file を減らしたが、D は循環を解消せず、B は health 時間、C は LoC を悪化させた。G・H が未実施で (4)(5) の after が無く、Outcome の到達は未確認。今の戦略で G・H を進め、実測で判断する。
- 主張と根拠:
  - [measured] E で self.store・self.config を書く dispatcher file が減った（E の review が A の C3 で数え直した値）。出現数 633 と分母 69 は不変 — 根拠: task_id=01M870AGBCEJ76G4EG565R9ZRJ
  - [measured] C は production LoC が増えた（+664）。C の final review が測り直した値で、親 Outcome の純減目標とは別に監視が要る — 根拠: task_id=01M870AGAJM46N7G4X6NG9SYA0
  - [measured] D 後も handlers ⇄ cos の単位の循環は残り、task-api の SCC は組 261・単位 49 のまま（D の review が要判断として挙げた） — 根拠: task_id=01M870AGB37RVBD9TFZK43GN50
  - [measured] B の新検査器で health 時間が悪化（B の review の報告値） — 根拠: task_id=01M870AGAD7JPRCBHJQEDB013A
  - [learned] A の baseline（計測コマンド C1〜C10、2 シナリオ S1・S2）が B〜F の before/after で再利用できた。子 B〜F はいずれも reviewer 合格つきの done で、同じ計測規則で比較されている。 — 根拠: task_id=01M870AGA8E6W3XXQJMPHE7JMP、task_id=01M870AGAD7JPRCBHJQEDB013A
  - [learned] E の pilot は、目安（store 5 file 以上・config 3 file 以上の純減）を store 6・config 4 で満たした。ただし出現数は不変で、狭い操作への置換が効いた範囲は 2 領域に限られる。 — 根拠: task_id=01M870AGBCEJ76G4EG565R9ZRJ
  - [refuted] D の review は、単位の循環が残る・組 261 と単位 49 が不変・S1 の変更波及が減らない・rusqlite の file が +1 と記録した。この仮説は支持されなかった。 — 根拠: task_id=01M870AGB37RVBD9TFZK43GN50
  - [uncertainty] 子 G（build・test・health・review の時間と費用の 3 回比較）と子 H（統合と数値のまとめ）は draft で未実施。共通受入条件 (4)(5) の after は未確認。 — 根拠: 参照無し
  - [uncertainty] B で health 時間が悪化（S1 86.56→92.81 s）、C で production が +664 行。LoC 純増が Outcome の趣旨に反しないかは未判断。 — 根拠: 参照無し
  - [uncertainty] D の循環が残ったまま F まで進んだ。cross-boundary import の減少が親 Outcome に足りるかは未確認。子の報告は主張で、ここでは証拠に数えていない。 — 根拠: 参照無し
- 費用: LLM run 1・0.1289822 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 1

##### 記録 `xt8ed4vg`（反復 iter-2）

- 評価の記録の状態: invalid（run: budget_exhausted、shape parent_evaluation: 節が無い）。判定と次の手は gate の既定値で、評価器の答えではない
- 判定（verdict）: unknown
- 次の手（next）: ask_human
- 主張と根拠: 無し
- 戦略の版 v1（この記録の評価の作業 tree に置いた文書）:
  - 候補 `guard-first`: 。仮説 。識別実験: source 改変 0 の読み取りの実験。基準 main bc665168 の作業 tree で、(1) 10/28 から bc665168 までの production の commit を 1 件ずつ前後で module 依存を計測し（A の baseline.md の計測コマンドと規則をそのまま使う）、既存 SCC の node 数を変えずに内部辺が増えた commit の件数と辺の数を数える。(2) #[path] の 6 例について、物理 path と論理 module の両方で size と依存の検査を数え、結果が変わる例の数を数える。(3) 同じ期間の commit のうち、高リスク path の候補（task-api/knowledge.rs・cos/operations.rs・dispatcher/worker_finish.rs・task-worker/browser_policy.rs）に触れ、assess_risk が Low を返したものの件数を数える。結果を表（commit、増えた内部辺、検査の判定）で記録する。
  - 候補 `state-owner`: 。仮説 。識別実験: (1) 読み取り: 10/28 以降の production 修正 195 件のうち複数領域へ波及したものを、波及の経路で分類する（共有状態: self.store・self.config・SqliteStore の method・raw Connection を介した同時変更 / 循環内の辺の両端 / その他）。分類規則は実行前に固定して記録する。(2) 使い捨て branch（main に入れない）で、dispatcher の work_units 1 領域だけ、既存の owner・Decision・Coordination の狭い操作を呼ぶ形にし、dispatcher 非試験 file の self.store・self.config 参照 file 数（A と同じ scope と分母）、本体 LoC、A のシナリオ 2（WorkUnit 終了処理の変更）で触る file 数を before/after で測る。lease・restart・parallel WU の既存試験を回す。
  - 候補 `cycle-break`: 。仮説 。識別実験: 使い捨て branch（main に入れない）で、handlers/org.rs と cos/ops/admin.rs に混ざった業務ロジックのうち 1 操作だけを既存の task-ops か task-api 内の共有 use-case に寄せ、HTTP と CoS の adapter から一方向に呼ぶ形にする。A の計測コマンドで、handlers↔cos の辺の数、task-api の SCC の単位数と内部辺の数（基準 49 単位・261 辺）、本体 LoC、A のシナリオ 1（新しい CoS 業務操作）で触る file 数を before/after で測る。既存の org・admin の HTTP と CoS の試験（認可・拒否理由・response・同一 transaction）を回す。
- 費用: LLM run 1・0.3713691 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 10

### 事例 research

#### 時点 11.16

##### 記録 `ewnqz6v2`（反復 iter-1）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠: 無し
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

##### 記録 `1gttmq2w`（反復 iter-2）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M88954FFJ4TK4CPRPEY6PFMY、task_id=01M88954FGYSPR7GYXF4S0VH88
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M88954FFJ4TK4CPRPEY6PFMY、task_id=01M88954FGYSPR7GYXF4S0VH88
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M88954FFJ4TK4CPRPEY6PFMY、task_id=01M88954FGYSPR7GYXF4S0VH88
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M88954FGYSPR7GYXF4S0VH88
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M88954FFJ4TK4CPRPEY6PFMY、task_id=01M88954FGYSPR7GYXF4S0VH88
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M88954FFJ4TK4CPRPEY6PFMY、task_id=01M88954FGYSPR7GYXF4S0VH88
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M88954FFJ4TK4CPRPEY6PFMY、task_id=01M88954FGYSPR7GYXF4S0VH88
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M88954FGYSPR7GYXF4S0VH88
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 費用: LLM run 1・0.21265509999999999 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 5

##### 記録 `30ndgmd9`（反復 iter-2）

- 判定（verdict）: at_risk
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2ECBPVHAB2DKSWA7V4、task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2ECBPVHAB2DKSWA7V4、task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2ECBPVHAB2DKSWA7V4、task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2ECBPVHAB2DKSWA7V4、task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8897K2ECBPVHAB2DKSWA7V4、task_id=01M8897K2FVQ1TFM5B1Y9DH9XD
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 戦略の版 v1: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 費用: LLM run 1・0.2657108 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 6

##### 記録 `ewnqz6v2`（反復 iter-2）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠: 無し
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

##### 記録 `30ndgmd9`（反復 iter-1）

- 評価の記録の状態: invalid（run: budget_exhausted、shape parent_evaluation: 節が無い）。判定と次の手は gate の既定値で、評価器の答えではない
- 判定（verdict）: unknown
- 次の手（next）: ask_human
- 主張と根拠: 無し
- 戦略の版 v1: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 費用: LLM run 1・0.3301986 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 10

##### 記録 `1gttmq2w`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8881Y5751HAR7K7PCC9M72R、task_id=01M8881Y584SYZBEHYH7Q9E1S6
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8881Y5751HAR7K7PCC9M72R、task_id=01M8881Y584SYZBEHYH7Q9E1S6
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8881Y5751HAR7K7PCC9M72R、task_id=01M8881Y584SYZBEHYH7Q9E1S6
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8881Y584SYZBEHYH7Q9E1S6
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8881Y584SYZBEHYH7Q9E1S6、task_id=01M8881Y5751HAR7K7PCC9M72R
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8881Y584SYZBEHYH7Q9E1S6
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8881Y584SYZBEHYH7Q9E1S6
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 費用: LLM run 1・0.2601667 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 有、読んだ path 8

#### 時点 11.17

##### 記録 `1gttmq2w`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8888MQYEHECJ842ARJ8E5TW、task_id=01M8888MQZ36R7G1EE92J3KH03、task_id=01M8888MR1G1YJP4WV0MGX9D3X
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8888MQZ36R7G1EE92J3KH03、task_id=01M8888MR1G1YJP4WV0MGX9D3X
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8888MQYEHECJ842ARJ8E5TW、task_id=01M8888MQZ36R7G1EE92J3KH03
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8888MQYEHECJ842ARJ8E5TW、task_id=01M8888MQZ36R7G1EE92J3KH03、task_id=01M8888MR1G1YJP4WV0MGX9D3X
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8888MQYEHECJ842ARJ8E5TW、task_id=01M8888MQZ36R7G1EE92J3KH03
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8888MQZ36R7G1EE92J3KH03
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8888MR1G1YJP4WV0MGX9D3X
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M8888MQZ36R7G1EE92J3KH03
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 費用: LLM run 1・0.39229230000000004 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 8

##### 記録 `30ndgmd9`（反復 iter-1）

- 判定（verdict）: on_track
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888BJ2DV108VQFFYY0XQ3VG、task_id=01M888BJ2EFW3FDQ3WKSKDDWR2、task_id=01M888BJ2GVCRKW088GGRHJ6JY
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888BJ2DV108VQFFYY0XQ3VG、task_id=01M888BJ2EFW3FDQ3WKSKDDWR2、task_id=01M888BJ2GVCRKW088GGRHJ6JY
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888BJ2DV108VQFFYY0XQ3VG、task_id=01M888BJ2EFW3FDQ3WKSKDDWR2
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888BJ2EFW3FDQ3WKSKDDWR2
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888BJ2GVCRKW088GGRHJ6JY
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888BJ2EFW3FDQ3WKSKDDWR2
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888BJ2EFW3FDQ3WKSKDDWR2、task_id=01M888BJ2GVCRKW088GGRHJ6JY
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 戦略の版 v1: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 費用: LLM run 1・0.39153920000000003 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 6

##### 記録 `1gttmq2w`（反復 iter-2）

- 判定（verdict）: on_track
- 次の手（next）: continue
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889C3727WPK010MPHRKG935、task_id=01M889C3736B12B7FZA2TG8HMQ、task_id=01M889C3744AJ1H34NDSFF5S1V
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889C3736B12B7FZA2TG8HMQ、task_id=01M889C3744AJ1H34NDSFF5S1V
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889C3727WPK010MPHRKG935、task_id=01M889C3736B12B7FZA2TG8HMQ
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889C3736B12B7FZA2TG8HMQ
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889C3744AJ1H34NDSFF5S1V
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889C3727WPK010MPHRKG935、task_id=01M889C3736B12B7FZA2TG8HMQ
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 費用: LLM run 1・0.3005696 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 9

##### 記録 `30ndgmd9`（反復 iter-2）

- 判定（verdict）: on_track
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNJH57R7RMP40S2EA7H、task_id=01M889ERNKYTPN32XR471QJ4VY
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNKYTPN32XR471QJ4VY、task_id=01M889ERNMEBS60TEFEFFG63CP
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNKYTPN32XR471QJ4VY
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNJH57R7RMP40S2EA7H、task_id=01M889ERNKYTPN32XR471QJ4VY
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNKYTPN32XR471QJ4VY、task_id=01M889ERNMEBS60TEFEFFG63CP
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNKYTPN32XR471QJ4VY
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNMEBS60TEFEFFG63CP
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNKYTPN32XR471QJ4VY
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889ERNKYTPN32XR471QJ4VY、task_id=01M889ERNMEBS60TEFEFFG63CP
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 戦略の版 v1: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 費用: LLM run 1・0.29497419999999996 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 9

##### 記録 `ewnqz6v2`（反復 iter-1）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠: 無し
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

##### 記録 `ewnqz6v2`（反復 iter-2）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠: 無し
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

#### 時点 11.20

##### 記録 `1gttmq2w`（反復 iter-2）

- 判定（verdict）: at_risk
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3Y8GG1JM9K1XRYFGGWF、task_id=01M889K3Y951YXDKSFZGK56YGH
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3Y8GG1JM9K1XRYFGGWF、task_id=01M889K3Y951YXDKSFZGK56YGH
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3YA4P16V8XDQD0SFAES、task_id=01M889K3YBPK392TQ5JGGQHHND、task_id=01M889K3YCQ8G6QKK16H7V5SB8、task_id=01M889K3YDBKXHC3PCQ6JN8ABP
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3Y8GG1JM9K1XRYFGGWF、task_id=01M889K3Y951YXDKSFZGK56YGH、task_id=01M889K3YCQ8G6QKK16H7V5SB8
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3Y951YXDKSFZGK56YGH、task_id=01M889K3YA4P16V8XDQD0SFAES、task_id=01M889K3YBPK392TQ5JGGQHHND
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3Y951YXDKSFZGK56YGH
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3Y8GG1JM9K1XRYFGGWF、task_id=01M889K3Y951YXDKSFZGK56YGH、task_id=01M889K3YDBKXHC3PCQ6JN8ABP
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3YA4P16V8XDQD0SFAES、task_id=01M889K3YBPK392TQ5JGGQHHND、task_id=01M889K3YCQ8G6QKK16H7V5SB8、task_id=01M889K3YDBKXHC3PCQ6JN8ABP
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3Y951YXDKSFZGK56YGH
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889K3YDBKXHC3PCQ6JN8ABP
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 費用: LLM run 1・0.2933102 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 8

##### 記録 `30ndgmd9`（反復 iter-2）

- 判定（verdict）: at_risk
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVH7613NEA9V1XF2QZF9、task_id=01M889NVH9MBJWX9F0SPV8RVCB、task_id=01M889NVHB4C17MHWHZ9XVG7MA、task_id=01M889NVHES184BX3Q83R7V64W、name=strategy/pilot-research/v1.json sha256=sha256:e0db59942362ff0978293486d863320d027fa8e754ac2184ca6e27d4eb0aab7e task_id=01M889NVH5DEQCEBWCBGN72CP5
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVH9MBJWX9F0SPV8RVCB、task_id=01M889NVHB4C17MHWHZ9XVG7MA、task_id=01M889NVHCVR2A5554G74C0DP1、task_id=01M889NVHD39V3K6ANXHMJ0048、task_id=01M889NVHES184BX3Q83R7V64W
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVH7613NEA9V1XF2QZF9、task_id=01M889NVH9MBJWX9F0SPV8RVCB、name=strategy/pilot-research/v1.json sha256=sha256:e0db59942362ff0978293486d863320d027fa8e754ac2184ca6e27d4eb0aab7e task_id=01M889NVH5DEQCEBWCBGN72CP5
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVH7613NEA9V1XF2QZF9、task_id=01M889NVH9MBJWX9F0SPV8RVCB、task_id=01M889NVHD39V3K6ANXHMJ0048
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVHES184BX3Q83R7V64W
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVH9MBJWX9F0SPV8RVCB、name=strategy/pilot-research/v1.json sha256=sha256:e0db59942362ff0978293486d863320d027fa8e754ac2184ca6e27d4eb0aab7e task_id=01M889NVH5DEQCEBWCBGN72CP5
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVH9MBJWX9F0SPV8RVCB、task_id=01M889NVHB4C17MHWHZ9XVG7MA、task_id=01M889NVHES184BX3Q83R7V64W、name=strategy/pilot-research/v1.json sha256=sha256:e0db59942362ff0978293486d863320d027fa8e754ac2184ca6e27d4eb0aab7e task_id=01M889NVH5DEQCEBWCBGN72CP5
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVH9MBJWX9F0SPV8RVCB、name=strategy/pilot-research/v1.json sha256=sha256:e0db59942362ff0978293486d863320d027fa8e754ac2184ca6e27d4eb0aab7e task_id=01M889NVH5DEQCEBWCBGN72CP5
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M889NVH7613NEA9V1XF2QZF9、task_id=01M889NVH9MBJWX9F0SPV8RVCB、task_id=01M889NVHES184BX3Q83R7V64W、name=strategy/pilot-research/v1.json sha256=sha256:e0db59942362ff0978293486d863320d027fa8e754ac2184ca6e27d4eb0aab7e task_id=01M889NVH5DEQCEBWCBGN72CP5
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 戦略の版 v1: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 費用: LLM run 1・0.29464690000000004 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 7

##### 記録 `30ndgmd9`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC0CBQ0QXXSQ9C7KFPT、task_id=01M888GNC1Q5ASWTHPDC99NXAX
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC2HW2PEMH5CSMYMM61、task_id=01M888GNC3NV5KSGB1T8E76B35、task_id=01M888GNC48A2KXVFG6TM1G10J、task_id=01M888GNC55SBCQ0JXWDR1YJVH
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC2HW2PEMH5CSMYMM61、task_id=01M888GNC3NV5KSGB1T8E76B35、task_id=01M888GNC48A2KXVFG6TM1G10J、task_id=01M888GNC55SBCQ0JXWDR1YJVH
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC0CBQ0QXXSQ9C7KFPT、task_id=01M888GNC1Q5ASWTHPDC99NXAX
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC55SBCQ0JXWDR1YJVH
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC1Q5ASWTHPDC99NXAX
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC2HW2PEMH5CSMYMM61、task_id=01M888GNC3NV5KSGB1T8E76B35、task_id=01M888GNC48A2KXVFG6TM1G10J、task_id=01M888GNC55SBCQ0JXWDR1YJVH
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC0CBQ0QXXSQ9C7KFPT、task_id=01M888GNC1Q5ASWTHPDC99NXAX、task_id=01M888GNC55SBCQ0JXWDR1YJVH
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC1Q5ASWTHPDC99NXAX
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888GNC1Q5ASWTHPDC99NXAX、task_id=01M888GNC55SBCQ0JXWDR1YJVH
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 戦略の版 v1: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 費用: LLM run 1・0.2647752 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 7

##### 記録 `1gttmq2w`（反復 iter-1）

- 判定（verdict）: at_risk
- 次の手（next）: ask_human
- 評価の軸（axes）: epistemic=advanced、execution=partial、goal=not_met
- 理由: （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る）
- 主張と根拠:
  - [measured] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888F26CY4MXJQ61GQTVMCB4、task_id=01M888F26DR88E5X50DEKKQ0SP、task_id=01M888F26EJ8Y8Q60DVHC58X9P、task_id=01M888F26FEKS5WA7D2SDTPWNR、task_id=01M888F26GD67D4D1KPA6K0HZN
  - [learned] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888F26BASZJ970WDC1HT3A4、task_id=01M888F26CY4MXJQ61GQTVMCB4、task_id=01M888F26DR88E5X50DEKKQ0SP、task_id=01M888F26FEKS5WA7D2SDTPWNR
  - [refuted] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: task_id=01M888F26CY4MXJQ61GQTVMCB4
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
  - [uncertainty] （研究ケースの本文は repo に写さない。root の artifacts の pilot-scoring.md を見る） — 根拠: 参照無し
- 費用: LLM run 1・0.1533694 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 1

##### 記録 `ewnqz6v2`（反復 iter-1）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠: 無し
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

##### 記録 `ewnqz6v2`（反復 iter-2）

- 判定（verdict）: —（親の途中評価の記録が無い）
- 次の手（next）: continue
- 主張と根拠: 無し
- 費用: LLM run 0・0 USD
- 決定的な指標: 根拠の無い claim 0（参照無し 0・slice の外 0）、secret の形 0、記録の mutation 無、読んだ path 0

## id ごとの決定的な指標

検知できた時点（replay は E-mid）から検知までの史実の worker run は rubric のコストの軸の材料（史実 137 run・264.25 USD）。研究ケースの worker run は数えていない（未確認）。

| 事例 | id | 反復 | 未達発見の時点 | 未達発見までの task 数 | 検知できた時点から検知までの史実の worker run・USD | 検知の後の史実の worker run・USD | LLM run | USD | 根拠の無い claim | secret の形 | mutation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| replay | `39qsqvyx` | iter-1 | 無し | 8 | 137・264.2525 | — | 3 | 0.362781 | 0 | 0 | 有 |
| replay | `39qsqvyx` | iter-2 | D-done | 4 | 1・2.2666 | 136・261.9859 | 3 | 0.391194 | 1 | 0 | 有 |
| replay | `dny1h1fe` | iter-1 | 無し | 8 | 137・264.2525 | — | 0 | 0 | 0 | 0 | 無 |
| replay | `dny1h1fe` | iter-2 | 無し | 8 | 137・264.2525 | — | 0 | 0 | 0 | 0 | 無 |
| replay | `xt8ed4vg` | iter-1 | C-mid | 2 | 0・0 | 200・350.1604 | 5 | 1.474531 | 0 | 0 | 有 |
| replay | `xt8ed4vg` | iter-2 | 無し | 8 | 137・264.2525 | — | 5 | 1.357666 | 0 | 0 | 有 |
| research | `1gttmq2w` | iter-1 | 11.20 | 6 | — | — | 3 | 0.805828 | 0 | 0 | 有 |
| research | `1gttmq2w` | iter-2 | 11.20 | 6 | — | — | 3 | 0.806535 | 0 | 0 | 無 |
| research | `30ndgmd9` | iter-1 | 11.20 | 6 | — | — | 5 | 1.530184 | 0 | 0 | 有 |
| research | `30ndgmd9` | iter-2 | 11.16 | 2 | — | — | 5 | 1.571612 | 0 | 0 | 有 |
| research | `ewnqz6v2` | iter-1 | 無し | — | — | — | 0 | 0 | 0 | 0 | 無 |
| research | `ewnqz6v2` | iter-2 | 無し | — | — | — | 0 | 0 | 0 | 0 | 無 |

## 採点欄

### 事例 replay・id `39qsqvyx`

<!-- score case=replay id=39qsqvyx -->
| 軸 | 点（0〜2） | 根拠 |
|---|---|---|
| execution success |  |  |
| epistemic progress |  |  |
| parent goal achievement |  |  |
| unsupported claims |  |  |
| pivot の時点 |  |  |
| コスト |  |  |

- 採点者: 
- レビュー時間（分）: 
<!-- /score -->

### 事例 replay・id `dny1h1fe`

<!-- score case=replay id=dny1h1fe -->
| 軸 | 点（0〜2） | 根拠 |
|---|---|---|
| execution success |  |  |
| epistemic progress |  |  |
| parent goal achievement |  |  |
| unsupported claims |  |  |
| pivot の時点 |  |  |
| コスト |  |  |

- 採点者: 
- レビュー時間（分）: 
<!-- /score -->

### 事例 replay・id `xt8ed4vg`

<!-- score case=replay id=xt8ed4vg -->
| 軸 | 点（0〜2） | 根拠 |
|---|---|---|
| execution success |  |  |
| epistemic progress |  |  |
| parent goal achievement |  |  |
| unsupported claims |  |  |
| pivot の時点 |  |  |
| コスト |  |  |

- 採点者: 
- レビュー時間（分）: 
<!-- /score -->

### 事例 research・id `1gttmq2w`

<!-- score case=research id=1gttmq2w -->
| 軸 | 点（0〜2） | 根拠 |
|---|---|---|
| execution success |  |  |
| epistemic progress |  |  |
| parent goal achievement |  |  |
| unsupported claims |  |  |
| pivot の時点 |  |  |
| コスト |  |  |

- 採点者: 
- レビュー時間（分）: 
<!-- /score -->

### 事例 research・id `30ndgmd9`

<!-- score case=research id=30ndgmd9 -->
| 軸 | 点（0〜2） | 根拠 |
|---|---|---|
| execution success |  |  |
| epistemic progress |  |  |
| parent goal achievement |  |  |
| unsupported claims |  |  |
| pivot の時点 |  |  |
| コスト |  |  |

- 採点者: 
- レビュー時間（分）: 
<!-- /score -->

### 事例 research・id `ewnqz6v2`

<!-- score case=research id=ewnqz6v2 -->
| 軸 | 点（0〜2） | 根拠 |
|---|---|---|
| execution success |  |  |
| epistemic progress |  |  |
| parent goal achievement |  |  |
| unsupported claims |  |  |
| pivot の時点 |  |  |
| コスト |  |  |

- 採点者: 
- レビュー時間（分）: 
<!-- /score -->

## 合格線（rubric.md から）

<!-- rubric:合格線 -->
- 方式 (b)・(c) が (a) より良いと言える条件: **pivot の時点が 2 点**、**unsupported claims が 1 点以上**、**parent goal achievement が 0 点でない**（未達なら未達と書いた 1 点を含む）、の 3 つを replay と研究ケースの両方で満たし、
  合計点が (a) を 3 点以上上回ること。
- どの方式でも、execution success が 2 点で parent goal achievement が 0 点の記録（子の done だけで親の成果と扱った記録）は、合計点によらず不合格にする。
- 誤った pivot（answer key で on_track・unknown の時点で pivot した）が 2 回以上なら、pivot の時点の軸を 0 点にする。
- 採点者 2 名の合計点の差が 4 点を超えたら、採点をやり直す（基準の読み違いとみなす）。
<!-- /rubric -->

## 採点の後に開く

採点を書き終えてから開く（rubric の手順 2）。
- answer key: `agent-docs/progress/2026-11-21-strategy-v1-0/replay/answer_key.json`
- 研究ケースの参照の表: `agent-docs/progress/2026-11-21-strategy-v1-0/research/research-case.md` の「時点ごとの採点の参照」
- 方式の対応表: `pilot-key.json`（sha256 は冒頭）
- 指標の全体（answer key との一致・誤った pivot・見逃しを含む）: `/local/celeris/data/workspaces/01M7XQA74FP70G3T3KSTP02S0W/artifacts/pilot-metrics/metrics.json`
