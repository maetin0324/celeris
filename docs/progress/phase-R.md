# Phase R: 再帰的な task 分解（ADR-0079）

## R0: 設計（完了 2026-09-28）

- ADR: [`docs/adr/0079-recursive-task-decomposition.md`](../adr/0079-recursive-task-decomposition.md)
- 種類: 設計のみ（コード・本番の DB と設定は変えていない。本番 DB は `sqlite3 "file:/var/lib/celeris/celeris.sqlite3?mode=ro"` で読んだだけ）。
  cargo のビルド・テストは実行していない（変更は文書だけ）。
- 置き換えの注記を足した ADR（本文は書き換えず、末尾に 1 段落）: ADR-0038・0044・0048・0062（関係のみ、置き換えなし）・0072・0074・0077。
- `docs/DESIGN.md` と `docs/SPEC.md` は変えていない（SPEC への提案は ADR-0079 D17、DESIGN への提案は下の「提案」）。

### 発端（2026-09-28）

ChatGPT（RDC）からの「browser capability を足す。Phase 1〜4」という依頼を、CoS が「設計 + Phase 1 MVP」の 1 task（01M3MFS5T52FXA63W4V10XGC4S）に
した。その task の ExecutionPlan の工程（adopt / harden / ship）は実行の段取りで、製品の Phase ではない。Phase 2〜4 の小タスクと人の決定点 H1〜H7 は
設計の成果物（WU design-gap の `phases-and-decisions.md`）に落ち、行き場が無い。案件に付く「案件計画」（ADR-0074 D3）は本番で一度も使われておらず、
案件「agent-platform の自己改善」は実態として粗い入れ物になっている。人の結論: 案件と task の責務がずれた。直し方は新しい中間層ではなく、task の構造を
再帰にすること。対応表は ADR-0079 §5。

### 人の決定（2026-09-28。原文を日本語で記録。固定）

1. **節点の種類は task だけ。** task は目的・受け入れ条件・決定点を持ち、任意で計画 = 順序付きの段階（stage）を持つ。段階の unit は **leaf**（今の
   WorkUnit: 1 回の run で終わる）か **子 task**（自分の計画を持つ）のどちらか。製品の「Phase N」は root task の段階の名前にすぎない。
   「取り組み / 途中目標 / 案件計画」はすべてこの構造で表し、専用の中間層は作らない。
2. **抽象度は自動で管理する。** Complexity Gate はすべての節点で走る（子 task は自分で gate をやり直す。atomic → 1 run、compound → 自分の計画）。
   planner は深さと残りの深さを受け取り、明示的な leaf の基準（1 run で終わる: 有界の turn、1 つの領域・リポジトリ、機械的な検査）に照らして各 unit を
   leaf / task と宣言する。最終判断は gate にあり、食い違いは記録する。上限: **max_depth = 3**（root → 子 task → leaf）、段階あたりの unit の最大数、
   木あたりの leaf の総数と費用。深い段ほど atomic に寄せる。上限に収まらない unit は**人への決定の要求**にし、黙って leaf に押し込まない。
3. root の計画を人が承認するのは、その計画が**決定点を含むとき、または上限に近いとき**だけ。それ以外は通知して進める。決定の要求はどの深さからでも
   出せ、木の中の位置（path）を持ち、受信箱と通知（cluster_login_needed で使っている Discord の経路が既にある）に出る。答えの無い決定は、それに依存しない
   unit を止めない。子の失敗は上限の範囲で親の replan（repair）が吸収し、人に届くのは上限の超過だけ。**基盤の失敗は決して「質問」として人に届けない**
   （自動で回復し、回復できなければ障害通知）。
4. 子 task は**親のブランチ**に成果を取り込む。main に取り込むのは root だけ（今日見た「task の最終レビュー中に main が進んで merge_base がずれる」も
   これで消える）。
5. 案件（project）が持つのは**ごく粗い方向**（「BenchFS に取り組む」「celeris を良くする」「hoge クラスタの維持管理」）と、常設の文脈（組織ノードの既定、
   リポジトリ、知識の置き場 `projects/<slug>`、quota の枠、常設の認可、報告の集約）だけ。**案件に付く計画（ADR-0074 D3、`POST /projects/{id}/plan`、
   project-plan/1、途中目標の auto_advance、F6 の GUI「案件計画を提案させる」）は廃止する。** 案件には依存の無い独立した root task が並列に付くだけで、
   依存は root task の木の中にだけある。

