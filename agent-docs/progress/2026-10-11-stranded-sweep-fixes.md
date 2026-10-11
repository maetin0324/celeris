---
title: 取り残し検出の不備を直す（default branch の一本化・遠隔 repo・受信箱 item の同一性）
updated: 2026-10-11
status: done
tasks: [01M4NM5720BYCYHXT02297GFKK]
---

# 取り残し検出の不備を直す

設計決定は [ADR: done task の変更を取り残さない](../adr/2026-10-11-stranded-done-branches.md) の「付記: 取り残し検出の不備（2026-10-11 本番 deploy）」（F1〜F3）を参照。

完了日: 2026-10-11

## 目的

2026-10-11 の本番 deploy（50f349d84fea）で取り残し sweep が done root task 218 件を誤検出した。

- default branch NULL の 180 件（うち 175 件は取り込み済み）: 解決経路を `task_ops::changes::resolve_default_branch` の 1 関数にまとめ、sweep・delivery ほか全呼び出し元が使う（F1）。
- 遠隔 repo の 38 件: sweep は ssh せず `not_applicable` にし、受信箱に出さない（F2）。
- 5 分ごとに item が新着に見える: 状態が同じなら行を書かず、item の `at`・id を変えない（F3）。

## 段の一覧

| 段 | unit | 内容 |
|---|---|---|
| design | adr | ADR 付記と本進捗ファイル（コード変更なし） |
| build | resolve | `resolve_default_branch` を作り、旧 `default_branch`・sweep の個別判定・`"main"` 埋めを消し、全呼び出し元を移す |
| build | remote | `not_applicable` 状態（migration の CHECK・sweep・API・celerisctl） |
| build | identity | 状態が変わらない再 sweep で行を書かない |
| build | web-state | web の統合状態表示に `not_applicable` を足す |
| close | verify | 全体検証（test-parallel・clippy）・ADR 付記と実装の突き合わせ・本番への移行節の仕上げ |

葉の記録は `agent-docs/progress/2026-10-11-stranded-sweep-fixes/<unit key>.md` に書く。

## 証拠

最終検証（na-event 統合後 HEAD `ca0dd20b`、final-verify 段）の結果。詳細は [final-verify.md](2026-10-11-stranded-sweep-fixes/final-verify.md)（前段の verify.md は `00e3dcb0` 時点）。

