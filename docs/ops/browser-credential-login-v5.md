---
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
---
# credential login v5（username 欄・ログイン後の読み取り）の本番手順（人が行う）

決定は [ADR 2026-10-09 credential username / post-login](../../agent-docs/adr/2026-10-09-browser-credential-username-and-post-login-read.md)。
前提の launcher 解放手順は [browser launcher 経路の credential 解放手順](browser-launcher-credential-release.md)。この手順は運用
セッションで人が実行する。worker は root 操作・本番設定の変更・資格情報の入力をしない。秘密値（password・username）を shell
history・ログ・台帳・成果物・chat に書かない。

## 0. 何が変わるか

- site policy に `username_selector`（任意）と `post_login {read_origins, actions}`（任意）が増えた（DB migration 0064、schema 64）。
- credentiald は username 欄を伴う注入要求（IPC v2）と固定 `INJECT_PAIR_FUNCTION` を持つ。1 承認 = 1 lease = 1 回の注入で 2 欄を入れる。
- launcher protocol は 5。daemon は、承認で固定したログインが username 欄か post_login を使うとき、承認を消費する**前**に
  launcher の版を確かめ、5 未満なら次の文言で拒否する（承認は残る）:
  `browser launcher protocol 4 lacks the credential login verbs; rebuild and replace celeris-browser-launcher (protocol 5 required)`。
- post_login を持つ site では、ログイン後に条件（ログイン頁を離れた・`read_origins` の頁・password 欄なし）が 15 秒以内に揃えば
  agent は `read_origins` の頁だけを読める。揃わなければ今までどおり session の終わりまで観測停止（progress `browser.post_login:
  post_login_unconfirmed`）。
- 版のずれ: 旧 daemon + 新 launcher、新 daemon + 旧 launcher（username / post_login を使わない policy）は従来どおり動く。
  `celerisctl browser doctor` の `launcher` は版が 5 と一致するまで NG を出す。

## 1. daemon の release と昇格

本 branch が main に入った後、通常の selfdeploy の経路で release・verify・promote する（`release.sh <ref>` → `verify.sh <sha12>` →
`<release>/scripts/promote.sh <sha12>`、素のコマンドで）。migration 0064 は列を足すだけ（additive）。昇格後に旧 release へ戻すと
旧 binary は schema 64 の DB を開けない（`SchemaTooNew`）ので、戻すときは promote.sh が取った DB backup から戻す。

```sh
cut -d" " -f1 /proc/loadavg
pgrep -af '^bash /local/celeris/state/current/scripts/(prepare|release|verify)\.sh'
readlink /local/celeris/state/current
sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro" "select release,role,drained_at from daemon_instances order by started_at desc limit 3"
```

昇格後、credentiald を新しい release の unit に切り替える（unit は release ごと。vault は `CELERIS_CREDENTIALD_DATA_DIR` にあり残る。
登録中の稼働 session は memory だけなので、browser run が動いていないときに行う）。

```sh
systemctl --user list-units 'celeris*' --no-pager
systemctl --user stop celeris-credentiald@<旧 sha12>.service
systemctl --user start celeris-credentiald@<新 sha12>.service
systemctl --user status celeris-credentiald@<新 sha12>.service --no-pager
```

## 2. launcher の再 build と差し替え（root）

launcher は昇格した release と同じ HEAD から作る。target は NFS ではなく `/local` の scratch に置く（release の後始末は
`.cargo-target` を消すので、そこを使わない）。稼働 browser session が無いことを確かめ、既存 binary を退避し、hash を記録する。

```sh
W=<配送された agent-platform worktree（昇格した sha の checkout）>
SHA12=<昇格した sha12>
L=/usr/local/libexec/celeris/celeris-browser-launcher
cd "$W"
git rev-parse HEAD
CARGO_TARGET_DIR=/local/celeris/data/scratch/launcher-$SHA12-target cargo build --release -p task-worker --bin celeris-browser-launcher
sha256sum /local/celeris/data/scratch/launcher-$SHA12-target/release/celeris-browser-launcher "$L"
install -o root -g root -m 0755 "$L" "$L.pre-credential-login-v5"
pgrep -u celeris-browser -a
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 /local/celeris/data/scratch/launcher-$SHA12-target/release/celeris-browser-launcher "$L"
sha256sum "$L"
systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
```

daemon の実行 user で必須実証を取る（出力に秘密は無い）。どちらも最終行 `EXIT: 0` であること。

```sh
cd "$W"
sh crates/task-worker/scripts/launcher-admission-evidence.sh --stutter 3 /tmp/launcher-admission-stutter.log
sh crates/task-worker/scripts/launcher-admission-evidence.sh --credential /tmp/launcher-credential-tests.log
celerisctl browser doctor
```

doctor の `launcher` が OK（protocol 5）になったことを確かめる。build に使った target は確認後に消してよい
（`rm -rf /local/celeris/data/scratch/launcher-$SHA12-target`）。

## 3. 台帳の再生成

browser-ledger.sh は既定で `$SD_RELEASES/.cargo-target` を使うが、release の後始末がそれを消すので、`/local` の scratch を明示する。

