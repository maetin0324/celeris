# launcher Live View（protocol 9）の本番反映手順（人が行う）

---
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
---

決定は [ADR 2026-10-10 browser launcher Live View frames](../../agent-docs/adr/2026-10-10-browser-launcher-live-view-frames.md)（付記 2026-10-10c が版の正、付記 2026-10-10b が wire 契約）。版は Live View が protocol 9、artifact 転送（screenshot / download）が protocol 8。
前提の launcher 配置は [launcher の host 準備](browser-launcher-host-setup.md)、launcher 差し替えの元の手順は
[credential 解放手順](browser-launcher-credential-release.md)、Live View 全体の実機確認台本は [browser web Live View 実機確認](browser-web-live-check.md)。

この手順は運用セッションで人が実行する。worker は root 操作・本番設定の変更・daemon の差し替えをしない。Live View の画像
（frame）は保存・転記しない。screenshot を証跡に取らず、確認結果は「表示された／拒否された」と HTTP status だけを記録する。

## 0. 何が変わるか

- launcher が protocol 9 になり、session の画面を CDP screencast で取り、daemon が開いた **専用の frame 接続**でだけ流す。
  通常の control 接続には frame を流さない。input の verb は無い（読み取り専用）。
- daemon は frame を容量 1 の最新 frame slot で中継し、task-api の `POST /api/v1/tasks/{id}/browser/live/{run}/{session}/frames`
  が grant を再確認しながら本人の gateway にだけ流す。web gateway は `/browser/live/{task}/{run}/frames` の WebSocket で
  owner session に送る。frame はどの永続層（DB・events・progress・log・artifacts・tmp）にも残らない。
- `/browser/runs` の Live View は、映像経路があれば `link`、launcher が protocol 8 以下（v8 は artifact のみ、v7 以下も含む）なら
  理由 `launcher_protocol_no_live_frames` で `disabled` になる。

## 1. 差し替え順と互換の根拠

daemon（celeris・web を含む release）と launcher のどちらを先に差し替えてもよい。間の期間は Live View が出ないだけで、
session・credential login・post-login 読み取り・consent はそのまま動く。

| daemon | launcher | Live View | その他の機能 |
|---|---|---|---|
| Live View 入り release（daemon は 9 を要求） | v7 以下（旧） | `disabled`、理由 `launcher_protocol_no_live_frames`。daemon は `live_start` を送らない | session・login・consent は従来どおり。screenshot / download は v8 launcher を要する |
| Live View 入り release | v8（artifact のみ） | `disabled`、理由 `launcher_protocol_no_live_frames` | 従来どおり（screenshot / download を含む） |
| Live View 入り release | v9（新） | 本人に映像 | 従来どおり |
| Live View 導入前の release | v9（新） | daemon が frame 接続を開かないので frame なし | 従来どおり |

根拠:

- daemon は機能ごとに launcher の版を下限で比べる（credential login 4、username 5、post-login 6、consent 7、artifact 8、Live View 9。
  `crates/task-worker/src/browser_launcher/protocol.rs`）。版の値の正は付記 2026-10-10c。
  Live View は hello の版を見て 9 未満なら接続せずに理由を返し、session を失敗させない
  （試験 `browser_launcher_daemon_checks_live_protocol_before_enable`）。
- v9 launcher は frame を、daemon が `live_start` を送った別接続でだけ流す。`live_start` を送らない daemon には frame を送らないので、
  要求していない通知が同期 request/response に混ざることはない（試験 `browser_launcher_v8_continues_without_live_view` は v8 launcher 側の固定）。

注意: `celerisctl browser doctor` の `launcher` 行は daemon の版と launcher の版の**完全一致**を見る（`crates/celeris/src/browser_doctor.rs`）。
両方を差し替え終わるまで `launcher` は NG（期待どおり）。NG のまま他の行（ledger・backend）が OK なら、上の互換により既存の browser task は動く。

推奨順: release の昇格（daemon・web） → launcher の差し替え → 台帳の再生成 → 確認。稼働中の browser session がある間は
launcher を止めない。

## 2. release の昇格と web の追従

