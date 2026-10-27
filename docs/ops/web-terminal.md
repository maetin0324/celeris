# web 端末（クラスタ画面からの対話シェル）

---
tasks: [01M5ZA2NP31AVVKQ9F4T3A52VB]
---

web のクラスタ画面から、celeris が動くこのマシンと登録済みクラスタの対話シェルを開く。設計は
[ADR 2026-10-27-web-terminal](../../agent-docs/adr/2026-10-27-web-terminal.md)。

## 使い方

1. tailscale の URL か、このマシンの `http://127.0.0.1:7720` で web に login する。LAN の `:7721`（平文）からは開けない。
2. クラスタ画面の「操作」で、このマシンの行かクラスタの行の「端末を開く」を押す。
   クラスタは ssh 接続（ControlMaster）が無いと押せない。先に「接続」（TOTP が要るクラスタはコード入力）で繋ぐ。
   作業ディレクトリが未設定のクラスタも開けない。
3. 画面下の「端末」に web のパスワードを入力する（step-up。既定 600 秒で失効し、失効後に開くときは再入力）。
4. タブで複数の端末を切り替える。スマートフォンでは補助キー（Ctrl・Esc・Tab・矢印）とコピー・貼り付けを使う。
5. 「終了」で閉じる。タブを閉じる・画面を離れる・通信が切れると端末は終わる（再接続はしない）。

注意:

- このマシンの端末は celeris を動かすユーザー（rmaeda）の権限で動く。sudo も使える。
- クラスタの端末で `ssh -O exit <host>` を打つと worker が使う接続も閉じる。
- daemon の再起動では端末は残る。release の昇格（web の入れ替え）・web の再起動では終わる。
- 入出力は記録しない。開始と終了（対象・時刻・接続元・理由・秒数）は DB の表 `terminal_session_events` に残る。

## 設定（`~/.config/celeris/web.env`）

| 変数 | 既定 | 意味 |
|---|---|---|
| `CELERIS_WEB_TERMINAL` | `on` | `off` で端末を止める |
| `CELERIS_WEB_TERMINAL_HOSTS` | 空 | loopback の他に端末を許す Host（カンマ区切り）。tailscale の名前を入れる |
| `CELERIS_WEB_TERMINAL_MAX_SESSIONS` | `4` | 同時に開ける端末の数（web 全体） |
| `CELERIS_WEB_TERMINAL_IDLE_SECS` | `1800` | 入出力が無いまま経つと閉じる秒数 |
| `CELERIS_WEB_TERMINAL_MAX_SECS` | `28800` | 1 端末の最大継続秒数 |
| `CELERIS_WEB_TERMINAL_STEP_UP_SECS` | `600` | パスワード再入力の有効秒数 |
| `CELERIS_WEB_TERMINAL_LAUNCHER` | `auto` | `systemd-run`（scope に入れる）/ `inline` / `auto` |
| `CELERIS_WEB_TERMINAL_MEMORY_MAX` | `2G` | scope の `MemoryMax` |
| `CELERIS_WEB_TERMINAL_CPU_QUOTA` | `200%` | scope の `CPUQuota` |
| `CELERIS_WEB_TERMINAL_TASKS_MAX` | `512` | scope の `TasksMax` |
| `CELERIS_WEB_TERMINAL_HELPER` | `<release>/bin/celerisctl` | PTY helper（`celerisctl terminal-pty`） |

daemon 側は追加の設定を持たない。web の token（`human:web`）だけが `POST /api/v1/terminals` を通る。

## 本番で有効にする手順（operator が利用者に告知してから行う）

1. tailscale の名前を確かめる（読むだけ）: `grep CELERIS_WEB_ALLOWED_HOSTS ~/.config/celeris/web.env`
2. `~/.config/celeris/web.env` に `CELERIS_WEB_TERMINAL_HOSTS=<tailscale の名前>` を足す。
3. この変更を含む release を昇格する（web が入れ替わり、新しい env を読む）。
4. 確認: tailscale の URL で login し、クラスタ画面で「このマシン」の端末を開いて `id` と `pwd` を打つ。
   LAN の `http://192.168.1.103:7721` では端末の欄に「この入口からは端末を開けません」と出ることを確かめる。
5. 記録の確認（読むだけ）:
   `sqlite3 -readonly <db> "SELECT ts, kind, target, actor, reason FROM terminal_session_events ORDER BY id DESC LIMIT 5"`
6. 端末が残っていないこと: `systemctl --user list-units 'celeris-terminal-*'` が空。

## 止め方・戻し方

- すぐ止める: `web.env` に `CELERIS_WEB_TERMINAL=off` を書き、`systemctl --user restart celeris-web@<sha12>.service`。
  開いている端末は web の停止で全て終わる。
- 残った端末を止める（通常は不要）: `systemctl --user stop 'celeris-terminal-*.scope'`。
- 機能を戻す: この変更を `git revert` した release を作り、昇格する。表 `terminal_session_events` は残す（消さない）。