```sh
CARGO_TARGET_DIR=/local/celeris/data/scratch/ledger-$SHA12-target bash scripts/selfdeploy/browser-ledger.sh $SHA12 --force
celerisctl browser ledger check --file ~/.local/celeris/releases/$SHA12/browser/conformance.json
celerisctl browser doctor
rm -rf /local/celeris/data/scratch/ledger-$SHA12-target
```

`ledger-status.json` が `ok: true`、doctor の ledger・backend が OK であること。欠落・失敗なら以降に進まない。

## 4. manaba の site policy を埋める

値はコードに無い（A9）。host 実証の段で運用セッションが実頁（IdP の login form、manaba の課題一覧の origin）を見て提案し、人が
web `/browser/settings` の「ログイン先の設定」で既存の `manaba-tsukuba` を編集する（API なら `PUT /api/v1/browser/site-policies/manaba-tsukuba`）。

| 欄 | 入れるもの | 確かめ方 |
|---|---|---|
| exact_origin | 今のまま（`https://idp.account.tsukuba.ac.jp`） | IdP の login form がある origin |
| login_url | 今のまま（IdP の Unsolicited SSO の URL。IdP の root には form が無い） | 開くと redirect の後に username と password の form が同じ頁に出る |
| password_selector / submit_selector | 今のまま（`input[name="j_password"]` / `button[name="_eventId_proceed"]`） | form の 1 要素ずつに一致 |
| username_selector | 実頁の username 欄（例: `input[name="j_username"]`。実頁で確認） | password 欄と同じ頁・同じ form のちょうど 1 個の `type=text` か `email` |
| post_login.read_origins | manaba の origin（例: `https://manaba.tsukuba.ac.jp`。実頁で確認） | SP（manaba）が login 後に着く頁の origin。IdP の origin は入れられない |
| post_login.actions | 課題監視に要るものだけ（推奨: snapshot・extract。添付の取得が要れば download、頁の移動に要れば click、screenshot は必要なときだけ） | 実効は task policy（task ∩ grant）との積。task policy に無い操作は増えない |

opt-in の保存時に「ログイン後の頁の内容（個人情報を含みうる）が LLM に渡る」確認を毎回求める。read_origins は task の
`network_domains`（と grant の `allowed_domains`）にも入っていなければ効かない（実効集合が空なら opt-in 無しと同じ）。

**site policy を変えたら credential を登録し直す。** broker の vault は登録時の site policy（login URL・selector・post_login）を
写して持ち、承認で固定するログインはその写しである。既存の登録（`cred-01M4H1PP…`・`cred-01M4H35F…`）は username 欄を持たないので、
policy を更新した後に task の登録依頼（`waiting_for_auth` の wait）へ人が改めて username と password を登録する。登録は owner session の
form だけで行い、値は chat・log に貼らない。

## 5. manaba の課題監視 task の実行（host 実証）

1. task の指示に「課題を提出・削除・変更しない（読むだけ）」を明記する（A6: click は read_origins 内で許すが、提出の禁止は
   task の指示で扱い、ここでは強制しない）。
2. task を再開すると、登録済み credential の使用承認（`credential_use`）が承認待ちになる。承認画面に login URL・入力する 2 欄・
   ログイン後に読む origin と操作が出るので、内容を確かめて一回だけ承認する（A7: run ごとに承認）。
3. 結果の確認（秘密を開かない）:
   - progress に `browser.credential_use: success` と `browser.post_login: resumed` が出る。`post_login_unconfirmed` なら
     IdP に留まった（属性送信の同意画面・エラー）か、着いた頁が read_origins でない・password 欄があることを示す。
   - task の成果物（課題一覧の抜き出し）に password が無い。username（学籍番号）は頁に表示された分だけ出てよい（A1）。
   - `celerisctl browser doctor` が OK のまま。

## 6. 戻し方

拒否・不一致・秘密露出の疑いがあれば task を止め、site policy の post_login を外す（または grant から `credential_use` を外す）。
launcher は退避版へ戻す。

```sh
pgrep -u celeris-browser -a
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 "$L.pre-credential-login-v5" "$L"
sha256sum "$L"
systemctl start celeris-browser-launcher.socket
```

v4 の launcher に戻すと、username 欄・post_login を使う承認は daemon が「protocol 5 required」で拒否する（承認は消費しない）。

## 7. 実 host でしか確かめられないこと

- 実 launcher（UID 995、別 userns）での v5 `authenticate` と、daemon が渡した injection 接続での credentiald の Attested admission →
  pair 注入。同一 process の偽 launcher では admission の responder UID が daemon UID になり成立しない。
- 筑波大学統一認証の実 form（username 欄の selector、同意画面の有無、redirect の経路）と manaba の着地頁・origin。
- launcher の bwrap 内 Chromium から IdP・manaba への egress（task policy の許可 domain）。
- agent-browser 0.38.1 が post-login の CDP 検査の下で snapshot / extract / click を通せること（試験は CDP を直接使う。
  agent-browser が使う CDP method の一部が検査の対象外の一覧に無く拒否される場合は `post_login_control_method` を見直す）。
- launcher 経路の screenshot / download の artifact は daemon に渡らない（既存の制限: `LauncherExecutor` は両 verb を
  opaque な失敗として返す）。policy で許しても launcher runtime では成果物にならない。
