# CLI の API 接続設定を ApiConfig に集約する

- 日付: 2026-11-15
- 状態: Accepted
- tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]

## 決定

`celerisctl` の daemon command の接続設定は `cron::ApiConfig` が所有する。
現行コードには候補一覧が正本として指定した `from_config` がまだ無いため追加する。
`from_config(path, url, token_file)` は明示 path、CELERIS_CONFIG、既定 path の順で config を選び、
listen と client_token を読む。CoS run credential があれば人の token を読まない。
URL override は明示値、CELERIS_API_URL の順とし、従来の CoS 同様 /api/v1 を要求し、
config と人の token は読まない。browser doctor の明示 token file は最後に適用する。
これらの解決も ApiConfig に置き、command 側に写しを残さない。
cron・integration・models・script も URL 環境変数を利用できるようになる。
release notes の任意 HTTP lookup は daemon command の config 解決とは別契約であり対象外。

## 実装前 brief

1. 根本原因: command ごとの config/listen/token の写しが接続先の差を生んでいる。
2. owner: architecture-map の celerisctl 行。接続設定は cron::ApiConfig、未参照関数は各定義 module。
3. 再利用: Config::load、api.client_token、cos_ops の URL 優先順、cron の HTTP transport と既存試験。
4. 選択肢: 各写しの修正より、指定済みの ApiConfig に一本化する。新しい共通基盤は作らない。
5. 新しい状態・fallback・port: 無し。既存の URL override と token file の解決を owner に移す。
6. 撤去: command の重複解決、未参照の薄い wrapper・集計 accessor・TOP_DIRS と警告専用 helper。
7. 不変条件: credential は漏らさず、run credential 時に人の token を読まない。時計注入発行・KB validator・レビュー判定は残す。
   既存 CoS/cron/browser 試験と接続設定の優先順試験で確認する。新試験は従来無かった共通 resolver の契約を守る。