通常の selfdeploy（[selfdeploy.md](selfdeploy.md)）で Live View を含む release を昇格する。`promote.sh` が `web-follow.sh` で
web gateway も新 sha に切り替える（[selfdeploy.md §4e](selfdeploy.md#4e-web-の追従web-followsh)）。昇格後:

```sh
SHA12=<昇格した sha12>
systemctl --user list-units 'celeris*' --no-pager   # celeris@$SHA12 と celeris-web@$SHA12 が active
celerisctl browser doctor                           # launcher 以外が OK。launcher は旧 v7 のうち NG
```

この時点で `/browser/runs` の launcher run は Live View が `disabled`（`launcher_protocol_no_live_frames`）で表示される。

## 3. launcher の再 build と差し替え（root）

launcher は昇格した release と同じ HEAD から作る。target は NFS ではなく `/local` の scratch に置く（release の後始末は
`.cargo-target` を消すので使わない）。既存 binary を退避し、hash を記録する。現在の launcher が v7 でも v8（artifact のみ）でも、
同じ手順で protocol 9 に上がる。退避版の版は `celeris-browser-launcher` の hello で確かめて記録しておく（§6 の戻し先）。

```sh
W=<配送された agent-platform worktree（昇格した sha の checkout）>
SHA12=<昇格した sha12>
L=/usr/local/libexec/celeris/celeris-browser-launcher
T=/local/celeris/data/scratch/launcher-$SHA12-target
cd "$W"
git rev-parse HEAD                      # 昇格した sha と一致
grep -n 'pub const PROTOCOL_VERSION' crates/task-worker/src/browser_launcher/protocol.rs   # = 9
CARGO_TARGET_DIR=$T cargo build --release -p task-worker --bin celeris-browser-launcher
sha256sum "$T/release/celeris-browser-launcher" "$L"
install -o root -g root -m 0755 "$L" "$L.pre-live-view"
pgrep -u celeris-browser -a             # 稼働中の session が無いこと（あれば終わるまで待つ）
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 "$T/release/celeris-browser-launcher" "$L"
sha256sum "$L"                          # build した値と一致
systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
```

daemon の実行 user で必須実証を取る（出力に秘密は無い）。どちらも最終行が `EXIT: 0` であること。

```sh
cd "$W"
sh crates/task-worker/scripts/launcher-admission-evidence.sh --stutter 3 /var/tmp/launcher-admission-stutter.log
sh crates/task-worker/scripts/launcher-admission-evidence.sh --credential /var/tmp/launcher-credential-tests.log
celerisctl browser doctor
```

doctor の `launcher` 行が `OK  launcher  固定 Hello IPC（protocol 9、本番は loopback 許可なし）…` になったことを確かめる。
確認は hello の版そのものでも取れる: `celerisctl browser doctor` が `OK` なら launcher の版は daemon の `PROTOCOL_VERSION`（9）と一致している。
`protocol 8` と出るなら launcher が差し替わっていない（§3 をやり直す）。
NG なら launcher と daemon の版がずれている（launcher が古い・daemon が旧 release のまま）か、socket/許可 UID の問題で、
[host 準備](browser-launcher-host-setup.md) §6 の確認に戻る。build に使った target は確認後に消してよい（`rm -rf "$T"`）。

## 4. 台帳の再生成

launcher を差し替えたら release の browser 台帳を作り直す。`browser-ledger.sh` は既定で `$SD_RELEASES/.cargo-target` を使うが、
release の後始末がそれを消すので、`/local` の scratch を `CARGO_TARGET_DIR` で明示する。

```sh
CARGO_TARGET_DIR=/local/celeris/data/scratch/ledger-$SHA12-target bash scripts/selfdeploy/browser-ledger.sh $SHA12 --force
celerisctl browser ledger check --file ~/.local/celeris/releases/$SHA12/browser/conformance.json
celerisctl browser doctor
rm -rf /local/celeris/data/scratch/ledger-$SHA12-target
```

`ledger-status.json` が `ok: true`、doctor の `ledger`・`ledger-backends`・`launcher`（protocol 9）が OK であること。
daemon は台帳の mtime を見て再起動なしに拾う。

## 5. Live View の確認

owner session（本人が login した web）と、別の確認用の手段を使う。画像は保存しない。

1. **本人に映像が出る**: 公開 origin だけを開く browser task を 1 件走らせ、`/browser/runs` で RUNNING の run の Live View が
   `link` になり、開くと画面が更新されることを見る。映像の下に「読み取り専用の映像です。入力や操作はできません。」が出ること。
2. **credential session（ログイン中・ログイン後）も本人に出る**: credential を使う task（例: manaba の課題監視、
   [credential login v5](browser-credential-login-v5.md) §5）を承認して走らせ、`waiting_for_auth` から login・ログイン後の頁まで、
   owner session の Live View に映像が出続け、「credential session の映像は本人だけに表示されます。」が出ることを見る（人の決定 live-credential-default=a）。agent 側の観測（progress・
   snapshot）は auth section の間止まったままで、Live View の映像とは無関係であること。
3. **非 owner に見えない**: 同じ run の frame 経路に、cookie なしと、owner でない login 済み session で接続し、拒否されることを見る。

   ```sh
   H=<web の origin（例 https://celeris.example）>; P=/browser/live/<task>/<run>/frames
   K=$(head -c16 /dev/urandom | base64)
   curl -s -o /dev/null -w '%{http_code}\n' -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
     -H 'Sec-WebSocket-Version: 13' -H "Sec-WebSocket-Key: $K" -H "Origin: $H" "$H$P"            # 401
   curl -s -o /dev/null -w '%{http_code}\n' -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
     -H 'Sec-WebSocket-Version: 13' -H "Sec-WebSocket-Key: $K" -H "Origin: $H" \
     -b '<非 owner session の cookie>' "$H$P"                                                       # 403
   ```

   cookie の値は shell history に残さない（`read -s` で変数に入れる等）。owner の cookie では試さない（成功すると frame が
   端末に流れる）。agent・LLM へは経路そのものが無い（task-api の frame route は署名 assertion と owner grant が要る）。
4. **input が届かない**: owner の Live View 上で click・key 入力をしても、browser の頁に何も起きないこと。viewer から WebSocket に
   data を送ると gateway は接続を閉じる（試験 `browser_launcher_live_view_rejects_input`・gateway の viewer input close）。
   人が操作するときは従来の takeover（pause → takeover の lease）を使う。
   映像が出ないときは web の log（`journalctl --user -u celeris-web@$SHA12`）を見る。frame の upgrade は
   `{"path":"/browser/live/frames",...,"status":101}`（開いた）か `"status":409,"code":"auth_interval"` 等（拒否）を 1 行残し、
   `/browser/runs` が link を出さなかった run は `{"event":"browser_live_unavailable",...,"reason":...}` を残す。
   画面の Live View 枠にも同じ理由の文が出る（ADR 付記 2026-10-10e）。
5. **後始末**: run の終了・lease 失効・owner の logout で Live View が閉じ、`/browser/runs` が `disabled`（`not_running` 等）に
   戻ること。`journalctl --user -u celeris@$SHA12` と web の log に frame の中身（base64 等）が出ていないこと。

## 6. 戻し方

Live View に問題（他人に見える・input が届く・frame が残る疑い）があれば、まず owner session を logout して viewer を閉じ、
launcher だけを退避版（Live View 導入前の版。v7 または v8）に戻す。daemon は Live View 入りのままでよい（§1 の互換で Live View が
`disabled` になり、他の機能は動く。退避版が v7 なら screenshot / download は失敗する点に注意）。

```sh
pgrep -u celeris-browser -a             # session が無いこと
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 "$L.pre-live-view" "$L"
sha256sum "$L"                          # §3 で記録した差し替え前の hash と一致
systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
CARGO_TARGET_DIR=/local/celeris/data/scratch/ledger-$SHA12-target bash scripts/selfdeploy/browser-ledger.sh $SHA12 --force
celerisctl browser doctor               # launcher は版不一致で NG（期待どおり）、ledger・backend は OK
```

`/browser/runs` の Live View が `launcher_protocol_no_live_frames` になることを確かめる。daemon・web も戻す必要があるとき
（release 自体の不具合）は、selfdeploy の通常の戻し（直前 release への promote）を使う。その場合、Live View 導入前の daemon と
v9 launcher の組でも他の機能は動くが、launcher も揃えて戻すなら上の手順で退避版を戻す。退避版が無いときは直前 release の HEAD から
§3 の手順で再 build する（その HEAD の `PROTOCOL_VERSION` が退避先の版になる）。
