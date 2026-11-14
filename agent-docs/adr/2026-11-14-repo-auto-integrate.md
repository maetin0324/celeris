# ADR: 自動取り込みの切り替えを案件からリポジトリ（`project_repos` の行）へ移す

---
tasks: [01M7BSKR87T3ERB9EJVXBZQTFB]
---

- Date: 2026-11-14
- Status: Accepted
- 決定者: 人の依頼（2026-11-14、運用 session への答え「リポジトリ毎に切り替えられるようにするタスクを起票してください」）
- 置き換えるもの: [ADR 2026-10-16-project-auto-integrate](2026-10-16-project-auto-integrate.md)（案件の属性 `projects.auto_integrate`）
- 関連: ADR-0043（案件のリポジトリ）、ADR-0051（supervised delivery）、ADR-0118（pre-review target sync）、ADR-0121（root delivery と `DeliverySkipped`）、
  [破壊的変更を是とする](2026-10-11-breaking-changes-by-default.md)、[CoS の操作](2026-10-09-cos-operations-all-mutations.md)

## Context

レビュー合格した root task を既定ブランチへ自動で取り込むかは、案件の属性 `projects.auto_integrate`
（ADR 2026-10-16-project-auto-integrate D1）で決まり、案件のリポジトリごとには切り替えられない。

BenchFS の案件（01M35WRV77A2JPYGERQGXF6V7K）は、Sirius 上のコードのリポジトリ `benchfs`（remote、primary）と、
この host の論文のリポジトリ `benchfs-paper`（local、01M7A3NAAKPM9ECBE2CR14V0BC）を持つ。人は論文だけを
review の合格で自動で main に入れたい。案件で有効にすると、remote の `benchfs` の root task が done になるたびに
`task_ops::delivery::prepare` が `DeliverySkipped`（`no_marker` か `repo_not_local`）を記録し、受信箱
（`task_ops::inbox` の `AttentionItem::DeliverySkipped`）に「取り込みを開始できませんでした」が 1 件出る。
そのため案件は無効のままで、論文の取り込みは運用 session が毎回手で merge している（2026-11-13・14 の 2 回）。

案件の欄を読む所は 4 つある: `task_ops::delivery::prepare`（delivery を始めるか）、`celeris::delivery::tick_with`
（delivery 行を進めるか）、`task_dispatch::dispatcher::review_spawn`（reviewer run を必須にするか）、
`task_ops::stranded::reintegrate`（done の task を再取り込みするか）。受信箱は欄を読まず、記録された理由をそのまま出す。

## Decision

D1. **欄は `project_repos.auto_integrate`（INTEGER NOT NULL DEFAULT 0）。既定は false。**
    `ProjectRepo.auto_integrate: bool`（serde の既定 false）。API は `POST /api/v1/projects/{id}/repos` と
    `PATCH /api/v1/repos/{id}` の本文に `auto_integrate?: boolean` を足し、リポジトリの一覧・`GET /projects/{id}` の
    `repos[]` に出す。管理トークンが要り、変えたら管理ログに `op = "repo_update"`（作成は既存の op）と欄名を残す。
    案件の欄（`projects.auto_integrate`・`Project.auto_integrate`・`PATCH /projects/{id}` の `auto_integrate`）は消す（D8）。

D2. **取り込めるリポジトリだけを有効にできる。remote のリポジトリで有効にする要求は 422。**
    `auto_integrate = true` を持てるのは `kind = git` かつ `location` が `local` の行だけ。delivery は登録 path の
    ローカルの git で `merge --ff-only` するので、remote（`repo_not_local`）と `dir`（`not_git`）は有効にしても
    必ず取り込みに失敗する。
    - `POST /projects/{id}/repos` で remote または `dir` の行に `auto_integrate: true` を書けば 422。
    - `PATCH /repos/{id}` は、変更を当てた後の行が remote または `dir` で `auto_integrate = true` なら 422。
      `location` を remote に変える・`kind` を `dir` に変える要求で、既に有効な行も 422（人は同じ要求で
      `auto_integrate: false` を書くか、先に無効にする）。
    - 黙って false に落とさない。`sync`・`default_branch` は意味を失うだけなので落とすが、`auto_integrate` は
      人が与えた取り込みの許可で、落とすと人が有効だと思っている間に取り込まれない状態になる。
    - 条件の正本は task-core のリポジトリの検証（store の `repo_insert`・`repo_update` が拒否する）に置き、API は
      その拒否を 422 に写す。CoS の操作・CLI・API のどの経路でも同じ検証を通る。

