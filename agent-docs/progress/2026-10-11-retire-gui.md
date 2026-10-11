---
title: "旧 GUI を廃止し web に一本化する"
tasks: [01M4NAKDCESYK36PNY7TTCTJ5E]
status: done
updated: 2026-10-11
---

# 旧 GUI の廃止

## 現在地

2026-10-11: 人が必要とした2機能を web に実装し、旧 GUI のソース・配布・検査経路を撤去した。
撤去後の最終検査は合格。本番の unit・config・DB には触れていない。
方針は [廃止 ADR](../adr/2026-10-11-retire-gui.md) と
[破壊的変更を既定にする ADR](../adr/2026-10-11-breaking-changes-by-default.md)。

## 実装済み（Phase 1）

- `/cron` と `/cron/:id`: 一覧・次回・前回結果・雛形・履歴・task リンク、停止/再開/手動実行。
  `web/features/cron/` が既存 API relay `/api/cron-jobs` を使う。送信後に一覧・詳細・履歴を再取得。
  読み込み・空・取得失敗・古いデータ・権限不足を区別し、操作の二重送信を防ぐ。
- タスク詳細の「実行」に WorkUnit 検査ログを追加。展開時のみ取得し、実行中は5秒更新。
  現在/最後の検査の番号・command・exit・末尾・省略情報を表示。失敗時は直前のログを保持。
  木の実装は変更していない（並行 task 01M4NAKDCC との境界）。
- API・生成型は既存のものだけを使用。旧 server 処理の移植、Rust/API/schema の変更はない。
- cron を画面台帳・gateway の SPA route・ナビへ登録。既存 `spa-routes.test.mjs` の古い固定画面数は削除し、
  route tree との集合一致検査を維持した。

## 依存調査

web gateway は `web/server/index.js` の daemon URL（既定7710）から API・SSE・Console・file を配線する。
`auth.js` は独自 cookie/鍵、`browser-live.js` は本人認証・承認・credential・identity・Live View と
frames/WS を提供する。旧 GUI プロセスを経由する必須の経路は確認されていない。
本番の環境変数で upstream が上書きされている可能性は、移行時に運用セッションが確認する。

旧 `gui/app/routes.ts` の画面と resource を web の各 feature と対応表に照合。
旧画面限定として確認した cron と検査ログは今回追加済み。それ以外の限定機能は確認されていない。
詳細な対応表・調査コマンドは task artifacts の `gui-removal-audit.md` と `gui-removal-audit-checks.json`。

## 検証（Phase 1）

ログの根: `/local/celeris/data/workspaces/01M4NAKDCESYK36PNY7TTCTJ5E/artifacts`。

| コマンド | 結果・ログ |
|---|---|
| `corepack pnpm@12.6.0 -C web build` | 合格。既存の chunk size 警告あり。`web-build.log` |
| `corepack pnpm@12.6.0 -C web typecheck` | 合格。`web-typecheck.log` |
| `corepack pnpm@12.6.0 -C web lint` | 合格。既存 CSS の警告5件。`web-lint.log` |
| `corepack pnpm@12.6.0 -C web test` | vitest 93 files / 678件、gateway 93件合格。`web-test.log` |
| `corepack pnpm@12.6.0 -C web check:boundaries` | 合格。`web-boundaries.log` |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/cron-check-logs.spec.ts e2e/parity/task-detail.spec.ts` | 追加3件＋既存7件、計10件合格。`web-cron-check-e2e.log` |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/cron-check-logs.spec.ts` | ログの4幅・axe・更新失敗時の保持を追加後も3件合格。`web-cron-check-final-e2e.log` |
| `corepack pnpm@12.6.0 -C web e2e a11y/axe.spec.ts --grep 'S4 /cron'` | 一覧/詳細の2件合格。`web-cron-axe.log` |
| `corepack pnpm@12.6.0 -C web mobile-audit --only /cron` と `--only /cron/daily` | 各360/390/412/1440pxで合格。`mobile-cron.log` / `mobile-cron-detail.log` |