| コマンド | exit | 要点 |
|---|---|---|
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | nextest 5178 run / 5178 passed / 13 skipped、doctest 3 passed（JSON 集計 5181 passed・0 failed） |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告無し |
| `cargo fmt --all -- --check` | 0 | 差分無し |
| `sh scripts/dev/check-doc-links.sh` / `check-adr-numbers.sh` / `progress-index.sh --check` / `check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | いずれも ok |
| `cargo nextest run --workspace -E 'test(stranded_)'` | 0 | 24 passed |

- 葉の記録: [adr.md](2026-10-11-stranded-sweep-fixes/adr.md)・[resolve.md](2026-10-11-stranded-sweep-fixes/resolve.md)・[remote.md](2026-10-11-stranded-sweep-fixes/remote.md)・[identity.md](2026-10-11-stranded-sweep-fixes/identity.md)・[web-state.md](2026-10-11-stranded-sweep-fixes/web-state.md)・[na-event.md](2026-10-11-stranded-sweep-fixes/na-event.md)・[verify.md](2026-10-11-stranded-sweep-fixes/verify.md)・[final-verify.md](2026-10-11-stranded-sweep-fixes/final-verify.md)
- ADR 付記の「実装との突き合わせ（F1〜F3、2026-10-11 close 段）」に関数名・migration 0070・試験名（`stranded_default_branch_*` 6 件・`stranded_remote_*` 3 件・`stranded_identity_*` 1 件・`stranded_audit_*` 1 件）と決定からのずれを書いた。F2 の event のずれは na-event 葉で実装して解消した（commit `eb7968ef`、試験 `stranded_audit_remote_not_applicable_appends_one_event_and_no_repeat`）。
- close 段の record は agent-docs/ だけを変えた（`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` は exit 0）。

## 未解決事項

- （解消）`unintegrated` → `not_applicable` の遷移の event 欠落は na-event 葉で解消（`integration_not_applicable` を追記、commit `eb7968ef`、試験 `stranded_audit_remote_not_applicable_appends_one_event_and_no_repeat`）。
- ADR F2 の文言のずれ（人の判断が要る・ADR 本文か実装のどちらを直すか）: 表の名前（決定 `task_repo_integration_states`／実装 `task_integrations`）、remote の reason 文字列（決定 `remote repository is not checked by the stranded sweep`／実装 `remote repository: not checked by the sweep`）。event 以外は監査要件に当たらないため、文言の改訂は後続に回す。
- task-api docs と task-dispatch behind_target の解決失敗時の振る舞いが ADR F1 の表と少し違う（ADR 付記に記載）。
- `default_branch_unresolvable`（409）が `docs/api/v1/api.md` のエラーコード表に無い。
- repos 空の task で Remote の repository に branch がある場合は記録しない（`not_applicable` 行もできない）。
- test-parallel の要約が nextest の `passed (N slow)` を数え落とす既知の問題（本件の範囲外、未修正）。
- web の mobile 幅の overlay 試験は `not_applicable` の section を含まない。

## 本番への移行

本番 host の操作はすべて人が行う。以下は人が実行する手順。

1. **release と DB**: 通常の release 手順（release → verify → promote）で deploy する。daemon 起動時に migration `0070_task_integration_not_applicable.sql` が適用され、`task_integrations` を CHECK に `not_applicable` を足した表へ作り直して全行を写す（行は失わない）。通常どおり事前に DB の backup を取る。適用の確認:
   ```sh
   sqlite3 -readonly <本番 DB> "SELECT max(version) FROM schema_migrations;"   # 70 以上
   sqlite3 -readonly <本番 DB> "SELECT state, count(*) FROM task_integrations GROUP BY state;"
   ```
   （表名・列名は本番の schema に合わせる。読み取りだけ。deploy 前に同じ集計を取って件数の合計が変わらないことを見る）
2. **default_branch=main は残す**: 運用セッションが `PATCH /api/v1/repos/01M2XPE3NZ1FZE061FKDY4A9PK` で入れた agent-platform の `default_branch=main` は戻さない。`resolve_default_branch` は設定を最優先で使い、結果は同じ `main`。
3. **初回 sweep の確認**（deploy 後、sweep 周期 5 分を待つ）: benchfs など遠隔 repo の `unintegrated` 38 行が `not_applicable` に移り、受信箱から消える。
   ```sh
   celerisctl integration list --state unintegrated --json   # deploy 前の件数から 38 減る（遠隔 repo の行が 0）
   celerisctl integration list --state not_applicable --json # 38 件、repo は benchfs 等、reason "remote repository: not checked by the sweep"
   curl -s -H "Authorization: Bearer $CELERIS_ADMIN_TOKEN" http://<api>/api/v1/inbox \
     | python3 -c 'import json,sys; a=json.load(sys.stdin)["attention"]; print(sum(1 for i in a if i["type"]=="unintegrated"))'
   ```
   受信箱の `unintegrated` 件数が `--state unintegrated` の件数と一致し、遠隔 repo の task が含まれないこと。
   sweep の遷移は event も追記する（`integration_not_applicable`、行 1 件につき 1 件）。読み取り専用で数える（events の `json` 列の `type` で絞る。`json` は serde の tag 付き JSON）:
   ```sh
   sqlite3 -readonly <本番 DB> "SELECT count(*) FROM events WHERE json_extract(json,'$.type')='integration_not_applicable';"   # 初回 sweep 後に 38 件増える（deploy 前は 0）
   ```
   38 件が追記されていること（既に同じ行の再 sweep で増えないこと: 2 回目以降の sweep の後も件数が 38 のまま）を確かめる。events は追記専用なので、この確認で書き換え・削除はしない。
4. **再 sweep で新着にならない**: 2 回目以降の sweep の後、同じ `unintegrated` item の id と `at` が変わらないこと（上の `/api/v1/inbox` を 5 分以上あけて 2 回取り、`unintegrated` item の `at` を比べる）。
5. **rollback**: release の戻し（selfdeploy の rollback）で行う。ただし migration 0070 の適用後は DB の版数が 70 になり、0070 を知らない前の版の binary は `SchemaTooNew`（ADR-0013 D5）で DB を開かない。前の版へ戻すときは、手順 1 で取った deploy 前の backup の DB と組で戻す（backup 以降の events・状態は失われるので、人が判断する）。DB を手で書き換えない。0070 を含む版どうしの戻しなら DB はそのまま使える。

## 提案

- `docs/api/v1/api.md` のエラーコード表に `default_branch_unresolvable`（409）を足す葉を後続で置く（文面は [resolve.md](2026-10-11-stranded-sweep-fixes/resolve.md) の提案）。
- ADR F2 の表の名前・reason 文字列を実装に合わせて ADR 本文を直すか、実装を決定に合わせるかを人が決める（event の追記は na-event で実装済み）。
