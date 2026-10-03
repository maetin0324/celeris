---
tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]
---
# 定期実行（cron job）基盤と daily-curation — 運用手順（人が実行する）

- 対象: [ADR-0131](../adr/0131-cron-jobs.md)（汎用の定期実行基盤、最初の job = 知識ベースと受信箱の日次整理）。
  API の形は [docs/api/cron-jobs.md](../api/cron-jobs.md)。セルフデプロイの一般手順は
  [docs/selfdeploy.md](../selfdeploy.md)（ここでは重複させず参照する）。
- 実行者: 人。本番 host の操作（`~/.config/celeris` の編集、daemon の再起動・昇格）はここに書いた手順どおり
  人が行う。このリポジトリの run からは本番 DB・本番 `~/.config/celeris`・`~/.local/celeris` に書き込まない。
- migration: `0046_cron_jobs.sql`（`cron_jobs` / `cron_job_runs` 2 表）、`SCHEMA_VERSION = 46`。

## 0. 前提

- `GET /health` の `schema_version` が 39 以上になる release を [selfdeploy.md](../selfdeploy.md) の
  `release.sh <ref>` → `verify.sh <sha12>` → `promote.sh <sha12>` で本番に昇格させてあること。
- `config.toml` は selfdeploy のどのスクリプトも書き換えない（上記 doc §0）。この job を使うための
  2 つの設定（`[[harnesses]] knowledge-curation` と、使うなら `[[cron.seed]] daily-curation`）は
  **人がこの手順で `~/.config/celeris/config.toml` に足す**。

## 1. `knowledge-curation` harness を本番 config に足す

cron job 自体は harness が無くても作れるが、この job の雛形は `harness = "knowledge-curation"`
（`genre` として task に入る）を指すので、本番 `config.toml` にこの harness が無いと **task の作成・発火時に
`422 unknown genre: "knowledge-curation"` で失敗する**。`config/celeris.example.toml` の該当ブロックを
そのまま本番 `config.toml` の `[[harnesses]]` の並びに追記する:

```bash
sed -n '/^\[\[harnesses\]\]$/,/^$/{/id = "knowledge-curation"/,/^$/p}' \
  ~/workspace/agent-platform/config/celeris.example.toml
# 出力をレビューしてから ~/.config/celeris/config.toml の [[harnesses]] の並びに追記する（手で貼る。
# sed では書き換えない — config.toml は人が確かめて書く対象）
```

追記後、設定の妥当性だけ手元で確かめる（本番には触れない）:

```bash
~/.local/celeris/releases/<sha12>/bin/celerisctl cron --config ~/.config/celeris/config.toml list
# [api] listen に繋がり、まだ job が無ければ {"items":[]} が返ればよい。
# 設定の構文・harness 検証に失敗していれば起動せずエラーが出る。
```

設定ファイルの反映には daemon の再起動が要る（config は起動時にしか読まない。`systemctl --user restart
celeris@<sha12>` は昇格の一部として既に行っている手順に含まれる。harness 追記だけなら
`systemctl --user restart celeris@<sha12>` で反映する）。

## 2. job を作る

`[[cron.seed]]` は **`cron_jobs` 表が空のときだけ**起動時に一度だけ投入される（ADR-0131 D6）。この運用では
明示的に `celerisctl cron create` で作るほうが確実（seed を使うと config.toml にも project 等を正しく
埋める必要があり、取り消しづらい）。`celerisctl` の `--config` は cron サブコマンドの引数なので、以降は
各コマンドに指定する。

```bash
export CELERISCTL="$HOME/.local/celeris/releases/<sha12>/bin/celerisctl"
export CELERIS_CONFIG="$HOME/.config/celeris/config.toml"
export PROJECT=<agent-platform 案件の ULID>   # celerisctl project list / GUI で確認する

cat > /tmp/daily-curation-template.json <<'JSON'
{
  "title": "日次整理: {date}",
  "harness": "knowledge-curation",
  "lane": "cheap",
  "project": "__PROJECT__",
  "mode": "dry_run",
  "acceptance": [
    {"type": "artifact_exists", "name": "curation-plan.json"},
    {"type": "artifact_exists", "name": "curation.diff"},
    {"type": "artifact_exists", "name": "daily-summary.md"}
  ],
  "objective": "知識ベースと受信箱の日次整理（ADR-0131 D6・付記 D10）。本番 KB は書かず、作業場所の写しだけを編集する。"
}
JSON
sed -i "s/__PROJECT__/${PROJECT}/" /tmp/daily-curation-template.json

$CELERISCTL cron --config "$CELERIS_CONFIG" create \
  --name daily-curation \
  --schedule "30 4 * * *" \
  --timezone Asia/Tokyo \
  --overlap skip \
  --catch-up latest \
  --enabled false \
  --template "$(cat /tmp/daily-curation-template.json)"
```

- `schedule` / `timezone` は環境に合わせて変える（本番の運用で都合のよい時刻にする）。
- `--enabled false` で作る（本番で有効にするのは §4 の確認後）。
- `mode = "dry_run"` のまま作る（本番 KB には書き込まない）。`mode` は `CronTaskTemplate` の `extra`
  〈`#[serde(flatten)]`〉で持つ欄だが、JSON の上では `title`/`harness` 等と同じ階層に書く
  （`{"extra": {"mode": ...}}` ではない）。`422` が出た場合は雛形の
  `acceptance` / `harness` / `project` のどれかを見直す（エラーメッセージに理由が出る）。

## 3. 作った job を確認する

