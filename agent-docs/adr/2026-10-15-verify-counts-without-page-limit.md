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
- D1: 件数だけを返す口を 2 本足す。`GET /api/v1/reports/count` と `GET /api/v1/messages/count`。
  応答は `{"count": <表の全行の COUNT(*)>}`（`SqliteStore::report_count_all` / `message_count_all`）。
  絞り込みも limit も持たないので 1 応答の上限に当たることがない。node が org から消えた行
  （orphan）も数えるので、生の行数（snapshot）とそのまま比べられる。
- D2: `verify.sh` の `collect()` は reports・messages をこの 2 口で数える。一覧の items を node・案件ごとに
  数える経路・`LIMIT`・`capped`（"hit the 500-row page limit"）・snapshot の `orphan_reports` /
  `orphan_messages` と不一致時の orphan hint は消す（旧い仕組みを残さない）。
- D3: 上限に当たって数え切れない状態は存在しない。口が失敗した（接続失敗・非 200）ときは `safe()` が
  errors に積み、件数は null になる。counts-match（検査 2）は errors が空でないことと件数の不一致の
  両方で不合格になる（黙って 0 や部分の数にしない）。
- D4: n-1-compat（検査 5）は旧版の API で数える。旧版は D1 の口を持たないので、検査 5 の件数照合は
  `N1_COUNT_KEYS`（tasks・tasks_total・projects・milestones・org・approvals・approvals_decided・
  tasks_digest）だけで行い、reports・messages は照合しない。旧版が migrate 後の reports・messages を
  読めることは `probe_api` の一覧 GET 200 で見る。旧版が口を持つようになった後も同じ扱い
  （版で分岐しない）。
- 試験: `crates/task-api/tests/reports.rs::count_endpoints_count_every_row_beyond_the_page_limit`
  （reports 1723 件・1 node 650 件、messages 1 node 650 件。一覧は 500 で打ち切られ、口は全数を返す）と
  `scripts/selfdeploy/tests` の verify 件数照合試験。
