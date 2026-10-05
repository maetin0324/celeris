---
tasks: [01M46W97H391DSFW1XJ745W0G9]
---

# ブラウザ実行課（browser-execution）の投入手順

対象は本番 `~/.config/celeris` の org（DB が正。`config/org.example.toml` は空 DB の種でしかない）。
実行者は人、または配送後に Fable が API で行う。[ADR 2026-10-05-browser-department-web-live-view](../../agent-docs/adr/2026-10-05-browser-department-web-live-view.md)
D1・D5 に従う。このタスクでは本番の DB・daemon を変更しない。

## 前提

- `CELERIS_API` に稼働中 daemon の base URL（例 `http://127.0.0.1:<api-port>`）、
  `CELERIS_API_TOKEN` に管理系トークン（`token_file` に設定した値）を入れておく。
- `engineering`（parent_id）が既に org に存在すること（本番は既存）。

## 1. 投入（POST）

投入する body は [`browser-department-org.json`](browser-department-org.json)
（`OrgCreateBody` の形。`config/org.example.toml` の `[[org]] id = "browser-execution"` と同じ内容）。

```bash
curl --fail-with-body -X POST \
  -H "Authorization: Bearer $CELERIS_API_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @docs/ops/browser-department-org.json \
  "$CELERIS_API/api/v1/org"
```

`201 Created` と `Location: /api/v1/org/browser-execution` が返る。既に同じ `id` があれば `409`
（`org_node_exists`）になる。その場合は投入済みなので次節の GET で確認するだけでよい。

## 2. 確認（GET）

```bash
curl --fail-with-body "$CELERIS_API/api/v1/org" | \
  python3 -c 'import json,sys; d=json.load(sys.stdin); n=[x for x in d["items"] if x["id"]=="browser-execution"][0]; print(json.dumps(n, indent=2, ensure_ascii=False))'
```

確かめる点:

- `parent_id` が `"engineering"`、`genre` が `"coding"`。
- `profile.browser.allowed_domains` が `["localhost", "127.0.0.1"]`（初期は loopback だけ。
  人の決定 `browser-initial-domains`）。
- `profile.harnesses.allowed` に `"coding"` が入っている（`browser-enabled` task を受けられる）。

`GET /api/v1/org` のレスポンスは `effective_profiles` も返す。`browser-execution` の
`effective_profiles[].browser` が `null` でないことも合わせて見ると、継承後も grant が消えていないと
確認できる。

## 3. 業務 host を足す（PATCH）

初期値は loopback だけなので、実際に操作してよい業務 host を人が決めてから `profile` を**丸ごと**
PATCH する（`profile` は部分更新ではなく置換。D1.2 の他の欄もすべて書き直す必要がある）。

```bash
curl --fail-with-body -X PATCH \
  -H "Authorization: Bearer $CELERIS_API_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @- \
  "$CELERIS_API/api/v1/org/browser-execution" <<'JSON'
{
  "profile": {
    "skills": ["browser-enabled", "browser", "web-automation"],
    "harnesses": { "allowed": ["coding"], "default": "coding" },
    "budget": { "max_lane": "standard", "max_attempts": 2 },
    "browser": {
      "allowed_domains": ["localhost", "127.0.0.1", "wiki.internal.example"],
      "allowed_actions": null,
      "credential_policy_ids": []
    },
    "policy": [
      "ブラウザ操作は grant と task の browser policy の範囲だけで行う。範囲外の origin・操作が要るときは自分で広げず、waits で人に上げる。",
      "人が control lease を持つ間は操作しない。返却されたら現在の画面を読み直してから続ける。",
      "credential・cookie・token をメモ・成果物・会話に書かない。"
    ]
  }
}
JSON
```

`allowed_domains` は `BrowserCapability::validate()`（`crates/task-core/src/browser.rs`）がその場で検証する。
裸の `"*"` や `scheme://` 付き、ポート付きの値は構文として `422` で拒否される
（`valid_host()` は英数・ハイフン・ドット区切りの DNS ラベルのみを受ける）。

`credential_policy_ids` を足す場合も同じ PATCH で `browser.credential_policy_ids` に id を追加する
（policy 自体は別途登録してから参照する）。

## 4. 戻し方

grant を外す（この課に browser task を割り当てないようにする）には、`profile.browser` を省略した
`profile` で PATCH する（`profile` は丸ごと置換なので、`browser` を書かなければ `None` になる）。
node 自体を削除する場合は:

```bash
curl --fail-with-body -X DELETE \
  -H "Authorization: Bearer $CELERIS_API_TOKEN" \
  "$CELERIS_API/api/v1/org/browser-execution"
```

## 5. seed（`config/org.example.toml`）との関係

`config/org.example.toml` は空 DB に対する種でしかなく、稼働中の本番 DB には効かない。本番へは
上記の POST/PATCH で反映する。seed ファイルの更新は、新規に `org_include` する環境（開発用の
使い捨て DB など）向けの既定値を合わせておくためのものであり、本番の反映作業とは別である。

## 変更の記録

`POST /api/v1/org` と `PATCH /api/v1/org/{id}` の呼び出しは、操作ログ（`tracing::info!`）に
`who = "admin"` で残る。actor 付きの `Event`（`Event::OrgNodeUpdated`）による変更履歴の記録は
この WorkUnit の範囲外であり、follow-up task として起票する（ADR D5.3）。
