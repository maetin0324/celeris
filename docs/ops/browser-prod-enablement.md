# 本番 browser 実行の有効化と点検

---
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64, 01M4D6TWCKEH25YVZK1YHMBFJQ]
---

この手順は運用セッション向け。利用者の日常操作は **web で owner session を立てる、初回に credential form に ID/password を入力する、run 中の承認ボタンを押す**の三つにする。release の生成・昇格、host の設定・unit の配置は運用側が準備する。この文書の本番変更コマンドは、人が本番操作を実行する運用セッションの手順であり、coding run からは実行しない。

## 1. 適合台帳を用意する

新しい release は `release.sh` の `browser-ledger` 段で実 agent-browser 0.38.1 と protocol 台本を使い、loopback fixture で適合台帳を生成する。実 LLM は使わない。credential 注入には実 Chromium・userns での P4-B 証拠も必要。台帳は `<release>/browser/conformance.json` に置かれ、daemon は起動 release と同じ台帳を読む。新 release の昇格時に環境変数を手で追加する必要はない。

既存 release の台帳だけを作り直す場合、運用セッションで次を実行する（`<sha12>` は対象 release の実 SHA に置換する）。

```sh
bash scripts/selfdeploy/browser-ledger.sh <sha12>
# 有効な台帳があるが P4-B 証拠を再測定したいとき
bash scripts/selfdeploy/browser-ledger.sh <sha12> --force
celerisctl browser doctor --config /absolute/path/to/config.toml
```

台帳が有効なら通常は何もしない。`--force` は既存の測定を作り直すために使う。失敗時は既存台帳を残す。成功後は daemon の mtime 監視が拾い、台帳不足で止まっていた task は自動で再開する。ログは selfdeploy のログディレクトリ、release 時は `gate-logs/browser-ledger.log`、結果は `browser/ledger-status.json` と manifest/gate の `browser_ledger` 欄で確認する。

新 release を用意する場合は [selfdeploy 手順](selfdeploy.md)に従い、運用セッションで prepare・verify・promote を行う。

```sh
bash scripts/selfdeploy/prepare.sh <sha40> /absolute/path/to/prepare-result
bash scripts/selfdeploy/promote.sh <sha12>
```

browser 台帳生成は release 全体を失敗にしない。prepare が成功しても browser が使えるとは限らないため、**昇格後に doctor を確認**する。台帳は手書きしない。`--scripted` の測定は本番台帳に使えない。`CELERIS_BROWSER_CONFORMANCE_FILE` は開発・試験用の明示上書きであり、本番で別 release の台帳を指さない。

## 2. 点検コマンド

```sh
celerisctl browser doctor --config /absolute/path/to/config.toml
celerisctl browser doctor --config /absolute/path/to/config.toml --json
# API の場所を明示する場合（token 自体は argv に載せない）
celerisctl --api-url http://127.0.0.1:7700/api/v1 browser doctor \
  --token-file /absolute/path/to/api-token
```

`CELERIS_CONFIG`、`CELERIS_API_URL`、`CELERIS_API_TOKEN_FILE` でも差し替えられる。`--config` は CLI の接続先・token file の設定。runtime・socket・PATH・release の判定は **稼働 daemon 自身の設定と環境**で行う。CLI は本番 DB を開かず、`GET /api/v1/browser/readiness` を読み取る。socket path は daemon の config で指定する。設定を変更した場合は運用セッションで設定を反映した daemon に切り替える。site policy と grant は web で変更し、再起動不要。

出力は `<状態> <項目> <説明と修正方法>` の一項目一行。`OK` は確認済み、`NG` は不足・不達、`WARN` は公開 task は使えるが認証などの追加設定が必要、`SKIP` はその構成では検査しない。終了値は NG なしで 0、NG ありで 1、API 不達・認証失敗・未対応 daemon で 2。JSON は `{items: [{status, check, detail}]}`。

```text
NG   ledger browser の適合台帳が未配置 code=missing; 修正: release か台帳再生成
OK   runtime launcher
OK   launcher 固定 Hello IPC
NG   credentiald control Ping; 修正: socket と daemon PID 許可を確認
WARN grant browser-execution: credential_use=false; 修正: web で設定
```

doctor は能力や認可を変更しない。DNS は固定名 `example.com` に一回だけ問い合わせ（上限 2 秒）、launcher は session を作らない Hello、credentiald は登録・秘密・lease を変更しない Ping。Ping は他の control 操作と同じ UID/PID admission を要求する。launcher 構成では daemon 側の bwrap/sandboxd/egress は SKIP となるため、launcher host の準備も別途確認する。doctor の成功は manaba の認証成功や実 Chromium 起動までを保証しない。

## 3. 不足項目の直し方（運用セッション）

