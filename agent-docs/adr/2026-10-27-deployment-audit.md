# 配備・rollback の操作監査

## 決定

- promote と rollback は副作用の前に `SD_ACTOR`・`SD_REASON`・`SD_POLICY_REF` の空白以外の値を必須にする。actor は `human` / `operator:<name>` / `cos` / `system:<name>` の語彙を使い、USER から推測しない。
- 明示した関係は `SD_TASK_REFS` / `SD_DECISION_REFS` の JSON 文字列配列（省略時 `[]`）。commit から導出する included.tasks とは別に保存する。
- 対象 release の `audit/<一意ID>.json` に開始時の verify.json の内容、実行者・理由・方針・参照・from/to・operation・mode を保存し、終了時に status / exit_code / finished_at を確定する。強制終了でも started の記録を残す。同じ release の再昇格でも履歴を失わない。
- 成功時は promoted.json または rolled_back.json に同じ監査欄を写す。通常 rollback は promote へ operation=rollback を渡す。DB 復元の成否も同形式で残す。verify が無い復元先は null とし、成功を捏造しない。health / web の既存確認を通った結果は status=succeeded で表す。
- GET /releases の各項目の deployment_audit は操作記録の配列。既存 release に無い情報は補わず空配列とする。status.sh --json も同じ履歴を出す。
- 認証済み昇格 API は actor=human、理由=authenticated release promotion request、policy_ref=POST /api/v1/releases/{sha12}/promote を明示して渡す。CoS 経路は actor=cos とし、認証済み OperationAudit の reason / policy_version / operation_id を渡す。CLI operator は自身の具体的な理由・承認参照を渡す。

## 運用

本番の操作はこの task では行わない。初回導入では新しい scripts を使い、必須環境変数を明示する。既存記録は保持する。
