# ADR: 認証失敗（OAuth 切れ）のアカウントは routing から外し、task を失敗にしない

- Date: 2026-10-17
- Status: Accepted
- 決定者: 人の依頼（task 01M544JTS13PDN3EJHTDE4YX97）
- 関連: ADR-0010 D5 / ADR-0011（供給側失敗の requeue）、ADR-0024 D3〜D6（アカウントプール・cooldown・check）、
  ADR-0054 D2（reviewer の失敗回数）、[破壊的変更を是とする](2026-10-11-breaking-changes-by-default.md)

## Context

2026-10-16 22:40Z ごろ `claude_max_personal` の OAuth が切れ、claude は
`Failed to authenticate: OAuth session expired and could not be refreshed` を返した。次の 3 つが重なった。

1. `classify_provider_failure` の AuthFailed の語（`authentication` / `not logged in` / `/login` / `invalid api key` / `401`）に
   この文面が当たらず、**未分類**になった。`POST .../check` は「分類できないが result 行はある」を `ok` とするので
   `result="ok"`（失敗文は detail だけ）を返し、run は「reviewer run 自身のインフラ都合の失敗」になって cooldown が付かず、
   router が同じアカウントを選び続けた。
2. 分類できても、AuthFailed の cooldown は `now + fallback_cooldown_secs`（既定 300 秒）で失効し、ログインしていないのに
   5 分ごとにまた選ばれる。
3. 供給側失敗の requeue は `max_requeues`、reviewer のインフラ失敗は `max_reviewer_retries` で数えるため、認証が直らない間に
   上限へ達して task が failed になり、依存する子と親が `dependency_failed` で cancelled になった。

## Decision

D1. **分類:** AuthFailed の語に `failed to authenticate` / `oauth session expired` / `oauth token has expired` /
    `could not be refreshed` を足す。check・worker run・reviewer run は同じ `classify_provider_failure` を通るので、
    check は `result="auth_failed"`（200 の本文）を返し、`GET /llm/accounts` の `last_check` に載る。

D2. **保持:** アカウントの AuthFailed cooldown は `AUTH_FAILED_HOLD_SECS`（10 年＝実質無期限）。戻す経路は 2 つだけ:
    check の成功（`record_account_check("ok")` が cooldown を消す）と再ログインの成功（`record_account_login`。claude の
    `login/code` 成功と codex の device login 完了で呼ぶ）。source の暗黙アカウント（API 型）の AuthFailed は従来の
    `source_error_cooldown` のまま（check・login の経路が無いため。無期限にしない）。

D3. **一覧:** 認証失敗で保持中のアカウントは `AccountLive.logged_in = false`（鍵ファイルの有無 && 保持なし）。
    `excluded_reason` は評価順の先頭の `cooldown`、`cooldown.reason = auth_failed`、`last_check.result = auth_failed`。

D4. **回数を消費しない:** AuthFailed は worker run の `max_requeues` にも reviewer run の `max_requeues` /
    `max_reviewer_retries` にも数えず、attempts を消費しない。アカウントは routing から外れているので、別アカウントがあれば
    そちらで再試行され、無ければ ready のまま再ログインを待つ。

D5. **通知:** アカウントが認証失敗で外れた遷移ごとに 1 回、`NoticeKind::BadNews`（group `bad_news:account:<adapter>:<id>`）を
    記録する（受信箱・Discord の既存経路）。保持中の再失敗・再 check では重ねない。

D6. 旧い挙動（AuthFailed の 300 秒失効、AuthFailed への requeue 上限の適用）は同じ変更で消す。互換 flag は置かない。

## 試験

- `dispatcher/tests/account_auth_failure.rs`: 外れて別アカウントへ回る・一覧への反映・通知 1 回・check 成功/再ログインで復帰・
  唯一のアカウントが外れても task が failed にならず再ログイン後に完了（`max_requeues = 0` でも）。
- `dispatcher/tests/review.rs::reviewer_auth_failure_does_not_consume_the_requeue_limit`
- `task-worker`: 分類（`provider/tests.rs`）と check（`claude_account/tests.rs`）。実 OAuth には触れない。

## 本番への移行

config・DB の変更なし。deploy 後、`claude_max_personal` は `claude auth login` 相当の再ログイン（GUI の login）か
check の成功まで外れたまま。