| 項目 | 確認・修正と再点検 |
|---|---|
| `daemon` | config の `[api] listen`、token file、URL の `/api/v1`、稼働 release を確認する。readiness 未対応の旧 daemon は新 release に切り替える。token をログへ出さない。 |
| `ledger` | `missing/invalid/stale_release/stale_agent_browser/no_conformant_backend` を確認し、対象 release の台帳を上の手順で生成する。`generated_for` の release と agent-browser の版が一致すること。 |
| `ledger-backends` | `credential` が空ならログの P4-B 段を確認する。userns・実 Chromium と launcher の host 前提を整え、`--force` で再生成する。証拠の無い credential 注入は許可しない。 |
| `agent-browser` | daemon の PATH で `agent-browser --version` が `0.38.1` を返すよう配置する。版を変えたら台帳も再生成する。 |
| `runtime` / `launcher` | `[browser] runtime = "launcher"` と `launcher_socket` を設定し、[launcher host 手順](browser-launcher-host-setup.md)の専用 UID・固定 binary・socket/service・daemon UID の許可を確認する。同じ release の Hello protocol に対応する launcher を配置する。本番で試験用 loopback 許可は使わない。 |
| `bwrap` / `sandboxd` / `egress` | `runtime=daemon` の場合、`/usr/bin/bwrap` と release の `bin/celeris-browser-sandboxd`、`bin/celeris-browser-egress` を配置する。launcher 構成ではこれらは launcher 側の準備。 |
| `egress-resolver` | `[browser.egress] resolver = "<DNS server IP>"` と UDP/53 の到達性を確認する。launcher 側の固定 resolver 設定も確認する。doctor は DNS 応答を検査し、接続先許可を広げない。 |
| `credentiald` | `[api] browser_credentiald_control_socket` を実 socket に合わせる。credentiald は daemon と同じ UID で、control の許可 PID/starttime が**現在の daemon**を指す構成にする。daemon 切替後の古い PID 許可は更新する。同じ release の Ping 対応 credentiald を使用する。[credentiald 手順](../guides/browser-credentiald.md)を参照。 |
| `attestation-key` | `[api] browser_attestation_public_key_file` に web の署名鍵と対になる Ed25519 公開鍵を指定する（32 byte raw または hex）。web の `CELERIS_WEB_ATTESTATION_KEY_FILE` の秘密鍵を daemon に渡さない。 |
| `site-policies` / `site-policy` | web `/browser/settings` で owner session を立て、対象の exact origin・login URL・password/submit selector を登録・修正する。DB が正本。config の `[[api.browser_site_policies]]` は未登録 ID の種であり、取り込み済みなら削除してよい。DB と食い違う種は WARN。 |
| `grant` | web `/browser/settings` で browser-execution の許可 origin、`credential_use`、実在する credential policy ID を設定する。存在しない ID は NG。credential_use が off または ID が空なら WARN（公開 task だけでよい場合はそのまま）。 |

SSO（IdP 経由）の login では、password を入力する画面の origin を exact origin、IdP の静的入口を login URL にする。
manaba の例は exact origin が `https://idp.account.tsukuba.ac.jp`、login URL が
`https://idp.account.tsukuba.ac.jp/idp/profile/SAML2/Unsolicited/SSO?providerId=https%3A%2F%2Fmanaba.tsukuba.ac.jp%2Fshibboleth`。
同一 origin の redirect（最大 32 hop）と JS 自動 POST の中継画面は許可する。15 秒以内に password 欄が現れなければ中止する。
注入前に別 origin への redirect・文書遷移があれば拒否する。submit 後の SP への戻りは通常遷移として扱う。
password/submit selector は人が IdP の入力画面で確認して登録する（selector 文法は従来どおり）。多要素認証の完了は password 注入だけでは保証しない。

## 4. 有効化から manaba 確認まで

運用セッションが host 前提・台帳・web の site policy と grant を準備し、doctor の NG を解消する。認証を使う場合は `ledger-backends` の credential が空でなく、grant の credential_use が on、対象 policy ID があることも確認する。利用者に TOML・curl・systemd 作業を依頼しない。

1. 利用者は web `/browser/settings` の手順で信頼端末から owner session を立てる。
2. 運用セッションは一時停止中の manaba 監視 task `01M4CD11W51ADC4HHMMEKRQM71` を web で確認する。task 詳細の browser policy と requirements の対象 origin、site policy ID を確認・必要なら編集し、監視を再開する（新規の確認 task を web で起票してもよい）。台帳不足だけで blocked の task は台帳が揃うと自動再開するが、人が一時停止した task は別途再開する。
3. 初回の `waiting_for_auth` では、利用者が web の credential form に manaba の ID/password を入力する。秘密を task 本文・chat・artifact・CLI に書かない。
4. `waiting_for_approval` では、利用者が操作の対象・origin を読み、web の承認ボタンを押す。click/download/credential_use は都度承認であり、包括承認や standing approval は設けない。
5. 運用セッションは task 詳細の browser 節、run の結果と受信箱を確認する。停止時は具体的な理由と doctor を照合する。台帳不足なら task の infra 再試行を繰り返すのではなく台帳を直す。認証・selector・task policy が原因なら web の該当欄を直す。

host の初期有効化・本番昇格・実 manaba 認証は運用セッションの作業であり、この実装の一時 fixture 試験とは分けて記録する。