```bash
$CELERISCTL cron --config "$CELERIS_CONFIG" list
$CELERISCTL cron --config "$CELERIS_CONFIG" show daily-curation
```

`next_fire_at` が `null`（`enabled=false` のあいだは発火しない）であることを確認する。

## 4. dry-run を手動で 1 回確かめてから有効にする

```bash
$CELERISCTL cron --config "$CELERIS_CONFIG" run daily-curation     # trigger=manual で 1 回だけ発火させる（enabled=false でも効く）
$CELERISCTL cron --config "$CELERIS_CONFIG" history daily-curation --limit 5
```

`outcome = created` と `task_id` が返ったら、その task を GUI（`/tasks/<task_id>`）か
`celerisctl task show <task_id>` で追い、終端になったら報告の `daily-summary.md` を読む。見るもの:

1. `_inbox` の処理件数・重複統合・古い記述の削除件数（ADR-0131 D8 の固定節）。
2. 削除したページの一覧と理由（`curation.diff` で実差分も確認できる）。
3. 受信箱の片付け提案（D7 で外れなかった判断要項目）。
4. 人への decision（`user/` 配下のページの扱いなど）。

**この dry-run は本番 KB を書き換えない**（`mode=dry_run` は `inputs/kb/` の写しだけを編集し、
`curation-plan.json` を本番へ適用しない）。差分が狙いどおりか人が確認する。

GUI での確認: `/cron` に一覧（有効/停止・次回・最後の結果）、`/cron/daily-curation` に履歴と各 run の
task への link がある。

## 5. 定常運用で有効にする

dry-run の差分を確認したら、通常運転として有効化する（引き続き `mode=dry_run`。本番 KB への書き込みは
まだしない）:

```bash
$CELERISCTL cron --config "$CELERIS_CONFIG" resume daily-curation
$CELERISCTL cron --config "$CELERIS_CONFIG" show daily-curation   # next_fire_at が入ることを確認
```

以後は daemon の tick が schedule どおりに task を作る（LLM 呼び出しは daemon に無い。task は通常の
routing・計画・review を通る）。履歴は `cron history` / GUI `/cron/daily-curation` でいつでも追える。

## 6. `dry_run` から `apply` へ切り替える（本番 KB を書き換えさせる）

複数回の dry-run 報告を見て、削除・統合の判断が妥当だと人が判断してから行う（ADR-0095 付記 D-d、
人の決定が要る不可逆に近い操作）。`PATCH` の `template` は丸ごと置き換えなので、現在の雛形を取得して
`mode` だけ書き換えて送り返す:

```bash
$CELERISCTL cron --config "$CELERIS_CONFIG" show daily-curation | python3 -c '
import json, sys
job = json.load(sys.stdin)
tmpl = job["template"]
tmpl["mode"] = "apply"
print(json.dumps({"template": tmpl}))
' > /tmp/daily-curation-apply.json

$CELERISCTL cron --config "$CELERIS_CONFIG" update daily-curation --template "$(cat /tmp/daily-curation-apply.json)"
$CELERISCTL cron --config "$CELERIS_CONFIG" show daily-curation   # template.mode が "apply" になっていることを確認
```

`apply` に切り替えた後の最初の run から、`curation-plan.json` が本番 KB（`~/.local/share/celeris/knowledge`）
へ決定的に反映される（書き込みは daemon のコードが行い、LLM は計画を書くだけ。削除は archive せず実削除、
理由は `knowledge/_curation/YYYY-MM-DD.md` と要約に残る）。切り替え後も最初の数回は `daily-summary.md` と
`curation.diff` を必ず目視で確認する。おかしければ `dry_run` へ戻す（同じ手順で `mode` を書き戻す）。

## 7. 一時停止・削除・トラブルシュート

```bash
$CELERISCTL cron --config "$CELERIS_CONFIG" pause daily-curation     # 一時停止（次回時刻は null に。溜まっていた queued は閉じる）
$CELERISCTL cron --config "$CELERIS_CONFIG" resume daily-curation    # 再開（一時停止中に過ぎた時刻は取りこぼし扱いにしない）
```

CLI に削除サブコマンドはない。人が job と履歴を削除する必要がある場合は、確認後に API の bearer token を
使って DELETE を呼ぶ:

```bash
curl --fail-with-body -X DELETE \
  -H "Authorization: Bearer $CELERIS_API_TOKEN" \
  "http://127.0.0.1:<api-port>/api/v1/cron-jobs/daily-curation"
$CELERISCTL cron --config "$CELERIS_CONFIG" list
```

一覧から消えたことを確認する。既に作成された task は残る。

- job が発火しない: `enabled` と `next_fire_at`（`cron show`）、daemon が動いているか（`GET /health`）を見る。
- 発火しても task が `422`/`400` で作れない: `cron history` の `detail`（outcome=`error` の行）を読む。
  たいていは harness 未登録（§1）か `project` の不整合。
- 整理 task が毎回 `skipped_overlap` になる: 前回の整理 task が終端になっていない
  （`celerisctl task show <前回 task_id>`）。長く詰まっているなら task 側の問題として個別に調べる
  （cron job 側の bug ではない）。

## 8. 分担の注意（ADR-0131 D7）

受信箱の決定論的な片付け（置き換え済み `failed` 子を attention から外す等）は `task_ops::inbox` に実装済みで、
人の操作は不要（daemon 側で常時効く）。受信箱・通知の分離 task（`01M3YFCJKMNWQ13HRS52M5BSWW`）はこの規則を
そのまま使う前提なので、同じ規則をそちらで重複実装しないこと。
