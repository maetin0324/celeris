---
tasks: [01M75722SX158YRMFYPQRVEEDY]
---
# ADR 2026-11-12: 受信箱の一次対応で CoS が答える範囲を「人に残す件の列挙以外すべて」にし、Celeris の外の操作は operator 宛てに分ける（policy_version 12）

- 状態: 採用。実装の進み具合は [進捗](../progress/2026-11-11-cos-triage-scope.md)。
- 置き換えるもの: [ADR 2026-10-28 定型の代答](2026-10-28-cos-routine-delegated-answers.md) D1 の「CoS の単独の回答は定型の
  7 種類だけ」。D2 の basis の形（`category`・`refs`）と、定型ごとの決定的な検査（(a) 枠の計算・(b) 一度だけ・(e) 差し戻し・
  (f) 計画の守り・(g) job 待ち）は残す。
- 関係: [ADR 2026-10-06 CoS 受信箱の一次対応](2026-10-06-cos-inbox-triage.md)、
  [ADR 2026-10-27 取り次いだ指示](2026-10-27-relayed-instructions.md)、
  [ADR 2026-11-11 CoS の常設の文脈](2026-11-11-cos-standing-context.md)（常設の節・grant）、
  [ADR 2026-11-11 退避の時刻](2026-11-11-cos-fallback-timing.md)、`docs/ops/external-operator.md` §4、
  [ADR 2026-10-11 破壊的変更](2026-10-11-breaking-changes-by-default.md)。

## 背景（本番で operator が読み取りで確かめた数字）

- 2026-11-11（UTC）に CoS は受信箱の件を 16 件人に回し、知らせ 19 件を見届け、自分で答えた件は 0 件だった。回答・計画の承認・
  決定の event 20 件はすべて `operator:claude-code`。10-28 以降、人に回した 220 件に対し、人の指示なしの回答は 1 件。
- 人に回した理由は、write set を広げる決定（task の ADR の範囲内）、未回答の決定がある計画の承認（7 件）、兄弟 task の取り込み方、
  policy の file を main に入れたかの確認（CoS には main に入れる手段が無い）などで、多くは task の目的・ADR から答えられた。
- 方針（skill cos-inbox-triage v11）は CoS の単独の回答を定型の 7 種類に限り、それ以外を全て escalate にしていた。
  job 待ちで人に回した 6 件のうち 4 件の run は skill を開いていない（方針の要点が prompt に無い）。
- 人の了承（2026-11-11、operator session）: 「人に残す件を列挙し、それ以外は CoS が task の目的の範囲で判断する」方針を task にする。
  背景は「celeris 内で基本完結させ、どうしても出来ないものを claude code（外部 operator）から操作する」。

## 決定

### D1 人に残す件（列挙。これ以外は CoS が判断する）

| 分類（`reserved`） | 中身 |
|---|---|
| `credential_input` | login・TOTP・OAuth など credential の入力（クラスタの login・ブラウザの credential 待ちを含む） |
| `beyond_approval` | 承認（grant・予算・node-h の枠）を超える予算と資源。枠を確かめられないときも含む |
| `objective_change` | task・案件の目的や受け入れ条件の変更 |
| `security_boundary` | credential とセキュリティの境界（秘密の扱い・権限・sandbox の緩和・恒久の認可の新設） |
| `production_config` | 本番 config の変更 |
| `paid_trial` | 有料の追加試行（実 LLM の pilot 等、既存の承認の外） |
| `research_judgment` | 研究上の判断（評価条件・凍結した評価の変更） |
| `weaken_checks` | health の指摘や検査を弱める・例外にする判断 |

人が設定した `human_required`（label・human check・override の待ち）と `[cos.triage] human_required` の分類は従来どおり解除できない。

### D2 Celeris の外の操作が要る件は operator 宛て

policy だけの branch を main に入れる、本番の昇格・rollback、host の操作（systemd・`/local`・本番 DB）など、Celeris の API に
経路が無い操作が要る件は、人ではなく外部 operator（`docs/ops/external-operator.md`）向けとして回す。

- escalation packet に宛先 `to` を必須の欄として足す: `human` か `operator`。
- `human`: 従来どおり outbox に通知を積み、統括 CoS の札を書き、Discord に送る。
- `operator`: 経路（`cos_notification_routes.route = 'operator'`）だけを取り、通知の行（Discord）と統括 CoS の札を作らない。
  件は `escalated` のまま受信箱に残り、`GET /api/v1/cos/inbox` の `addressee` が `operator` になる。operator はそこから拾う。
