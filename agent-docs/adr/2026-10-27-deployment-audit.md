# 配備・rollback の操作監査

## 決定

- promote と rollback は副作用の前に `SD_ACTOR`・`SD_REASON`・`SD_POLICY_REF` の空白以外の値を必須にする。actor は USER から推測しない。actor の語彙は下の「actor」の節に従う。
- 明示した関係は `SD_TASK_REFS` / `SD_DECISION_REFS` の JSON 文字列配列（省略時 `[]`）。commit から導出する included.tasks とは別に保存する。
- 対象 release の `audit/<一意ID>.json` に開始時の verify.json の内容、実行者・理由・方針・参照・from/to・operation・mode を保存し、終了時に status / exit_code / finished_at を確定する。強制終了でも started の記録を残す。同じ release の再昇格でも履歴を失わない。
- 成功時は promoted.json または rolled_back.json に同じ監査欄を写す。通常 rollback は promote へ operation=rollback を渡す。DB 復元の成否も同形式で残す。verify が無い復元先は null とし、成功を捏造しない。health / web の既存確認を通った結果は status=succeeded で表す。
- GET /releases の各項目の deployment_audit は操作記録の配列。既存 release に無い情報は補わず空配列とする。status.sh --json も同じ履歴を出す。
- 認証済み昇格 API・CoS 経路・CLI operator が渡す actor と policy_ref は下の「actor」の節に従う。CoS 経路は認証済み OperationAudit の reason / policy_version / operation_id を渡す。CLI operator は自身の具体的な理由・承認参照を渡す。

## actor

- 語彙は [ADR 2026-10-27-human-ops-actor-roles-request-ref](2026-10-27-human-ops-actor-roles-request-ref.md) D2 の actor_kind をそのまま使う。D2 が Celeris の actor_kind の唯一の語彙で、配備監査のための別の語彙は作らない。kind は `human` / `script` / `cos_project` / `cos_general` / `external_operator` / `mcp_client` / `unknown` の 7 種に限る。
- `SD_ACTOR` の形は `<kind>` または `<kind>:<id>`。id は任意だが、書くなら空でなく、メールアドレス（`@` を含む値）は書かない。`scripts/selfdeploy/audit.sh` は 7 種以外の kind、空の id、メールの id を副作用の前に拒否する。旧い語彙 `cos` / `operator:<name>` / `system:<name>` は互換の分岐を置かずに拒否する（外部 operator は `external_operator:<name>`、script・cron は `script` を使う）。
- 記録（`audit/*.json`、`promoted.json`、`rolled_back.json`）には、渡された `actor` に加えて `actor_kind` と `actor_id`（id が無ければ null）を分けて書く。
- 人の API（`POST /api/v1/releases/{sha12}/promote`）は 認証済み principal から `DeploymentContext::authenticated` で actor=`human:<name>` または `external_operator:<name>`、理由=authenticated release promotion request、policy_ref=`POST /api/v1/releases/<実際の sha12>/promote` を渡す。`{sha12}` の型板は残さない。
- CoS 経路（CoS operation `release.promote`）は `AuditContext.thread_id` の chat thread を読み、`ChatThreadKind::General` なら `cos_general:<thread id>`、`ChatThreadKind::Project` なら `cos_project:<thread id>` を渡す（`task_api::releases::cos_deployment_actor`）。legacy の thread と読めない thread は拒否し、推測で埋めない。

## 運用

本番の操作はこの task では行わない。初回導入では新しい scripts を使い、必須環境変数を明示する。既存記録は保持する。

## 付記 2026-10-27: 記録した後の credentiald 切替の失敗

credentiald の切り替え（ADR 2026-10-27-promote-switches-credentiald）は、昇格・rollback を記録した後に行う。この切り替えが失敗しても、監査記録は成功のまま残す。

- promote.sh（通常 rollback を含む）: `sd_audit finish 0` で audit と promoted.json（rollback では rolled_back.json も）を書き、`PROMOTED_RECORDED=true` にする。その後の失敗で EXIT trap の `record_promote_failure` は audit を書き換えない（`PROMOTED_RECORDED=true` なら `sd_audit finish` を呼ばない）。promote_failed.json も書かない。
- rollback.sh --restore-db: `current` を previous へ付け替え、promoting 印を片付けた直後に `sd_audit finish 0` を呼び、audit と rolled_back.json を書く。`AUDIT_FINISHED=true` で EXIT trap の二度目の finish を止める。それより前の失敗は従来どおり trap が status=failed で閉じる。
- 切り替えの失敗は `credentiald_switch_failed.json` だけに残し、exit 3 で知らせる。audit file・promoted.json・rolled_back.json に別欄は足さない。このため GET /releases の deployment_audit の型は変えない。
- 試験は `scripts/selfdeploy/tests/credentiald_switch.sh` の H・J・K。audit/*.json がちょうど 1 件で status=succeeded・exit_code=0 であること、promoted.json / rolled_back.json の operation・status・exit_code・時刻と一致することを確かめる。
