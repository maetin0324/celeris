---
tasks: [01M3YFCJKMNWQ13HRS52M5BSWW]
---
# 受信箱と通知の外部送り出し設定

Discord への送り出しを設定する人向けの手順。2026-10-05 から、人への通常の通知は CoS が行う（ADR-0133 の付記「CoS チャットと受信箱一次対応への置き換え」、ADR 2026-10-05-cos-chat-home）。Discord へ直接送るのは、CoS の escalation と CoS が使えないときの退避の 2 つだけ。

## 設定値

本番の `~/.config/celeris/config.toml` の `[notify]` で効く欄は次の 3 つ。

| キー | 既定値 | 動作 |
|---|---:|---|
| `discord_webhook_secret` | `discord-webhook` | webhook URL を登録した secret の id。secret が無ければ何も送らない |
| `interval_secs` | `30` | 送り出しを行う間隔（秒） |
| `web_base_url` | なし | 文面に入れる GUI のリンクの根（例: `http://192.168.1.103:7720`） |

`inbox_batch_secs`・`inbox_reminder_secs`・`digest_interval_secs`・`digest_max_lines` は ADR-0133 D6 の受信箱の新着と通知の要約の 2 経路の設定だった。この 2 経路は 2026-10-05 に CoS へ置き換わり、どの送り出しもこの 4 欄を読まない。設定しない。設定の読み込みは今もこの 4 欄を受け付け、`GET /api/v1/notify` が値を返すが、送り出しには効かない。撤去は task 01M7J3NKVC2KQ4NGJ4GESHJV0B（task-api の `GET /api/v1/notify` の 4 値を消す変更）が行い、同じ変更でこの 4 欄は読み込みエラーになる。

Discord の webhook 秘密は TOML に書かず、既存手順どおり `discord-webhook` の secret として登録する。既存の `[notify]` がある場合は同じ節に値を加え、節を重複して作らない。

## 反映と確認

1. 変更前に `config.toml` のバックアップを作り、値を人が編集する。
2. 通常のリリース昇格手順で設定を読み直す。サービス再起動や本番設定の変更はこの文書では実行しない。
3. GUI の「アカウント → API キー」で webhook secret の登録を確認する。
4. `GET /api/v1/notify`（`celerisctl` の API 接続、または管理 GUI）で `configured` が有効であることと、秘密値が応答に含まれていないことを確認する。必要なら `POST /api/v1/notify/test` で test 送信を行い、Discord 側の受信を確認する。

`GET /api/v1/notify` の `inbox_new_last_sent_at`・`digest_last_sent_at` は、2026-10-05 より前に旧い 2 経路が送った通知の最後の成功送信時刻を返す。今はこの 2 種の通知を作らないので、値は増えない（履歴が無い DB では `null`）。

通常のリリース・昇格手順は [selfdeploy runbook](selfdeploy.md) を参照する。受信箱の項目は `/api/v1/inbox/items`、通知一覧と既読状態は `/api/v1/notifications` で確認できる。