画像は `cron-screenshots/`（8枚）と `check-log-screenshots/`（4枚）。新設画面の変更前は404
（初回調査 JSON の isSpaRoute=false）。検査ログは上記 e2e で4幅と axe を確認。
依存の offline install は tarball が不足したため、同一 lockfile の既存 checkout から依存だけを
worktree の `web/node_modules` へコピーして検証した。コピー元は変更していない。

Rust の指定検査も完了:

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0、5,131 passed / 0 failed / 14 ignored
  （nextest 5,128件＋doctest 3件）。`test-parallel.log`。残った一時 fixture 9件は script の EXIT cleanup で削除。
- `cargo clippy --workspace -- -D warnings`: exit 0。`clippy.log`。
- `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、
  `sh scripts/dev/progress-index.sh --check`: 全て exit 0（`doc-links.log` / `adr-numbers.log` / `progress-check.log`）。

通常の run TMPDIR は Unix socket の長さ上限を超えた。短い symlink での再試行は canonical path
比較/安全検査に影響した（`test-parallel-short-path.log`）。`/tmp/ct.XXXXXX` でも script がさらに作る
階層により4件が readiness timeout（`test-parallel-final.log`）となり、最終的に試験 fixture に限って
`TMPDIR=/tmp` を指定して全件合格した。source/依存のコピーや build 出力を /tmp に置いていない。
CARGO_TARGET_DIR・compiler wrapper・build 設定は渡されたまま。失敗した版と正常結果を取り違えないこと。

## 撤去（Phase 2）

`gui/`、専用 unit、check-mobile/delivery/upgrade/model-routing、sync-gui-docs、旧名称への一度きりの
移行台本を削除した。web の配布・verify・promote は必須で、失敗を成功扱いしない。
現行 API 文書は `docs/api/v1/api.md` に移動した。既存 ADR は本文を改変せず保持する。

通知の config/API 欄を `web_base_url`、Browser CLI の socket 環境変数を `CELERIS_WEB_OWNER_SOCKET` に変更。
生成 schema と web 型も更新した。DB migration は追加せず、events・credentials・cron API は維持する。

## 検証（Phase 2・撤去後）

全て exit 0。ログの根は上の task artifacts。失敗した中間版ではなく次の最終ログを参照する。

| コマンド | 結果・ログ |
|---|---|
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | **5,132 passed / 0 failed / 14 ignored**（nextest 5,129＋doctest 3）。`removal-test-parallel-final.log` |
| `cargo clippy --workspace -- -D warnings` | 合格。`removal-clippy.log` |
| `cargo fmt --all -- --check` | 合格。`removal-fmt.log` |
| `corepack pnpm@12.6.0 -C web typecheck` | 合格。`removal-web-typecheck.log` |
| `corepack pnpm@12.6.0 -C web test` | vitest **677件**＋gateway **93件**合格。`removal-web-test.log` |
| `corepack pnpm@12.6.0 -C web build` | 合格。既存の chunk size 警告のみ。`removal-web-build.log` |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/cron-check-logs.spec.ts e2e/parity/task-detail.spec.ts` | **10件**合格。`removal-e2e.log` |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/gateway-auth.spec.ts` | **4件**合格。`removal-auth-e2e.log` |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/gateway.spec.ts` | **6件**合格（型再生成一致・境界・認証・CSRF・Host・token を含む）。`removal-gateway-e2e.log` |
| `for f in scripts/selfdeploy/tests/*.sh; do bash "$f"; done` | **25本全て成功**。`selfdeploy/summary.tsv` と各同名 `.log`。本番 systemctl は使用せず隔離 fixture を使用 |
| `sh scripts/dev/check-doc-links.sh` と `--self-test` | 合格。`removal-doc-links.log` / `removal-doc-links-self-test.log` |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 合格。`removal-doc-layout.log` |
| `sh scripts/dev/check-adr-numbers.sh`、`python3 scripts/dev/check-architecture-map.py`、`sh scripts/dev/progress-index.sh --check` | 合格。`removal-adr-numbers.log` / `removal-architecture-map.log` / `removal-progress-index.log` |
| `python3 -m unittest discover -s scripts/tests -p test_source_size_report.py` | **28件**合格。`removal-source-size.log` |