### 決定の一覧（ADR-0079）

D1 再帰のモデルと語彙 / D2 `celeris.execution-plan/3` / D3 深さと上限 / D4 節点ごとの gate と子 task の生成 / D5 段階の完了 / D6 親ブランチへの取り込み
（「配送」→「成果の取り込み」）/ D7 決定の要求 / D8 root 計画の承認の規則 / D9 replan / repair の再帰と、人に届くもの / D10 木の生存確認 / D11 報告と
metrics の roll-up / D12 CoS の指針 / D13 案件モデルの変更 / D14 GUI / D15 後方互換（採用 adopt を含む）/ D16 安全と上限 / D17 SPEC / DESIGN との関係。

### 実装計画（ADR-0079 §7）

R1a（plan/3 とデータモデル、migration 0030）→ R1b（子 task の生成と段階の完了）→ R1c（親ブランチへの取り込み）→ R2a（再帰の gate と木の上限）→
R2b（planner と replan の再帰）→ R3a（決定の要求の流れ）→ R3b（root 計画の承認と生存確認）→ R4a（木と roll-up の API）→ R4b（GUI）→
R5a（案件モデルの変更と CoS の指針）→ R5b（本番の移行と dogfood: browser の root と BenchFS の root）。`[execution.tree] enabled` は R5b まで既定 `false`。

### 未解決事項（人に確認したいこと。ADR-0079 §8）

- **U-R1**: 「max_depth = 3（root → 子 task → leaf）」を「task の段は root と子の 2 つ、leaf が 3 段目（孫 task は無い）」と読んだ。孫 task まで許す意味なら定義を 1 つずらす。**R1a の前に確認したい。**
- **U-R2**: 部署をまたぐ子 task の認可を、今の approvals（人に聞く）のままにするか、root 計画の承認に含めるか（既定は前者）。
- **U-R3**: 承認を挟まない root の計画は報告の流れに 1 件残すだけ（Discord は鳴らさない）で良いか。
- **U-R4**: 木の上限の既定値（leaf 40、run 120、段階あたり 6、同時の子 2）は推測の初期値。
- **U-R5**: root の gate の既定（本番は 17:06Z から `gate = "on"`。木の子は shadow でも gate を採用する設計）。
- **U-R6**: `POST /plans`（ADR-0028 の Plan kind）も 410 にして分解の経路を一本化するか。
- **U-R7**: 子 task ごとの reviewer run の費用を許すか（既定は許す）。
- **U-R8**: agent-platform の非終端の途中目標の行（approved / in_progress）を凍結のまま表示するか、R5b で一括 `cancelled` にするか。

### 提案

- DESIGN §5.6（計画が不正なら 1 回だけ再試行、それでも不正なら failed）に「内部の実行計画 /3 では failed でも atomic でもなく人への決定の要求にする
  （ADR-0079 D9）」の注記を足す（ADR-0072 D24 の提案と同じ場所）。
- SPEC §3.3 / §7 への 1 文ずつの追記案は ADR-0079 D17。

### 次の一歩

U-R1（max_depth の数え方）に人が答えた後、R1a（plan/3 の型と検証・`Task.tree`・migration 0030・新しい Event・`[execution.tree]`）を 1 セッションで実装する。

### R0 追記: 未決点への人の決定（2026-09-28 21:0xZ）

U-R1 = task の層数で数える（根 1 / 子 2 / 孫 3、葉は数えない）、U-R2 = 認可は今のまま、U-R3 = 承認不要の計画は通知なし、U-R4 = 既定値は草案どおり
（dogfood で調整）、U-R5 = gate は設定に従う、U-R6 = `POST /plans` も 410、U-R7 = reviewer run は当面許容し深さ別・部分木別の review 数と費用を
指標化（Prometheus は棚上げ）、U-R8 = 未終了の途中目標は凍結し将来非表示。追加で **R6 回収フェーズ**（既存の案件・task を新モデルの実情に合わせる）を置く。
次: R1a（plan/3 の型と検証、Task.tree、migration 0030、events、`[execution.tree]` 設定）。
