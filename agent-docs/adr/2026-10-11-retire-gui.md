# ADR: 旧 GUI を廃止し web に一本化する

- Date: 2026-10-11
- Status: Accepted
- Tasks: 01M4NAKDCESYK36PNY7TTCTJ5E

## Decision

人の決定に従い、Remix の `gui/`、専用の build・deploy・検査・unit を削除する。
ADR-0013/0020 等の過去の判断は履歴として保持し、現行仕様は本 ADR で更新する。
互換 flag・旧経路は残さず、救出は git と release rollback による。

削除前に、旧画面限定だった定期実行の一覧・詳細・履歴・一時停止/再開/手動実行と、
WorkUnit 検査ログ末尾の表示・実行中の更新を web に実装する（人の追加決定）。
既存の `/api/v1/cron-jobs` と `/api/v1/tasks/{id}/work-units/{wu_id}/check-log` を使う。
検査ログはタスク詳細の独立した領域に追加し、並行タスクが扱う木の表示を変更しない。

web gateway は daemon に直接接続し、login・SSE・ファイル配信・Browser 本人承認・Live View を自ら提供する。
web の build・test・配布・verify は必須の release gate とする。
DB・events・credential・cron の daemon/API/CLI は保持する。本番の unit・proxy・port の変更は
進捗ファイルの「本番への移行」に記載し、運用セッションで実行する。
