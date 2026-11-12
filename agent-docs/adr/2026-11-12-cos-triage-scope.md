---
tasks: [01M75722SX158YRMFYPQRVEEDY]
---
# ADR 2026-11-12: 受信箱の一次対応で CoS が答える範囲を「人に残す件の列挙以外すべて」にし、Celeris の外の操作は operator 宛てに分ける（policy_version 12）

- 状態: 採用。D1〜D8 は実装済み、D9 は既存の仕組みの確認（[進捗](../progress/2026-11-11-cos-triage-scope.md)）。
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
| `credential_input` | login・TOTP・OAuth など credential の入力（credentiald への注入待ち・クラスタの login を含む） |
| `beyond_approval` | 承認（grant・予算・node-h の枠）を超える予算と資源。枠を確かめられないときも含む |
| `objective_change` | task・案件の目的や受け入れ条件の変更 |
| `security_boundary` | credential とセキュリティの境界（秘密の扱い・権限・sandbox の緩和・恒久の認可の新設・ブラウザの承認や credential の待ち） |
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
- 人に残す件を `operator` 宛てにすると API が 422 `cos_reserved_for_human` で断り、経路を取らない（人の判断を operator に渡さない）。
  人に残す件は、D4 の照合が D1 の分類を返す件に加え、人が設定した `human_required`（label・human check・override の待ち、
  integrate task が尽きた統合の依頼）の件。answer を断る判定（`kept_by_person`）と同じ関数で判定する（試験
  `cos_triage_scope_f_human_required_waits_are_not_routed_to_the_operator`）。

付記 2026-11-12（review の差し戻し。operator 宛ての件を人の待ちとして見せない・数えない）: 件の state は人宛てと同じ
`escalated` なので、人が読む場所と評価の記録は経路（`route = 'operator'`）を読んで区別する。状態の正本は経路 1 つで、
新しい item の state は足さない。

- 受信箱 thread の要約（`digest.rs` と control-ts の `cos_digest.ts`。byte 一致）: 判断の行は「外部 operator に回した（人の判断は不要。Discord 通知なし）」。「人が決めること」「選択肢」の行は出ない（operator 宛ては通知の本文（packet）を持たない）。
  card の state は `operator`（`OPERATOR_CARD_STATE`。card は表示の写しで、item の state ではない）。web は `operator` を人待ちに
  数えず（`pendingHumanCount`）、札は「外部 operator の対応待ち」。card は閉じていないので人はそこから答えられる。
  fixture `control-ts/fixtures/cos_digest/cases.jsonl` の `hand/decision/operator/*` が Rust と TS の一致を固定する。
- 論理要求の台帳（ADR 2026-10-27-eval-request-ledger、`celerisctl eval requests`）: 経路 `operator` の区間は `human_waits` でなく
  `operator_waits` に入り、`summary.open_operator_waits` に数える（schema `celeris.eval-requests/2`）。外部 operator は AI の
  運用補助（`docs/ops/external-operator.md`）なので人の待ちに数えない。revision の `route` は `operator` のまま残る。
- operator は `GET /api/v1/cos/inbox?state=escalated` の `addressee = "operator"` の件を拾う（`docs/ops/external-operator.md` §1）。
  人向けの受信箱（`/inbox`）には元の待ちがそのまま出るので、人も答えられる。

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
- 監査の reason は「CoS の判断（<category>）: <reason>」。方針の版は操作の記録の `policy_version` に、grant の id と根拠は
  payload の `basis.refs` に残る。判断者（grant・人の発言の出典）と実行者（CoS）は別々に残る（ADR 2026-10-27-relayed-instructions
  の記録を弱めない）。
  人は既存の取消・差し戻し（override）で覆せる。

### D4 daemon の事前の照合（決定的、LLM なし）

daemon は CoS の run を組むとき、受信箱の件ごとに `task_ops::triage_match::match_item` で次を調べ、件に付けて prompt に出す。
同じ関数を resolve の API も使い、D1 の判定と operator 宛ての拒否に使う（正本は 1 つ）。

- `reserved`: 件の種類と出所から決まる D1 の分類（例: `credential_wait`・`cluster_login` → `credential_input`、
  `browser_wait`（ブラウザの承認・credential の待ち）→ `security_boundary`、計画の承認の理由が `near_limit:` → `beyond_approval`）。
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

- 監査の reason の頭を「定型の代答（<category>）」から「CoS の判断（<category>）」に変えた。`review_rework` の開き直しの回数
  （同じ task で 2 回まで、ADR 2026-10-29-cos-review-rework-reopen）は applied の監査の reason の頭で数えるので、版 12 より前の行の
  「定型の代答（review_rework）」も数える（`basis/review_rework.rs`）。events は追記専用で旧い行を書き換えられず、数えないと
  版の切り替えで上限が外れるため。この読み分けは旧い行を読むためだけのもので、新しい行は旧い文言で書かない。撤去の条件:
  版 12 より前の `review_rework` の applied の行が、開いている task（終端でないもの）に 1 件も残らなくなったとき。撤去の責任は
  この方針の次の版を出す task が持つ。

### D8 答えの要らない件は CoS が見届けで閉じる（task のコメント 1）

`dependents_reopened`（依存先が終端から戻ったので、その失敗で cancel されていた後続を待ちに戻した知らせ）は、選択肢が
戻した後続の cancel だけで、後続を残すのが普通の答え。項目は後続が動き出すか終端になると自然に消える。
`InboxKind::observable_while_open` が真の種類（今は `dependents_reopened` だけ）は、元の項目が開いていても CoS が observe で
閉じてよい（resolve は 422 `cos_observe_needs_judgment` を返さない）。照合（D4）は `observe` の候補を出す。後続を取り消す
（重複した後継を手で起票していた）と判断したときは、従来どおり `task_scope` で cancel を答えるか人に回す。

### D9 release の切り替えで途切れた一次対応（task のコメント 2 の確認）

本番の `dependents_reopened-01M75722QN630BBASEQ759KG4T-735` は、2026-11-11 23:18Z の昇格の時刻に旧い規則
（到着から 120 秒で退避）で人へ送られた。ADR 2026-11-11-cos-fallback-timing（task 01M756VNCZ）の後は、切り替えで止まった
run の件は digest の回収で `pending` に戻り（`requeue_run_tx`）、新しい daemon が次の tick で拾う（D3。試験
`cos_fallback_timing_standby_to_active_picks_on_the_next_tick`・`cos_digest_script_restart_after_failed_requeue`）。
退避は run が件を手放したときか安全網（既定 1200 秒）を超えたときだけ取られる。送信済みの退避の件は同 ADR D2 のとおり人に残り、
CoS は拾い直さない（判定は 409 で記録だけ残る）。この ADR では仕組みを変えない。

## 本番への移行

`~/.config/celeris/config.toml` の `[cos.triage] policy_version` を `"12"` にし、KB の skill を v12 に差し替える
（手順は進捗の「本番への移行」節）。skill の写しは昇格の `promote.sh` が行う（ADR 2026-11-11-cos-skill-sync D2）。
KB の skill の版と `policy_version` が合うまで daemon は CoS の run を起こさないので、昇格から config を `"12"` にするまでの間は
一次対応が止まり、受信箱に 1 件知らせが出る。