- D1 の分類に当たる件を `operator` 宛てにすると API が 422 `cos_reserved_for_human` で断る（人の判断を operator に渡さない）。

### D3 それ以外は CoS が task の目的の範囲で答え、根拠を残す

- basis の `category` に `task_scope`（task の目的・受け入れ条件・ADR・grant・人の過去の指示から CoS が判断する）を足す。
  旧 (c) `documented_default` と (d) `work_guidance` は `task_scope` に含まれるので消す（受けた本文は 422）。
- `task_scope` の `refs` には根拠を最低 1 件含める: `task`（目的・受け入れ条件を読んだ task の id）・`adr`・`doc`・`progress`・
  `decision`・`standing_rule`・`approval`・`message`（人の発言の id）・`authority_grant` のどれか。`commit`・`artifact`・`usage`・
  `ledger`・`review` は証拠であって根拠にはならない。根拠を示せないときだけ人に回す。
- 残す決定的な検査（答えの形で決まる。`task_scope` で迂回できない）:
  - 計画の承認（plan_gate の `approve`）は `plan_conforms`（または `within_approved_frame`）。
  - 失敗の件の `reopen` は `review_rework`。木の上限の `raise-once` は `raise_once`。
  - 認可（authorization）への回答は `job_wait_extend` か `within_approved_frame` だけ。認可は権限の境界なので `task_scope` では答えない。
  - D1 の分類に当たると daemon と API が決定的に判定した件（D4）への回答は 422 `cos_reserved_for_human`。
- 監査の reason は「CoS の判断（<category>、方針 <policy_version>、grant <id…>）: <reason>」。payload の `basis` と
  `policy_version`、判断者（grant・人の発言の出典）と実行者（CoS）は別々に残る（ADR 2026-10-27-relayed-instructions の記録を弱めない）。
  人は既存の取消・差し戻し（override）で覆せる。

### D4 daemon の事前の照合（決定的、LLM なし）

daemon は CoS の run を組むとき、受信箱の件ごとに `task_ops::triage_match::match_item` で次を調べ、件に付けて prompt に出す。
同じ関数を resolve の API も使い、D1 の判定と operator 宛ての拒否に使う（正本は 1 つ）。

- `reserved`: 件の種類と出所から決まる D1 の分類（例: `credential_wait`・`cluster_login` → `credential_input`、
  `browser_wait` の credential の待ち → `credential_input`、計画の承認の理由が `near_limit:` → `beyond_approval`）。
- `candidates`: 当てはまる定型の答えの候補（例: クラスタ job の待ちの上限の認可で、分類 `job_wait_extend` の生きた grant があれば
  「`job_wait_extend` の候補、grant <id>」。plan_gate なら `plan_conforms`、reviewer の差し戻しの失敗なら `review_rework`）。
- 照合は store を読むだけで、LLM を呼ばない。候補は判断材料で、API の検査を省かない。

### D5 方針の要点を常設の節に置き、詳細は skill に置く

- 人に残す件の列挙・operator 宛て・答えるときに残す根拠の要点を、CoS の常設の文脈（ADR 2026-11-11-cos-standing-context）の
  受信箱の一次対応の節に置く。skill を開かない run でも読める。Core（9100 byte の上限）には足さない。
- skill cos-inbox-triage を version 12 にし、`[cos.triage] policy_version` の既定を `"12"` にする。

### D6 計画の承認で未回答の決定があるとき

CoS が同じ run で決定に先に答え（`task_scope`。推奨のある決定で取り消しが安く、task の範囲のもの）、その後に計画を
`plan_conforms` で判定し直す。決定が D1 に当たれば人に回し、計画の承認も人に回す。daemon の「決定の回答による計画の再判断」は
決定に答えた主体（人・CoS）によらず起こす。

### D7 撤去

- `BasisCategory::DocumentedDefault`・`WorkGuidance`、その検査と文書、「定型の 7 種類のどれにも当たらない判断は escalate」の行。
- skill v11 の §2 の表の「上記以外で (a)〜(g) のどれにも当たらない判断」の行と §3 の「定型の代答 7 種類」の節。

## 本番への移行

`~/.config/celeris/config.toml` の `[cos.triage] policy_version` を `"12"` にし、KB の skill を v12 に差し替える
（手順は進捗の「本番への移行」節）。