D3. **「task が自動取り込みの対象か」の判定は 1 つの関数にし、4 か所と受信箱がそれを使う。**
    関数は `task_ops::delivery` に置く（名前は実装で決める）。条件: task に `project_id` があり、`task.repos` が
    1 件以上あり、`task.repos` のすべての repo について案件のリポジトリの行が存在し `auto_integrate = true`。
    行が無い repo は無効として扱う。support task と木の子を外す条件は、今と同じく呼び出し側に残す。
    - `prepare`（`begin`）: 判定が偽なら、今の `project_not_enabled` と同じ位置（`no_project` の次）で
      `DeliverySkipped { reason: repo_not_enabled }` を記録して止める。marker・登録 path・remote の判定へ進まない。
    - `celeris::delivery::tick_with`: delivery 行の task に同じ判定を当て、偽なら `advance` しない（task の行が
      無ければ進めない）。走行中に人が無効にすると次の tick から止まる（旧 ADR の案件の欄と同じ）。
    - `review_spawn` の `delivery_requires_reviewer_run`: 判定が真で、support task でも木の子でもないとき reviewer run を必須にする。
    - `stranded::reintegrate`（D1.2）: done の root task の再取り込みは判定が真の task だけ。
    - 受信箱: D4。

D4. **無効のリポジトリの root task は、受信箱に「取り込み始められなかった」を出さない。**
    - 理由 `project_not_enabled` を `repo_not_enabled`（表示「リポジトリの自動取り込み（auto_integrate）が有効ではありません」）
      に置き換える。event は今と同じく (task, reason, head) ごとに 1 件だけ追記する（取り込みが始まらなかった理由の監査）。
    - 受信箱の `DeliverySkipped` の項目は、理由が `no_commits` でも `repo_not_enabled` でもなく、かつ読んだ時点で
      task が D3 の判定を満たすときだけ出す。読んだ時点の判定も見るのは、無効にしたリポジトリの過去の項目
      （移行前に案件で有効だった remote の repo の `no_marker`・`repo_not_local` など）も消えるようにするため。
    - events は追記専用で、過去の `DeliverySkipped` の `reason = "project_not_enabled"` は書き換えない。読み込みでは
      `repo_not_enabled` として読む（serde の alias など、読む側の対応表 1 か所）。旧い意味「案件が無効」は
      「案件のすべてのリポジトリが無効」なので、新しい意味に含まれる。これは events を読み続けるための恒久の読み替えで、
      新旧の挙動を切り替える分岐ではない（撤去の予定は無い）。
    - 既存の未取り込み（`AttentionItem::Unintegrated`）の項目の対象は変えない。

D5. **複数リポジトリの task（`multiple_repos`）は、すべての repo が有効のときだけ自動取り込みの対象にする。**
    delivery は 1 task 1 repo で動く（`deliveries` の行は `repo_id` を 1 件持ち、merge・repair・再 review の単位も 1 repo）。
    複数リポジトリの取り込みは今も非目標（D9）なので、対象になった task も今と同じく `prepare` が `multiple_repos` で止める。

| task の repo | D3 の判定 | reviewer run | `prepare` の結果 | 受信箱 |
|---|---|---|---|---|
| 1 件・有効 | 真 | 必須 | delivery を始める | — |
| 1 件・無効 | 偽 | 必須にしない | `repo_not_enabled` | 出さない |
| 2 件以上・すべて有効 | 真 | 必須 | `multiple_repos` | 出す（今の有効な案件と同じ） |
| 2 件以上・一部か全部が無効 | 偽 | 必須にしない | `repo_not_enabled` | 出さない |

理由:
1. 複数リポジトリの task の変更は 1 つのまとまり（例: コードと論文）で、reviewer はその全体を判定する。
   有効な repo の分だけを main に入れると、変更の半分だけが main に入り、残りはブランチに残る。
2. 人の許可はリポジトリごとに与える。許可していない repo に書く task が、その repo を理由に取り込みや
   reviewer run の費用を引き起こしてはいけない。「どれか 1 つが有効なら対象」にすると、BenchFS の `benchfs`
   と `benchfs-paper` の両方に書く task で reviewer run が必須になり、`multiple_repos` の項目が受信箱に出る（今回消したい通知が戻る）。