selfdeploy の確認範囲は、web 必須 gate、依存の無い梱包の拒否、verify の app 起動失敗、初回 web 起動、
web 切替失敗時の復旧、live/stop-start 両経路の再試行、hot root の unit と web/app 配布。
staging の read-only E2E は旧 GUI の health を見ず、web gateway と daemon を見る。
本番向け release の作成・実 staging verify・promote は下の運用手順で行う。

### 残存参照の走査

`git ls-files gui/` は空、作業場所の同ディレクトリも存在しない。
追跡ファイルを `rg -n '(?i)\bgui\b|remix'` と、旧パス・unit・config・環境変数を限定する式で全走査した。
再現 script は task artifacts の `scan-removal.py`。全件は `removal-rg-all.txt` / `removal-rg-legacy.txt`、
集計は `removal-rg-summary.json`。現行の実行・build・deploy・検査に必要な旧実装への参照は **0件**。

「GUI」という一般的な画面の呼び名や、過去の ADR・進捗・検証報告までゼロにした、という意味ではない。
過去192本の ADR 本文は変更前とバイト一致を確認し、廃止されたアプリ内 ADR は履歴側へ移した。
残す旧名称は歴史記録、本廃止の移行・復旧手順、ADR の所在/リンク検査、廃止 config 欄の拒否試験。
`removal-rg-unclassified.txt` は空。旧画面の設計説明・現行運用ガイド・生成型の二重管理は撤去した。

## delivery-repair（2026-10-11）

差し戻しの `prepare.log` と `.build/30c820ef45d7/.gate-cargo-test.log` を確認した。
18件の失敗は、配送 daemon から継承した `CELERIS_CONFIG` が本番 config を指し、
試験の `celerisctl add` が廃止済みの通知設定を読んだためだった。`--db` は設定の指定を解除しない。
worker DB read-only 試験でも、スタブ内の CLI が同じ設定エラーになり、本来の書込み拒否の検証まで届いていなかった。

`tests/e2e/tests/` の CLI 呼出し11箇所と DB read-only 試験の daemon 起動で、子プロセスに渡す
`CELERIS_CONFIG` を除いた。試験が明示する `--config`、本体の設定読込み、旧設定の拒否、
user namespace の gate は変更していない。nextest と直接の cargo test の両方に効く修正である。

回帰確認では、秘密を含まない旧設定の fixture を task artifacts の `repair-invalid-config.toml` に作り、
それを `CELERIS_CONFIG` に指定して試験する。本番 config を編集・複製せずに再現する。
検証ログは同じ task artifacts の `repair-*.log` と `repair-selfdeploy/` に保存する。

検証時の `A=/local/celeris/data/workspaces/01M4NAKDCESYK36PNY7TTCTJ5E/artifacts`。
`repair-invalid-config.toml` の内容は `[notify]` と `gui_base_url = "http://invalid.example"` の2行。

