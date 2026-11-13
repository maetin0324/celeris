# ADR: 入口（Cloudflare など）の一時的な遮断は認証の失敗にしない

- Date: 2026-11-13
- Status: Accepted
- 決定者: 人の依頼（task 01M7A9RJRPEMV9M4M65R0P39PJ）
- 関連: [認証失敗のアカウントは routing から外す](2026-10-17-account-auth-failure.md)（D1・D2 に付記）、ADR-0010 D5

## Context

2026-11-13 10:19Z、claude_max_personal と claude_max_lab の worker run が続けて
`Failed to authenticate. API Error: 403 Attention Required! | Cloudflare` で落ちた。Anthropic の入口の Cloudflare が一時的に返した
challenge のページで、token の失効ではない（11:10Z の確認で OAuth は有効、11:12Z の手動 check は両方 ok）。
`classify_provider_failure` は `failed to authenticate` を含む文を AuthFailed にし、ADR 2026-10-17 D2 により 10 年の cooldown が付いた。
2 アカウントとも外れ、claude_code だけを使う CoS の run が 10:19Z〜11:00Z に 9 回失敗した。

## Decision

D1. **入口の遮断の印**（大文字小文字を無視した部分一致）: `cloudflare` / `cf-ray` / `just a moment` / `attention required` /
    `enable javascript and cookies`。いずれかを含む文は、**入口が返した一時的な遮断**として `Throttled { retry_after_secs: 60 }`
    に分類する（短い cooldown。attempts を消費しない requeue）。

D2. **判定順:** 遮断 → Exhausted → Throttled → AuthFailed。遮断の印を最優先にする。
    理由: 遮断のページは認証の文言（`Failed to authenticate`、`401` 等）を一緒に載せることがあり、認証の印を先に見ると今回の誤分類になる。
    入口が challenge を返した時点で、リクエストは Anthropic の認証に届いておらず、token の状態について何も言えない。
    逆に誤って本物の認証失敗を遮断と見ても、60 秒後の再試行で再び認証失敗が返り、遮断の印が無い文として AuthFailed になる。
    無期限の保持を誤って付ける害（今回）より、短い cooldown を一度誤って付ける害の方が小さい。

D3. **403 だけ**（遮断の印が無い）の文は今までどおり分類しない（403 は認証・権限・遮断のどれかを区別できないため、AuthFailed にも
    Throttled にもしない）。本物の認証失敗（`oauth session expired`、`401`、`not logged in`、`/login` など）は今と同じ AuthFailed。

D4. ADR 2026-10-17 の D1 の分類は、D2 の順序の下でのみ有効とする。旧い「`failed to authenticate` は常に AuthFailed」を前提にした
    試験・文書は置かない。config・DB の変更なし。