3. 将来、複数リポジトリの取り込みを作るときも前提は「すべて有効」なので、判定を変えずに済む。

D6. **切り替えは人の管理操作に残す。** 旧 ADR D4 と同じく、CoS の操作（`cos/ops` の `repo.create`・`repo.update`）は
    `auto_integrate` を書けない。本文にこの欄があれば 422 にし、拒否の理由を操作の記録に残す（CoS が自分で
    既定ブランチへの自動 merge を許可できないようにする）。人は web のリポジトリの設定か API で切り替える。

D7. **migration の方針: `auto_integrate = 1` の案件の local の git リポジトリを有効にしてから、案件の列を消す。**
    新しい migration（次の番号）で次を 1 つの migration として行う:

```sql
ALTER TABLE project_repos ADD COLUMN auto_integrate INTEGER NOT NULL DEFAULT 0;
UPDATE project_repos SET auto_integrate = 1
 WHERE kind = 'git'
   AND json_extract(location_json, '$.kind') = 'local'
   AND project_id IN (SELECT id FROM projects WHERE auto_integrate = 1);
ALTER TABLE projects DROP COLUMN auto_integrate;
```

- 今 有効な案件（agent-platform の 01M2WTS3DKNZBSZ2JMVB4CZMBW など）で 1 件の local の git リポジトリを使う
  root task は、移行の前後で同じ動き（reviewer 必須・delivery・tick・再取り込み）をする。決定的な試験で確かめる。
- 有効な案件の remote・`dir` のリポジトリは D2 により有効にできないので無効になる。その task は reviewer 必須でなくなり、
  受信箱に `no_marker`・`repo_not_local`・`not_git` を出さなくなる。本番にそういう行があるかは未確認で、
  移行の前に `SELECT` で数える手順を進捗ファイルの「本番への移行」節に書く。
- データは失わない: 案件の欄の値は、その案件の取り込めるリポジトリの欄に写してから消す。events は触らない（D4）。
- `benchfs-paper` を有効にするのは migration ではなく人の操作（`PATCH /api/v1/repos/01M7A3NAAKPM9ECBE2CR14V0BC`
  に `{"auto_integrate": true}`）で、手順は進捗ファイルの「本番への移行」節に書く。

D8. **旧い仕組みは同じ変更で消す。** 消すもの:
    - DB の列 `projects.auto_integrate`（D7）と `Project.auto_integrate`、store の `project_set_auto_integrate`（trait・impl・`_tx`）、
      案件の行を読む SQL の列。
    - `PATCH /api/v1/projects/{id}` の `auto_integrate`。この欄を書いた要求は 422（未知の欄）。API の文書と schema からも消す。
    - 案件の snapshot（`SnapshotProject.auto_integrate`）の欄。読む側があれば同じ変更で直す。
    - 理由 `project_not_enabled`（D4 の読み替えだけを残す）。
    - web の案件の設定の切り替え（リポジトリの設定へ移す。remote・`dir` の行では切り替えを押せなくし、理由を出す）。
    - campaign の preflight の「案件の `auto_integrate` が有効なら止める」は、「案件のリポジトリのどれかが有効なら止める」に置き換える。
    - 案件の欄で取り込みを確かめていた試験は、リポジトリの欄の試験に書き換える。

D9. **変えないもの・非目標。** 旧 ADR の D2（取り込み先は task の repo 行の path・安全条件）、D3（自己リポジトリ以外は
    取り込みで止める・push の条件）、D5（`[selfdeploy] delivery_projects` は未知の欄として読み込みエラー）はそのまま引き継ぐ。
    複数リポジトリの取り込み、remote のリポジトリへの取り込み（Sirius 上での merge）、release・promote の自動化は扱わない。

## Consequences

- BenchFS の案件では `benchfs-paper` だけを有効にでき、論文の root task は review の合格で `main` に取り込まれる。
  `benchfs` の root task は reviewer 必須にならず、受信箱に「取り込みを開始できませんでした」を出さない。
- 判定が 1 つの関数にまとまるので、delivery・tick・reviewer 必須・再取り込み・受信箱の条件がずれない。
- 案件の `auto_integrate` を書く API の呼び出し・config・script は 422 で落ちる。移行の手順は進捗ファイルの「本番への移行」節。
- 有効な案件の remote・`dir` のリポジトリの task は、移行後に reviewer 必須でなくなる（D7）。