| コマンド | 結果・ログ |
|---|---|
| `CELERIS_CONFIG="$A/repair-invalid-config.toml" TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | **5,132 passed / 0 failed / 14 ignored**。`repair-test-parallel.log` |
| `CELERIS_CONFIG="$A/repair-invalid-config.toml" CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require TMPDIR=/tmp bash scripts/dev/test-parallel.sh -E 'package(e2e)'` | **E2E 48＋doctest 3 passed / 0 failed**、userns=true。`repair-e2e-userns.log`。sandbox 外で一時 DB とスタブを使用し、差し戻し18件を含む E2E 全体を実行 |
| `cargo clippy --workspace -- -D warnings` / `cargo fmt --all -- --check` | 両方 exit 0。`repair-clippy.log` / `repair-fmt.log` |
| `corepack pnpm@12.6.0 -C web typecheck` / `corepack pnpm@12.6.0 -C web test` | 合格、**677＋93件**。`repair-web-typecheck.log` / `repair-web-test.log` |
| `for f in scripts/selfdeploy/tests/*.sh; do bash "$f"; done` | **25本全て exit 0**。`repair-selfdeploy/summary.tsv` と各 `.log` |
| `python3 "$A/scan-removal.py"` | 追跡された旧アプリ0件、未分類0件。全体3,998行/506ファイル、legacy限定1,291行/210ファイル。`repair-reference-check.log` |
| `sh scripts/dev/check-doc-links.sh` / `sh scripts/dev/check-adr-numbers.sh` / `sh scripts/dev/progress-index.sh --check` | 合格。`repair-doc-links.log` / `repair-adr-numbers.log` / `repair-progress-index.log` |

`TMPDIR=/tmp` は Unix socket の長さ制限を避ける小さな試験 fixture 用。コンパイル出力と依存は
渡された `CARGO_TARGET_DIR` を維持した。release 作成・staging verify・昇格は実行していない。

## 本番への移行

**以下は運用セッションが実行する手順。本 task から本番操作はしていない。**
通常の配布の詳細は [selfdeploy](../../docs/ops/selfdeploy.md) と [web 運用](../../docs/ops/web-operation.md)。
今回の config 改名は新旧バイナリ間で互換でないため、旧 config の控えと新 config を揃えてから切り替える。

### 1. 現状の確認・復旧材料の保存

Bash で、確定した撤去コミットを `NEW_SHA` に指定する。秘密を標準出力へ出さない。

```bash
NEW_SHA='REPLACE_WITH_12_CHAR_SHA'
[[ "$NEW_SHA" =~ ^[0-9a-f]{12}$ ]] || exit 1
CONFIG_DIR="${CELERIS_CONFIG_DIR:-$HOME/.config/celeris}"
if [ -f "$CONFIG_DIR/paths.env" ]; then set -a; . "$CONFIG_DIR/paths.env"; set +a; fi
STATE_DIR="${CELERIS_STATE_DIR:-$HOME/.local/celeris}"
BACKUPS_DIR="${CELERIS_BACKUPS_DIR:-$STATE_DIR/backups}"
umask 077
mkdir -p "$BACKUPS_DIR"
MIGRATION_DIR=$(mktemp -d "$BACKUPS_DIR/gui-retirement.XXXXXX")
OLD_SHA=$(basename "$(readlink -f "$STATE_DIR/current")")
printf '%s\n' "$OLD_SHA" >"$MIGRATION_DIR/old-sha"
cp -p "$CONFIG_DIR/config.toml" "$MIGRATION_DIR/config.toml"
[ ! -f "$CONFIG_DIR/web.env" ] || cp -p "$CONFIG_DIR/web.env" "$MIGRATION_DIR/web.env"
systemctl --user list-units --all --plain --no-legend 'celeris-gui@*.service' >"$MIGRATION_DIR/gui-units.txt"
systemctl --user list-unit-files --no-legend 'celeris-gui@*.service' >"$MIGRATION_DIR/gui-unit-files.txt"
systemctl --user cat celeris-gui@.service >"$MIGRATION_DIR/gui-template.txt"
systemctl --user cat 'celeris-web@.service' >"$MIGRATION_DIR/web-template.txt"
mkdir -p "$MIGRATION_DIR/units"
for f in "$HOME/.config/systemd/user"/celeris-gui*; do
  [ ! -e "$f" ] || cp -a "$f" "$MIGRATION_DIR/units/"
done
ss -ltnp >"$MIGRATION_DIR/listeners.txt"
```

`gui-units.txt` と `gui-unit-files.txt` から停止対象の instance を確認する。単発 `celeris-gui.service` が
残っていないかも確認する。テンプレート・instance ごとの drop-in による port 上書きも調べる。
web の unit と web.env で `CELERIS_API_URL` が daemon（通常7710）、`CELERIS_WEB_LIVE_VIEW_UPSTREAM` が
Browser dashboard（設定がある場合）を指し、7700や旧 GUI を経由していないことを確認する。
環境変数の全値を共有ログへ出さず、秘密を含む調査出力は上の0700ディレクトリだけに保存する。

公開 URL を提供する nginx/Caddy/SSH tunnel 等の設定は、その管理場所で原本を退避する。
7700向けの upstream・WebSocket/SSE・ファイル転送設定を特定する。web は通常7720、既存LAN中継は7721。
実際の公開 URL・TLS・Host allowlist を確認し、切替先を決める。host の現物は本 task では未検査。

### 2. 新 config と gui 無し release の準備・verify

**初回の注意:** 旧 release の `current/scripts/prepare.sh` は同梱の旧 `release.sh` / `verify.sh` を
呼ぶ。今回の失敗ログにも `gui steps` が記録されているため、Rust 試験の修正だけでは旧台本の
GUI build 段は消えない。以下のコマンドは、撤去と本 repair を含むコミットのチェックアウトで実行する。
古い `current/scripts/` から実行しない。旧 config を本番で動かしたまま、新版の台本で release と
候補 config の verify を済ませ、その後に昇格する。一度新版へ昇格すれば同梱台本も更新される。
この初回切替は運用セッションの作業であり、本 repair では自動配送や本番台本を変更していない。

旧 config と同じディレクトリに候補を置く（相対パスの解決先を変えない）。候補の `[notify]` に
`gui_base_url` があれば `web_base_url` に改名し、値を確認済みの web 公開 URL にする。
同欄が無ければ追加は不要。他の設定・秘密は変えない。旧欄を残すと新 daemon は起動を拒否する。
CLI のログイン補助を使う shell 設定も `CELERIS_GUI_OWNER_SOCKET` から `CELERIS_WEB_OWNER_SOCKET` へ変更し、
値は `web.env` の owner socket と揃える。旧 GUI 用秘密や信頼端末・credential は削除しない。

```bash
cp -p "$CONFIG_DIR/config.toml" "$CONFIG_DIR/config.web-next.toml"
# 人が config.web-next.toml の上記1欄だけを編集する。
SD_REPO="$PWD" scripts/selfdeploy/release.sh "$NEW_SHA"
REL="$STATE_DIR/releases/$NEW_SHA"
test ! -e "$REL/gui" && test -f "$REL/web/app/server/index.js" && test -d "$REL/web/app/node_modules"
python3 - "$REL/gate.json" <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
assert v['ok'] and v['web']['ok'] and v['web']['blocking']
PY
CELERIS_CONFIG="$CONFIG_DIR/config.web-next.toml" scripts/selfdeploy/verify.sh "$NEW_SHA"
python3 - "$REL/verify.json" <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
assert v['ok']
assert any(c['name'] == 'web-app-start' and c['ok'] for c in v['checks'])
PY
```

release/verify は失敗ならここで止め、`.build/<sha>/` または staging のログを確認する。
候補 config を旧バイナリが読めず `live_ok=false` となる場合は正常な判定で、停止→起動で昇格する。
旧 config を変更していないこの段階は本番を切り替えず中断できる。

### 3. 切替・動作確認

```bash
scripts/selfdeploy/install-units.sh
install -m 0600 "$CONFIG_DIR/config.web-next.toml" "$CONFIG_DIR/config.toml"
"$REL/scripts/promote.sh" "$NEW_SHA"
test "$(basename "$(readlink -f "$STATE_DIR/current")")" = "$NEW_SHA"
systemctl --user is-active "celeris@$NEW_SHA" "celeris-web@$NEW_SHA"
curl -fsS http://127.0.0.1:7720/healthz
```

bind を上書きしている場合は最後の URL をその値に合わせる。health の release が `NEW_SHA` と一致すること。
promote は daemon の認可・backup・handoff の後に web を切り替える。web が失敗した場合は非0、current は更新せず、
旧 web を復旧する。ただし daemon は既に新しい場合がある。`promote_failed.json` とログを読み、修復後に同じ
promote を再実行する（無条件に daemon を再起動しない）。

web へログインし、タスク・WorkUnit 検査ログの更新、cron 一覧/詳細/履歴、本人認証・Browser Live View、
SSE の更新、成果物ファイル取得を確認する。確認のために本番タスクや cron の手動実行を作らない。
その後、保存したリバースプロキシ設定の upstream を web に変更し、設定検査→管理者の通常の reload 手順で反映する。
公開 URL でも login・SSE・Live View/WebSocket・file が使えることを確かめる。proxy の種類が host ごとに異なるため、
ここで別製品のコマンドを自動実行しない。

### 4. 旧 unit の停止・無効化・除去

web と公開 URL の確認後に行う。状態一覧に記録された instance を全て指定する（下は旧 current の例）。

```bash
systemctl --user disable --now "celeris-gui@$OLD_SHA.service"
# gui-units.txt / gui-unit-files.txt に他の instance があれば、その正確な名前にも同じ操作を行う。
systemctl --user list-units --state=active --no-legend 'celeris-gui@*.service'
systemctl --user list-unit-files --state=enabled --no-legend 'celeris-gui@*.service'
```

両方が空であることを確かめる。単発 unit があれば同様に停止・無効化する。保存したテンプレート・drop-in が
復元可能であることを確認してから、user unit の原本を退避する。

```bash
mkdir -p "$MIGRATION_DIR/removed-units"
for f in "$HOME/.config/systemd/user"/celeris-gui*; do
  [ ! -e "$f" ] || mv "$f" "$MIGRATION_DIR/removed-units/"
done
systemctl --user daemon-reload
ss -ltnp 'sport = :7700'
```

旧portに listener が残った場合は所有 process・unit を特定してから停止し、portだけを根拠に kill しない。
旧port向けの proxy upstream・不要な forward/listen/firewall 公開を、その設定の管理手順で除去する。
DB・events・credentials・既存 release・共有依存 cache・旧認証鍵は削除しない。旧版の rollback に必要である。

### 5. 復旧

この変更は DB schema を変更しない。画面撤去の復旧では **`--restore-db` を使わない**。
昇格後のデータを保持し、保存した config・旧 unit・proxy と旧 release を戻す。

1. proxy を戻す前に旧 unit のテンプレート/drop-in を `$MIGRATION_DIR/units/` から元の場所へコピーし、
   `systemctl --user daemon-reload`。旧 GUI の秘密と依存 cache が残っていることを確認する。
2. `$MIGRATION_DIR/config.toml` を0600で本番 config に戻す。保存した web.env も変更していた場合だけ復元する。
3. **旧 release に同梱された** `"$STATE_DIR/releases/$OLD_SHA/scripts/promote.sh" "$OLD_SHA"` を実行する。
   current が新へ更新済みなら旧同梱の rollback.sh も利用できるが、失敗した昇格では previous が指す先を推測しない。
   旧同梱 script は旧 GUI を起動できる。新コードへ互換経路は追加しない。
4. daemon と画面の health/release を確認し、proxy を保存した設定へ戻す。新 web が残る場合は新 instance を
   disable/stop し、旧 web を使っていた場合は保存した web unit と旧 release の web-follow.sh で復旧する。
   旧 GUI の表示とログイン、SSE・file・Live View を確認する。

別の同時変更で DB schema が進んでいて旧版が読めない場合はここで停止し、DBを戻してデータを失う操作は別途判断する。

## 未解決事項・提案

本番切替は運用セッションで上記の手順を実行する。追加の機能判断は無い。
