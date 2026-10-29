# ADR 2026-10-28: 承認済みの枠の内側の定型の判断を CoS が根拠つきで代答する

---
tasks: [01M60AMT8P6FQ5QX64ANRZ588S]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-06-cos-inbox-triage](2026-10-06-cos-inbox-triage.md)（受信箱の一次対応の store 契約。本 ADR を付記）、
  [2026-10-27-relayed-instructions](2026-10-27-relayed-instructions.md)（取り次ぎ。本 ADR を付記）、
  [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D3（answer / observe / escalate と人に回す基準）、
  [2026-10-07-cos-inbox-thread-conversation](2026-10-07-cos-inbox-thread-conversation.md) D3（`instructed_by`）、
  ADR-0079 D7（決定の種類と `raise-once`）

## 背景

利用者（2026-10-28）は「外部の Claude Code セッションでしか対応できないもの以外は CoS から対応できるようにしたい。
inbox の対応なども CoS に判断できるはず」と依頼した。外部 operator が 10-01〜10-27 の自分の操作を棚卸しすると、
operator が代答した判断のうち次の 5 件は、判断材料（commit・log・進捗・承認済みの枠）が CoS にも読めた。

| 日付 | 判断 | 種類 |
|---|---|---|
| 10-25 | d-release に推奨どおり post-merge（event 702907） | worker の推奨と既定の一致 |
| 10-20 | 01M5BTS6VK の予算延長（進捗を commit で確かめてから） | 承認済みの枠の内側の続行 |
| 10-19 | 多軸評価 2a の replan 上限の一度だけの引き上げ | 上限の一度だけの引き上げ |
| 10-13 | AP の replan 上限 raise-once | 上限の一度だけの引き上げ |
| 10-22 | 01M5HHB88W を文書化した既定値で進める | worker の推奨と既定の一致 |

CoS はどれも `cos-inbox-triage`（policy_version 4）の `resource_overrun`・`fundamental_change` に当たるとして escalate した。
skill の表は「上限超過/引上げ」を一律に人へ回し、「定型選択」の行は根拠の形を決めていない。API は answer の
`reason` が空でないことしか見ないので、「定型だから」で代答できる一方、代答してよい範囲を CoS が自信を持って使えない。

## 決定

### D1. CoS が代答してよい定型の判断は 4 種類だけ（`basis.category`）

人の取り次ぎ（`instructed_by`）の無い代答は、次の 4 種類のどれかに当たり、その条件を下の D2 の形で示せるときだけ許す。
どれにも当たらない判断は escalate する。

| category | 何を代答するか | 決定的な条件（API が検査する） |
|---|---|---|
| `within_approved_frame` (a) | 人が承認済みの資源・予算・node-h の枠の内側での続行 | 対象は decision・authorization・plan gate・phase gate。`frame` が人の承認（standing rule・人が答えた decision・決定済みの approval）を指し、その承認が store に在る。`used + requested <= limit`（数値は有限、`limit > 0`・`used >= 0`・`requested > 0`）。`refs` に使用量の出典（`usage` か `ledger`）が 1 件以上ある |
| `raise_once` (b) | replan・retry 等の木の上限の一度だけの引き上げ | 対象は kind=limit の決定で、答えは `raise-once`。`refs` に進捗の出典（`commit` か `artifact`）が 1 件以上ある。`failures.previous` と `failures.current` が空でなく、空白を詰めて比べて異なる（同じ失敗の繰り返しでない）。同じ task で CoS が `raise-once` を答えたことが無い（task の events の `DecisionAnswered{option:"raise-once", by: cos…}` が 0 件） |
| `documented_default` (c) | worker の推奨が文書化済みの既定値・既存 ADR と一致する選択 | 元の待ちに推奨（`recommended`）があり、答えの option がそれと一致する。`refs` に `adr`（`agent-docs/adr/*.md`）か `doc`（`docs/**.md`・`agent-docs/**.md`）が 1 件以上ある。`raise-once` には使えない |
| `work_guidance` (d) | 受け入れ条件・目的を変えない範囲の作業方針の補足 | 対象は worker の自由質問（kind=question）。答えは本文（note）があり、option は省略か `answer`（`replan` は使えない。計画・受け入れ条件を変えうるため） |

共通の条件:

- `raise-once` を答えるのは `raise_once` だけ。他の category で `raise-once` を選ぶと 422。
- `frame` は `within_approved_frame` だけ、`failures` は `raise_once` だけが持つ。他の category に付けると 422。
- 既存の拒否はそのまま先に効く: 明示の `human_required`（human check・task の `human_required` label・人の override 待ち）は 403、
  元の問いの選択肢の検証・revision・confidence の扱いも変えない。

人に回すもの（従来どおり escalate）: 目的・受け入れ条件の変更、枠を超える資源（`used + requested > limit` を含む）、
外部公開・外部送信、不可逆・破壊的操作、秘密・権限・セキュリティ・認証、恒久の認可（standing rule）の新設、人が「人の判断」と
指定したもの、上の 4 種類の条件を満たせないもの、confidence が `min_confidence` 未満のもの。

### D2. 根拠の形（`basis`）と API の検査

`POST /api/v1/cos/inbox/{i}/resolve`（outcome=answer）と、`POST /api/v1/cos/operations` で待ちに答える操作
（`decision.answer`・`approval.decide`・`execution.phase_gate`・`execution.plan_gate`・`question.answer`・`inbox.answer`）のうち
`instructed_by` の無いものは、body に `basis` を必須とする。

```json
"basis": {
  "category": "within_approved_frame",
  "refs": [{"kind": "usage", "id": "GET /api/v1/tasks/01M5BTS6VK/usage 2026-10-20T03:00Z"}],
  "frame": {"kind": "decision", "id": "<decision id>", "unit": "tokens", "limit": 1500000, "used": 1360000, "requested": 100000}
}
```

- `refs[]`: `{kind, id}` を 1〜20 件。kind は `standing_rule`・`decision`・`approval`・`usage`・`ledger`・`commit`・`artifact`・
  `progress`・`adr`・`doc`・`message`。`commit` は 7〜40 桁の 16 進、`adr`・`doc`・`progress` はリポジトリ内の `.md` の相対 path
  （`..` を含まない）。
- `frame`: `{kind: standing_rule|decision|approval, id, unit, limit, used, requested}`。
- `failures`: `{previous, current}`（各 1〜500 文字）。
- 欠け・形の誤り・条件の不一致は 422 で、監査に rejected として残る（`cos_operations` の行と `CosOperation` event）。
  problem code は `cos_basis_missing`（basis が無い）、`cos_basis_invalid`（形・対象・出典の誤り）、`cos_basis_over_frame`
  （枠を超える）、`cos_basis_repeated_failure`（同じ失敗の繰り返し）、`cos_basis_raise_once_used`（同じ task で 2 回目）。
- 通った代答は、監査の reason を「定型の代答（<category>）: <CoS の reason>」とし、payload に `basis` を残す。
  `instructed_by` の付いた操作は人の判断なので `basis` を要らない（付けても検査しない。payload にも入れない）。

### D3. policy_version 6 と `[cos.triage] human_required` の分類

- `cos-inbox-triage` skill を D1・D2 に合わせて書き直し、`metadata.version` と `[cos.triage] policy_version` の既定を "6" にする（版 "5" は同じ日の [2026-10-28-single-human-window-general-cos](2026-10-28-single-human-window-general-cos.md) D6 が使った）。
- `human_required`（escalate する理由の分類。CoS への指示の語彙）を D1 の境界に合わせて改める。
  既定は `objective_change`・`beyond_frame`・`external_publish`・`destructive`・`secret_or_security`・`standing_permission`・`explicit_human`。
  旧い `fundamental_change`・`resource_overrun`・`security` は削り、config に書かれていれば読み込みエラーにする
  （未知の分類も読み込みエラー）。`resource_overrun` は「上限超過/引上げ」全体を指したが、新しい `beyond_frame` は枠を超える
  ものだけを指す（枠内の続行と一度だけの引き上げは D1 の代答）。
- 評価契約（`docs/ops/mlsys-main-4-ops-r1.md` §8 方針の版の変更記録）に "5" → "6" を追記する（窓口の "4" → "5" とは別の行）。

## 検証

- fake harness（CoS run の credential で API を呼ぶ試験）と fixture で、(a)〜(d) の代答が通り、監査の reason・payload に basis が残る。
- basis の無い代答、枠を超える (a)、同じ失敗の (b)、2 回目の (b)、推奨と違う (c)、`replan` を選ぶ (d) は 422 で rejected に残る。
- `instructed_by` の付いた操作は basis 無しで通る。
- config の旧い分類名は読み込みエラー。
- `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`。

## 付記 A1（2026-10-28）: 統合の依頼の `integrate_task` は documented_default で代答する

ADR 2026-10-28-cos-integrate-task D4 は、統合の依頼を CoS が `integrate_task` で答えると決めた。D1 の 4 種類に載せるため、
`integrate_task` を選べる統合の依頼（origin `delivery`、結び無し、escalated でない）の受信箱項目は `recommended` を
`integrate_task` にする。CoS はこれを `documented_default`（`refs` に `agent-docs/adr/2026-10-28-cos-integrate-task.md`）で
代答する。basis の無い `integrate_task` の答えは他と同じく 422（`cos_basis_missing`）。escalated の依頼は従来どおり
`human_required_reason` で 403 になり、推奨も選択肢も出さない。

## 付記 A2（2026-10-29）: 5 種類目 `review_rework`

[2026-10-29-cos-review-rework-reopen](2026-10-29-cos-review-rework-reopen.md) が、reviewer の技術的な差し戻しで failed になった
task の開き直し（失敗の件への `reopen`）を 5 種類目 `review_rework` として足した。D1 の「4 種類だけ」はこの付記で 5 種類になる。
refs の kind に `review`（差し戻した reviewer run の id）を足し、problem code に `cos_basis_rework_limit` を足した。policy_version は "7"。

