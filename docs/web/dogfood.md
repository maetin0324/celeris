# web phase 6 P6-03: dogfood の準備

状態: 未開始

この文書は人が開始を決めた後の手順である。H6（実施期間と合格条件）が人に決まるまで dogfood を開始しない。web/ を起動して本番 daemon に接続する作業も、以下の開始条件を満たしてから行う。gui/（`:7700`）は止めず、並行して使える状態を保つ。

## 1. 開始条件

1. 人が H6 の期間、合格条件、判定日を決め、`docs/PROGRESS.md` の dogfood 節に記録する。
2. 人が H9（並行運用中のブラウザ通知の扱い）を決める。現行の暫定方針は web/ の origin で利用者が許可したときだけ通知すること（[実装計画](implementation-plan.md) §3）。dogfood で許可するかも記録する。
3. P6-02 の `celeris-web@.service` と release の web 段を確認し、H10 の staging 実 celeris 確認を人が完了する。[並行運用手順](parallel-operation.md) §2 と [ADR-0096](../adr/0096-web-parallel-operation.md) D4 に従い、release の `gate.json` の `web.ok: true`、`web/app/`、staging の `/healthz`・ログイン・gui/ の継続稼働を確認して結果を記録する。H10 はこの文書の作成時点では未確認である。

本番への配信切替や本番用の公開 port は、この開始判断に含めない。H2 により本番 port は未決定であり、以下は loopback `127.0.0.1:7720` だけを使う。

## 2. 手順（開始条件がそろった後に人が実施）

### 本番 daemon に向ける

[並行運用手順](parallel-operation.md) §2 の release と unit の設置・web 専用 password と session secret の準備を済ませる。`<sha12>` は `gate.json` で `web.ok: true` と確認した release の sha12 に置き換える。`deploy/systemd/celeris-web@.service` と `web/server/index.js` の実際の設定名を使い、web gateway を本番 daemon（同じホストの `127.0.0.1:7710`）へ向ける。`CELERIS_API_TOKEN_FILE` は unit の既定 `~/.config/celeris/api.token` を使う。

```sh
cat >~/.config/celeris/web.env <<'ENV'
CELERIS_WEB_BIND=127.0.0.1:7720
CELERIS_API_URL=http://127.0.0.1:7710
ENV
systemctl --user start celeris-web@<sha12>.service
systemctl --user status celeris-web@<sha12>.service --no-pager
curl -fsS http://127.0.0.1:7720/healthz
journalctl --user -u celeris-web@<sha12>.service -n 50
```

`/healthz` の `release` が `<sha12>` と一致することを確認する。selfdeploy の `scripts/selfdeploy/release.sh` は gui/ の gate の後に web 段を非 blocking で実行し、成功した release に `web/app/` を作る。`gate_ok` だけでは web 段の成功を示さないため、必ず `gate.json` の `web.ok` を見る。`verify.sh` と `promote.sh` は web gateway を起動しない（[ADR-0096](../adr/0096-web-parallel-operation.md) D3）。

### PC とスマホで確認する

- **PC:** gateway と同じホストで `http://127.0.0.1:7720/login` を開く。別の PC からは、その PC の SSH クライアントで `ssh -N -L 7720:127.0.0.1:7720 <user>@<gateway-host>` を張り、PC のブラウザで同じ URL を開く。web 専用 password でログインし、日常の画面と操作、通知の H9 方針を確認する。
- **スマホ:** スマホ上でローカル port forward が使える SSH クライアントから `ssh -N -L 7720:127.0.0.1:7720 <user>@<gateway-host>` と同等の転送を張る。スマホ自身のブラウザで `http://127.0.0.1:7720/login` を開く。PC 側の転送を張るだけではスマホの `127.0.0.1` には届かない。ログイン、画面の表示と操作、通知の H9 方針を確認する。
- 両端末で作業中も gui/ の `http://127.0.0.1:7700/healthz` と既存の入口を確認し、gui/ を止めない。問題が出た場合は [並行運用手順](parallel-operation.md) §3 の web unit 停止手順を参照する。

## 3. 問題の記録場所

`docs/PROGRESS.md` に次の節を作り、期間中の問題と対応を 1 件ずつ追記する。各問題は必要に応じて小さい修正タスクに分け、タスク ID と再確認結果を同じ行に追記する。

```md
## Web GUI dogfood（開始 <日付>、H6: <期間・合格条件>）

- H9: <通知の扱い>。H10: <staging 確認日・結果>。
- <日付>｜<画面>｜<端末: PC/スマホ・ブラウザ>｜<現象>｜<重大度>｜<対応・タスク ID・再確認結果>
```

## 4. 終了と判定

人が決めた H6 の期間が終わったら、`docs/PROGRESS.md` の記録を基に H6 の合格条件を人が判定し、結果と未解決事項を追記する。web/ への配信切替（H7、P6-04）は別の人の判断と承認を要する。dogfood の合格だけで入口を切り替えない。
