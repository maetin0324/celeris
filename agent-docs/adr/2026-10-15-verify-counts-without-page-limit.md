# verify の件数照合は 1 応答の上限に当てない — reports・messages の件数口（ADR-0058 付記）

- 日付: 2026-10-15
- 状態: 採択（実装中）
- 関連: ADR-0058（release verify の内訳）、ADR 2026-10-11-breaking-changes-by-default

## 背景
release 5c651a490521 の verify が counts-match・n-1-compat で不合格になった
（`reports(snapshot=1723 staging=1708) errors=["reports of software-engineering hit the 500-row page limit"]`）。
`scripts/selfdeploy/verify.sh` は `GET /api/v1/reports?node=<id>&limit=500` と
`GET /api/v1/org/<id>/messages?project=<p>&limit=500` の items を数えていた。1 node の行が 500 を
超えると数え切れず、上限到達を errors に入れて不合格にしていた。messages は `project` でしか
絞れないので、(b) の細分（node × project × level）では messages を数え切れる保証が無い。

## 決定
(a) を採る。
- D1: `GET /api/v1/reports/count?node=<id>` と `GET /api/v1/org/{id}/messages/count[?project=<p>]` を足す。
  応答は `{"count": <SQL の COUNT(*)>}`。limit を持たず、上限に当たることがない。
  messages/count は `project` を省くと node の全行（project 有無を問わず）を数える。
- D2: verify.sh は node ごとに 2 本の count を叩いて合計する。items を数える経路・`LIMIT`・
  `capped`（"hit the 500-row page limit"）は消す（旧い仕組みを残さない）。
- D3: 数え切れない状態は存在しなくなる。count 口が 404 等で失敗したら `safe()` が errors に積み、
  counts-match は不合格になる（黙って 0 にしない）。
- D4: n-1-compat（旧版 API で数える）は旧版に count 口が無い。旧版に対しては count 口の 404 を
  「旧版が口を持たない」として items 経路ではなく生の行数（snapshot）を使う……のではなく、
  n-1-compat は新版の staging と同じ DB を旧版で読めるかを見る検査なので、旧版の件数は
  tasks・projects 等の照合で足り、reports・messages は新版の count だけで照合する。
- 試験: 本番規模（reports 1723 件・1 node 600 件超、messages 1 node 600 件超）の一時 DB で
  verify の counts 部分を回し、snapshot と一致することを示す。
