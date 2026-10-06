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
- `profile.browser.allowed_domains` が `["http://localhost:3000", "http://127.0.0.1:3000"]`
  （初期は loopback だけ。人の決定 `browser-initial-domains`。投入時の値は origin として正規化されて
  保存される）。
- `profile.harnesses.allowed` に `"coding"` が入っている（`browser-enabled` task を受けられる）。

`GET /api/v1/org` のレスポンスは `effective_profiles` も返す。`browser-execution` の
`effective_profiles[].browser` が `null` でないことも合わせて見ると、継承後も grant が消えていないと
確認できる。

## 3. 業務 host を足す（browser-settings）

初期値は loopback だけなので、実際に操作してよい業務 host を人が決める。browser 欄の編集は
`PATCH /api/v1/org/{id}/browser-settings`（実装: `crates/task-api/src/handlers/org.rs` の
`patch_browser_settings`）で行う。

- 管理トークンで `admin` 権限が必要。
- 対象ノードが `profile.browser`（browser grant）を持たないと `422`（`browser: node has no browser grant`）。
- 本文は `BrowserSettingsPatch`（`deny_unknown_fields`）の**フラットな形**。browser grant だけでなく
  `harnesses`・`budget` も同じ本文で置き換えられる。書いた欄だけが置き換わり、`profile` の他の欄
  （`skills`・`policy` 等）は触れない。

```bash
curl --fail-with-body -X PATCH \
  -H "Authorization: Bearer $CELERIS_API_TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @- \
  "$CELERIS_API/api/v1/org/browser-execution/browser-settings" <<'JSON'
{
  "allowed_domains": ["http://localhost:3000", "http://127.0.0.1:3000", "https://wiki.internal.example"],
  "harnesses": { "allowed": ["coding"], "default": "coding" },
  "budget": { "max_lane": "standard", "max_attempts": 2 },
  "credential_policy_ids": [],
  "credential_identity_ids": {}
}
JSON
```

成功時は `200` で更新後のノードが返る。`allowed_domains` だけ足す場合は
`{"allowed_domains": ["…"]}` 1 欄だけでもよい。

`allowed_domains` の各値は `AllowedOrigin::parse`（`crates/task-core/src/browser/origin.rs`）で検証され、
不正なら `422`（`browser.allowed_domains: expected valid browser origins`）。許される形は次の通り。

- origin 形 `scheme://host[:port]`。port 省略は HTTPS 443 / HTTP 80。
- 外向きは HTTPS のみ。HTTP は `localhost`・`127.0.0.1`・`[::1]`（loopback）だけ。
- wildcard は `https://*.example.com` の形。public suffix（`com` 等の単一ラベル、2 レベルの ccTLD、
  `github.io` 等）を覆う wildcard と loopback・IPv4 への wildcard は拒否。userinfo・path・query・fragment
  を含める値は拒否。
- 旧形の裸 host（`wiki.internal.example` 等）は読み込み時のみ HTTPS 443 として読める
  （`parse_allowed_origin`）。新規の書き込みは origin 形で行う。

`credential_policy_ids` を足す場合も同一 endpoint で `credential_policy_ids` に id を書き、
`credential_identity_ids` に policy ID → identity ID の対応を入れる（policy 自体は別途登録してから
参照する。値は ID のみで、秘密は書かない）。

browser 設定の変更は `tracing::info!`（`op = "org_patch"`）に加え、`org_browser_events`
（migration `0051`）に actor と変更前後のスナップショットが同 transaction で残る
（`crates/task-core/src/store/org.rs` の `org_upsert_browser_settings`）。

`profile` の丸ごと置換（`skills`・`policy` 等を含む）が必要なときは、通常の
`PATCH /api/v1/org/browser-execution`（`OrgPatchBody`、本文は `{"profile": {…}}`）を使う。
その場合も `profile.browser.allowed_domains` は origin 形を渡すこと。

## 4. 戻し方

grant の**縮小**は §3 の `PATCH /api/v1/org/browser-execution/browser-settings` でよい。
grant を外す（この課に browser task を割り当てないようにする）のは browser-settings ではできない
（node に browser grant が無いと `422`）。`profile.browser` を省略した `profile` で
`PATCH /api/v1/org/browser-execution` する（`profile` は丸ごと置換なので、`browser` を書かなければ
`None` になる）。
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
`who = "admin"` で残る。browser 設定の変更（§3）は `tracing::info!` ではなく
`org_browser_events`（migration `0051`）に actor と変更前後のスナップショットを同 transaction で残す
（読み取り API は無い。ADR 2026-10-05 付記）。actor 付きの `Event`（`Event::OrgNodeUpdated`）による
変更履歴の記録は ADR D5.3 の follow-up task の範囲であり、まだ実装されていない。
