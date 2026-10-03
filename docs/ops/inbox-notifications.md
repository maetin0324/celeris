---
tasks: [01M3YFCJKMNWQ13HRS52M5BSWW]
---
# 受信箱と通知の外部送り出し設定

ADR-0133 D6 の Discord 送り出しを設定する人向けの手順。受信箱の新着はまとめて即時送り、通知は間隔ごとの要約として送る。

## 設定値

本番の `~/.config/celeris/config.toml` の `[notify]` に次を追加する。省略時は右欄の値が使われる。

| キー | 既定値 | 動作 |
|---|---:|---|
| `inbox_batch_secs` | `60` | 最初の判断待ちから何秒間、新着を束ねるか |
| `inbox_reminder_secs` | `86400` | 未回答の判断待ちを一度だけ再通知するまでの秒数 |
| `digest_interval_secs` | `3600` | 通知要約の間隔（`0` で要約を送らない） |
| `digest_max_lines` | `10` | 1 回の要約に含める最大行数 |

例:

```toml
[notify]
inbox_batch_secs = 60
inbox_reminder_secs = 86400
digest_interval_secs = 3600
digest_max_lines = 10
```

既存の `[notify]` がある場合は同じ節に値を加え、節を重複して作らない。Discord の webhook 秘密は TOML に書かず、既存手順どおり `discord-webhook` の secret として登録する。

## 反映と確認

1. 変更前に `config.toml` のバックアップを作り、上記の値を人が編集する。
2. 通常のリリース昇格手順で設定を読み直す。サービス再起動や本番設定の変更はこの文書では実行しない。
3. GUI の「アカウント → API キー」で webhook secret の登録を確認する。
4. `GET /api/v1/notify`（`celerisctl` の API 接続、または管理 GUI）で `configured` が有効であることと、秘密値が応答に含まれていないことを確認する。必要なら `POST /api/v1/notify/test` で test 送信を行い、Discord 側の受信を確認する。

`GET /api/v1/notify` は上記の `inbox_batch_secs`・`inbox_reminder_secs`・`digest_interval_secs`・`digest_max_lines` と、受信箱新着（`inbox_new_last_sent_at`）および要約（`digest_last_sent_at`）の最後の成功送信時刻を返す。該当経路の送信履歴が無い場合、時刻は `null` になる。

通常のリリース・昇格手順は [selfdeploy runbook](../selfdeploy.md) を参照する。受信箱の項目は `/api/v1/inbox/items`、通知一覧と既読状態は `/api/v1/notifications` で確認できる。
