# ADR: 「自動取り込み」を案件の属性（DB）にし、自己リポジトリ以外も既定ブランチへ取り込む

- Date: 2026-10-16
- Status: Accepted
- 決定者: 人の依頼（2026-10-16「asmr案件も自動でmainに取り込む対象になっていて欲しい。案件ごとにWeb UIから設定できるように」）
- 関連: ADR-0051（supervised delivery）、ADR-0118（pre-review target sync）、ADR-0120（integration repair）、ADR-0121（root delivery）、
  [破壊的変更を是とする](2026-10-11-breaking-changes-by-default.md)

## Context

レビュー合格した root task を既定ブランチへ取り込む delivery の対象は config `[selfdeploy] delivery_projects`
（案件 ID の一覧）で決まり、取り込み先も `[selfdeploy] repo`（自己リポジトリ 1 件）に固定されていた。
asmr-archive のような自己リポジトリ以外の案件は `delivery_skipped: project_not_enabled`（または `repo_path_mismatch`）になり、
operator が毎回 `POST /tasks/{id}/integrations/{repo}/merge` で手で取り込んでいた。config は reload が要り、Web UI から変えられない。

## Decision

D1. **`projects.auto_integrate`（INTEGER NOT NULL DEFAULT 0）を案件の属性にする**（migration 0083。`Project.auto_integrate: bool`）。
    `delivery_projects` と、それを `DeliveryPolicy.projects` へ写す経路は**消す**。`DeliveryPolicy` は部署の既定
    （`default_departments`）と自己リポジトリの path（`repo`）だけを持つ。判定は `begin`（task-ops）と `tick`（celeris）が
    store の案件行を読んで行う（`auto_integrate = 0` の案件は従来どおり `delivery_skipped: project_not_enabled`、行は作らない）。

D2. **対象は案件のリポジトリ（task の repo 1 件）。** 取り込み先は task の repo 行（`project_repos.location` のローカル path）で、
    `[selfdeploy] repo` とは限らない。`repo_path_mismatch`（登録 path ≠ 自己リポジトリ）の拒否は廃止し、
    marker の `source` と登録 path の照合（`marker_repo_mismatch`）は残す。安全条件は既存のまま使う:
    対象 SHA 固定・reviewer の判定と merge candidate の照合（ADR-0118 D4）・target が進んでいたら再 sync → 再 check → 再 review
    （上限 `MAX_TARGET_RESYNCS`）・木の子は取り込まない・局所修復と統合 repair の上限・`git merge --ff-only`。
    task の repo が 2 件以上（`multiple_repos`）は従来どおり取り込まない。

D3. **自己リポジトリ以外は取り込みで止める。** 取り込み先の path が `[selfdeploy] repo` と同じ（canonicalize 一致）ときだけ
    従来どおり release 準備（`prepare.sh`）・`Ready`・promote の人の操作へ進む。それ以外のリポジトリでは `merge_reviewed` が成功した
    時点で `Delivery` を終端（`Ready`、`release = None`、detail に「取り込み済み（release なし）」）にし、`start_prepare` を起こさない。
    push は **config の `[selfdeploy] push` が有効で、かつ取り込み先に `[selfdeploy] push_remote` のリモートが実在するときだけ**行う
    （remote の無い asmr-archive は push しない。失敗の再試行は従来どおり 1 回）。

D4. **API と GUI。** `PATCH /api/v1/projects/{id}` の本文に `auto_integrate?: boolean` を足す（`GET /projects/{id}` と一覧の
    `project.auto_integrate` に出る）。管理トークンが要り、変えたら管理ログに `op = "project_updated"` と欄名を残す。
    CoS の操作（`cos/ops`）は名前と説明だけで、この欄は人の管理操作に残す。web の案件画面に切替を置く。

D5. **config 欄の廃止と移行。** `[selfdeploy] delivery_projects` は未知の欄として読み込みエラーにする
    （`deny_unknown_fields`）。既存の値（`01M2WTS3DKNZBSZ2JMVB4CZMBW`）は migration 0083 が
    `UPDATE projects SET auto_integrate = 1 WHERE id IN (...)` で新しい属性へ写す（案件が無ければ何も起きない）。
    本番 config から欄を消す手順は進捗ファイルの「本番への移行」節に書く。asmr-archive 案件を有効にするのは operator が API で行う。

D6. **非目標。** release/promote の自動化、複数リポジトリ task の取り込み、remote への push の新設は扱わない。

## Consequences

- 案件の自動取り込みは DB の属性なので reload・再起動を要さず、Web UI から切り替えられる。
- 自己リポジトリ以外の案件は、レビュー合格で既定ブランチへ ff されて終わる。stale・衝突は既存の repair・再レビューの経路に乗る。
- `delivery_projects` を書いた config は起動できなくなる。移行前に本番 config から消す（進捗ファイル参照）。
