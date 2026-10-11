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

## 実装（2026-10-11）

- `gui/` と専用の check/sync script・unit・旧名称の初回移行台本を削除した。
  アプリ内の ADR は本文を変えず `agent-docs/gui/adr/` へ移した。同番号の設計案が既にある
  0001/0002 のアプリ内版は `agent-docs/gui/standalone/` に保持する。
- release の web gate と production dependency 梱包、verify の app 起動、promote の web 切替を必須にした。
  失敗した web で成功 release / promoted.json を作らない。旧 web の起動復旧と昇格再試行を試験する。
  `web-follow` は初回も起動し、失敗時は非0。current は web の健康確認後に更新する。
- `migrate-to-local` は web/app を除外せず、unit も同じ hot root を使う。作業ツリーの生成物清掃は web を対象とする。
- `[notify] gui_base_url` と API の同欄は `web_base_url` へ変更した。旧 config 欄は serde の
  `deny_unknown_fields` で失敗し、新欄名を示す。CLI の本人認証 socket は `CELERIS_WEB_OWNER_SOCKET` を使う。
- 現行 API 仕様は `docs/api/v1/api.md`、運用は `docs/ops/web-operation.md` に統合した。
  既存 ADR の削除済みソースへのリンクは `scripts/dev/retired-doc-links.tsv` の source/target 完全一致で
  歴史的参照と判定する。現行文書の欠落リンクは引き続き検査失敗になる。
