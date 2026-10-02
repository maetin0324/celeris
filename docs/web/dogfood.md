# web phase 6 P6-03: dogfood の開始と運用

---
tasks: [01M3WAKKJQXT79DDDFCF9F5Q3D]
---

状態: **開始（2026-10-01 18:32 UTC、release `bf54b41ad627`）**。web gateway は本番 daemon `127.0.0.1:7710` に向けて loopback `127.0.0.1:7720` で稼働中。LAN 入口は `http://192.168.1.103:7721`。

人は P6-03 の gateway 起動と読み取り確認、LAN `192.168.1.103:7721` からの公開を選んだ。H6（実施期間と合格条件）は後から人が決め、決まるまで cutover はしない。gui/（`:7700`）は止めず、並行して使える状態を保つ。

起動時の `127.0.0.1:7720/healthz` と LAN 入口の `/healthz` はともに `release: bf54b41ad627`。Playwright で PC 幅 1440px（loopback）とスマホ幅 390px（LAN 入口）を確認し、両幅とも `/`・`/tasks`・`/projects`・`/org`・`/reports`・`/clusters` の 6 画面が HTTP 200 で見出しを表示した。各画面で daemon down 表示はなく、主要 API 6 件も両幅とも HTTP 200。ログイン以外の変更系リクエストは発生していない。gui/ の `127.0.0.1:7700/healthz` は引き続き HTTP 200。LAN の別端末そのものからの到達は未確認。

**配置上の注意:** 本番 release パス `~/.local/celeris/releases/bf54b41ad627` は、検証済み staging 成果物 `/var/lib/celeris/workspaces/01M3W79QAQ06PG1M0MCK5HRZ5K/artifacts/staging-state/releases/bf54b41ad627` への symlink。NFS の本番 release 領域への実体コピーは途中で中止した。元の staging 成果物を削除すると gateway の再起動が失敗するため、dogfood 中は保持する。

## 1. 継続中の条件と未決定事項

1. P6-03 の起動許可は回答済み。H6 の期間、合格条件、判定日は人が決め、決定後に `docs/PROGRESS.md` の dogfood 節へ記録する。H6 が未決定の間は cutover をしない。
2. H9（並行運用中のブラウザ通知の扱い）は人の決定待ち。現行の暫定方針は web/ の origin で利用者が許可したときだけ通知すること（[実装計画](implementation-plan.md) §3）。
3. P6-02 の `celeris-web@.service` と release の web 段を確認済み。H10 の staging 実 celeris 確認は release `bf54b41ad627` で完了済み（`verify.sh` exit 0、読み取り parity 3 passed）。本番で参照している `gate.json` は `web.ok: true`。

本番への配信切替は、この開始判断に含めない。gateway は loopback `127.0.0.1:7720` に bind。user systemd の `celeris-web-lan.socket` と `celeris-web-lan.service` が `192.168.1.103:7721` を `127.0.0.1:7720` へ中継する。`web.env` の `CELERIS_WEB_ALLOWED_HOSTS=192.168.1.103` で Host を許可した。両 unit は start のみで enable はしていないため、再起動後は手動起動が必要。

## 2. 起動・確認手順

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

`/healthz` の `release` が `<sha12>` と一致することを確認する。LAN 中継を再起動する場合は `systemctl --user start celeris-web-lan.socket` を使う。公開先 URL は `http://192.168.1.103:7721/`、認証情報は `~/.config/celeris/web.password` にある。selfdeploy の `scripts/selfdeploy/release.sh` は gui/ の gate の後に web 段を非 blocking で実行し、成功した release に `web/app/` を作る。`gate_ok` だけでは web 段の成功を示さないため、必ず `gate.json` の `web.ok` を見る。`verify.sh` と `promote.sh` は web gateway を起動しない（[web ADR-W3](adr/web-0003-parallel-operation.md) D3）。

### PC とスマホで確認する

- **PC:** gateway と同じホストでは `http://127.0.0.1:7720/login` を開く。LAN 内の別の PC では `http://192.168.1.103:7721/login` を開き、web 専用 password でログインする。
- **スマホ:** 同じ LAN につないで `http://192.168.1.103:7721/login` を開き、web 専用 password でログインする。別端末からの実到達と画面確認は人が行い、結果を下の問題記録節へ追記する。
- 両端末で作業中も gui/ の `http://127.0.0.1:7700/healthz` と既存の入口を確認し、gui/ を止めない。問題が出た場合は [並行運用手順](parallel-operation.md) §3 の web unit 停止手順を参照する。

## 3. 問題の記録場所

`docs/PROGRESS.md` の「Web GUI dogfood」節に、期間中の問題と対応を 1 件ずつ追記する。各問題は必要に応じて小さい修正タスクに分け、タスク ID と再確認結果を同じ行に追記する。

```md
## Web GUI dogfood（開始 <日付>、H6: <期間・合格条件>）

- H9: <通知の扱い>。H10: <staging 確認日・結果>。
- <日付>｜<画面>｜<端末: PC/スマホ・ブラウザ>｜<現象>｜<重大度>｜<対応・タスク ID・再確認結果>
```

## 4. 終了と判定

人が決めた H6 の期間が終わったら、`docs/PROGRESS.md` の記録を基に H6 の合格条件を人が判定し、結果と未解決事項を追記する。web/ への配信切替（H7、P6-04）は別の人の判断と承認を要する。dogfood の合格だけで入口を切り替えない。
