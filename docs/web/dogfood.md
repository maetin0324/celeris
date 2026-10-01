# web phase 6 P6-03: dogfood の準備

---
tasks: [01M3WAKKJQXT79DDDFCF9F5Q3D]
---

状態: 開始待ち（2026-10-01、人の dogfood-mode=a と LAN 内の別端末への公開方針は回答済み。gateway は未起動）

人は P6-03 の gateway 起動と読み取り確認を選んだ。H6（実施期間と合格条件）は後から人が決め、決まるまで cutover はしない。gui/（`:7700`）は止めず、並行して使える状態を保つ。この run では本番への設置・起動ができていないため、開始日は未記入とする。

## 1. 開始条件

1. 人が H6 の期間、合格条件、判定日を決め、`docs/PROGRESS.md` の dogfood 節に記録する。P6-03 の起動許可は回答済み。H6 が未決定の間は cutover をしない。
2. 人が H9（並行運用中のブラウザ通知の扱い）を決める。現行の暫定方針は web/ の origin で利用者が許可したときだけ通知すること（[実装計画](implementation-plan.md) §3）。dogfood で許可するかも記録する。
3. P6-02 の `celeris-web@.service` と release の web 段を確認する。H10 の staging 実 celeris 確認は release `bf54b41ad627` で完了済み（`verify.sh` exit 0、読み取り parity 3 passed）。本番で使う release の設置と `gate.json` の `web.ok: true` は本番側で再確認する。

本番への配信切替は、この開始判断に含めない。起動時の bind は loopback `127.0.0.1:7720`。人は LAN 内の別端末からも使える公開方針を回答したが、公開 port・到達経路・Host 許可設定は未決定であり、公開作業も未実施。別端末からのアクセスは確立後に検証する。

### 2026-10-01 の開始待ち理由

- staging 合格済み release `bf54b41ad627` は前 run の staging 領域にあるが、本番 `~/.local/celeris/releases/` にはない。本番にある release の `gate.json` は web 段の成功を示さず、`web/app/` もない。
- `~/.config/systemd/user/celeris-web@.service`、`~/.config/celeris/web.password`、`web.session-secret`、`web.env` は未設置。この run は worktree 外を編集できず、user systemd bus にも接続できない（`No data available`）。
- `127.0.0.1:7700/healthz` は 200、release `7fbfc347b240`。`:7710` は待ち受け中で、未認証の `/healthz` は 401。`:7720` は待ち受けていない。起動後の `/healthz`・PC 1440px・スマホ 390px の Playwright 確認は未実施。

## 2. 手順（本番設置・起動が可能になったとき）

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
