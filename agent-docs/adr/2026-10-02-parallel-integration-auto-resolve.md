# ADR 2026-10-02-parallel-integration-auto-resolve: 並列取り込みの自動解消と統合の依頼

> 旧名 `docs/adr/0137-parallel-integration-auto-resolve.md`（ADR-0137）から改名（ADR-0128 D5: 0128 を超える番号は新設しない。新しい ADR は日付+slug の名前）。

---
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
---

- 日付: 2026-10-02
- 状態: 採用（実装は後続の葉）
- task: `01M3ZCXNXTRTA9GNJ52Q36ZFCP`
- 関連: ADR-0118（review 前の target 同期）、ADR-0120（IntegrationRepair）、ADR-0128 D3/D5/D6（進捗・ADR の配置と移行）、ADR-0133（受信箱と通知）

## 状況

並列の WorkUnit と task は、個別には正しい変更でも、段の統合や配送時に同じ記録ファイル、番号、生成物に触れて衝突する。現在の `task_dispatch::integration::integrate` は `git merge --no-ff --no-edit` が衝突すると、未統合ファイルを列挙して `merge --abort` し、`IntegrationOutcome.conflict` を返す。`crates/celeris/src/delivery.rs` の `validate_candidate` は承認済みの `base` と `head` を照合し、`merge_reviewed` は `--ff-only` で取り込む。失敗後は `advance` が `RepairClass::MergeBase` の局所修復を `make_repair` で作るが、上限到達時には `[needs-human]` で止まる。定型の衝突にも個別の修復 run が必要になっている。

ADR-0128 D3 は task ごとの進捗ファイルを正とし、D6 は旧 `docs/PROGRESS.md` と `docs/progress/` からの移行期間を定める。その間に走り始めた branch の追記は残る。ADR-0128 D5 は新規 ADR を日付＋slug にするが、移行前に番号を採った branch と、番号付き ADR を要求する既存 task の取り込み衝突には対処が要る。

## 決定

### D1. 決定的に自動解消する範囲と手順

#### D1a. 追記だけの記録

`.gitattributes` の `merge=union` は**凍結済みの `agent-docs/PROGRESS.md` と移行期間の旧 `docs/PROGRESS.md` の 2 行だけ**に設定する（人の方針）。task・WorkUnit ごとの進捗ファイル（`agent-docs/progress/*.md` と入れ子の `agent-docs/progress/<task>/<wu-key>.md`、移行期間の旧 `docs/progress/**/*.md`）には union を設定しない。これらは front matter（`status:`・`updated:` 等）を書き換えるファイルであり、既存行の変更も「末尾への追記」も区別せず行単位で無条件に結合する `merge=union` に任せると、両側が同じ front matter 行を別の値に変えても `git merge` は exit 0 で終わり、同じ key を 2 つ持つ front matter を黙って作ってしまう（records resolver にも `classify` にも衝突が一切見えない）。union を設定しない結果、これらのファイルは git の通常の 3-way merge に委ねる: 両側が base と異なる箇所に触れなければ無衝突で結合され、同じ箇所（front matter の同じ行を含む）を両側が変えれば `U` 状態の実衝突になる。

衝突した記録ファイルは共通の records resolver（`crates/task-dispatch/src/auto_resolve/records.rs`）が判定する。base、ours、theirs の各行を比較し、両側とも base の全行を順序と内容を変えずに保持して末尾へ追記しただけなら、**base → ours の追記 → theirs の追記**の順に結合する。両側で同じ行を追記した場合も記録として各側の追記を保持する。既存行の変更・削除、途中への挿入、判定不能な rename は自動解消せず人に回す。`.gitattributes` で union を設定した 2 ファイルについても、union が生んだ結果がこの条件（base の全行が保持されている）を満たすか records resolver で検証し、満たさなければ resolver の判定を優先する。ただしこの 2 ファイルは追記専用の運用（ADR-0128 D3/D6）で既存行を変えない前提のため、通常は union の結果がそのまま条件を満たす。

ADR-0128 D3 に従い新しい進捗は task・WorkUnit ごとのファイルに書く。この規則は共有ファイルへの新規追記を勧めるものではなく、移行期間の旧 branch の取り込みを救う。ADR-0128 D6 の `git mv` による `agent-docs/PROGRESS.md` への rename は追跡し、旧 path から来た末尾追記だけを移動先へ結合する。`docs/progress/` に旧 branch が追加したファイルは D6 の land 手順で `agent-docs/progress/` へ移す。案内行など移行先で既存行が変わっていれば機械的な「追記だけ」とは扱わず人へ回す。移行期間が終われば旧 path への規則を撤去し、task ごとのファイルを使う。

#### D1b. 番号の衝突

対象は `crates/task-core/migrations/NNNN_*.sql` と `docs/adr/NNNN-*.md`。取り込み時点の `main` と全 `refs/heads/celeris/*` の tree を走査し、種類ごとに最大使用番号を求める。同じ番号が別のファイルを指す衝突では、**main にまだ無い取り込み側のファイルだけ**を、最大使用番号＋1 から空きを確認して `git mv` する。複数件は元 path の辞書順で連番を割り当て、結果を再走査して重複がないことを確かめる。旧ファイル名への参照と旧番号による参照は、取り込み側の変更に属し、そのファイルを指すと一意に判定できるものを追従させる。参照先が曖昧な裸の番号や、main の既存ファイルを指す履歴は機械的に書き換えず人へ回す。番号の変更と参照の変更は同じ取り込み commit に含める。

予約制は長く走る branch が番号を占有し、予約と実際の取り込み順の管理が別に必要になるため採らない。取り込み時の tree と参照を正として振り直す。ADR-0128 D5 の日付＋slug は**新規 ADR の通常の命名規則**として維持し、番号の振り直しは移行前の番号付き branch と番号付き ADR を明示的に要求する既存 task の互換措置に限る。衝突時の取り込み側を日付＋slug にする ADR-0128 D5 と、この互換措置が競合する場合は、この ADR の取り込み時振り直しを適用する。移行完了後は日付＋slug の ADR に番号を新たに割り当てない。migration の連番には引き続きこの規則を使う。

#### D1c. 生成物

`docs/protocol/*.schema.json` と `docs/api/v1/*.schema.json` が衝突したら、いったん target 側の内容を採用する。次に設定で与えた再生成コマンドを取り込み後の tree で実行し、生成物の差分を同じ統合の commit に含める。コマンドを固定文字列として resolver に埋め込まず、試験では一時 git repo の偽コマンドを渡す。コマンドが失敗する、対象外のファイルを書き換える、再生成後も整合検査が落ちる場合は人に回す。生成物の衝突を「target 側採用」だけで完了とはしない。

#### D1d. 取り込み前の main 追従

配送ではレビュー対象 SHA と main の SHA を固定して追従を試みる。main が進んだ場合は新しい SHA を取得して取り込み候補を作り直し、自動解消を適用したうえで gate を再実行する。追従中に main が再び進めば再試行するが、上限は設定で与え、既定は **3 回**とする。上限に達したら人へ回す。各試行の base・ours・theirs と実施した actions を残す。同一入力への再試行は同じ結果とし、無制限の retry や古い承認 SHA のままの配送はしない。

設定ファイルでは従来どおり `[delivery.auto_resolve]` を使う。読み込み時に `[delivery]` を分離し、実行時の設定は `Config.selfdeploy.delivery` に保持して `Config` の公開欄を増やさない。

### D2. 共通の置き場所と呼び出し境界

判定規則は `crates/task-dispatch/src/auto_resolve.rs` に置き、`classify`、`renumber`、`generated`、`records` の submodule に分ける。外側への結果は `Resolution = Resolved { actions } | NeedsHuman { IntegrationRequest }` とする。入力は固定した branch・SHA・merge base・衝突 path・設定であり、分類と推奨は決定論的に行う。`actions` は移動、参照更新、記録結合、生成コマンド実行と commit の対象を記録し、同一入力で再実行しても二重適用しない。判定 module と daemon・store に LLM 呼び出しを入れない。

配送側は `crates/celeris/src/delivery.rs` の `advance` から、`validate_candidate`／`merge_reviewed` の前に main 追従と共通 resolver を呼ぶ。`merge_base` の局所修復に当たる `RepairClass::MergeBase` の `make_repair` を作る前にも定型衝突を判定し、局所修復が回数上限に達したときは `NeedsHuman` の依頼へつなぐ。自動解消で候補 SHA が変われば旧承認をそのまま流用せず、後続の review 判定へ渡す。段の統合側は `crates/task-dispatch/src/integration.rs` の `integrate` で merge 衝突を検出した時、既存の `merge --abort` と `IntegrationOutcome.conflict` への移行前に共通 resolver を呼ぶ。`Resolved` なら解消を commit して残りの WU を順に統合し、`NeedsHuman` なら衝突状態を片付けて依頼を返す。`dispatcher/phase_integration.rs` の既存の gate 実行と結果処理につなぐ。

### D3. 人に回す条件

次のいずれかでは自動解消を止め、D4 の `NeedsHuman { IntegrationRequest }` を返す。分類できない衝突も同じ扱いとし、推測で片側を採らない。

1. **コードの内容衝突**: 同じコードの変更が競合したとき。構文上の結合だけで意味の整合は保証できない。
2. **重複した修正**: 同じ試験または同じ箇所を両 branch が別の方法で直したとき。git が無衝突と判定しても、変更対象の重なりを検出したら人が採用する方法を選ぶ。
3. **自動解消後の gate 失敗**: 解消結果の gate が一つでも失敗したとき。失敗の原因を推定して別の解消を自動で重ねない。失敗した check と log の所在を依頼に添える。
4. **追従の再試行上限**: D1d の main 追従、または `merge_base` の局所修復が設定された上限に達したとき。最後に検証した SHA と試行回数を残し、人が再試行の可否を決める。

記録の既存行変更・削除、曖昧な番号参照、生成コマンドの失敗や対象外変更など D1 の前提を満たさない場合も人に回す。作業 tree は merge を abort して清潔に戻し、候補と証跡は保持する。人の決定を待つ間に自動 retry しない。

### D4. 統合の依頼

`IntegrationRequest` は配送と段の統合に共通の型にし、少なくとも次の欄を持つ。意図の要約は git の事実から組み立てる定型文で、LLM に作らせない。

| 欄 | 内容 |
|---|---|
| `source_branch`, `source_sha`, `target_branch`, `target_sha` | 取り込み側と target 側の branch 名と固定した SHA。再試行で変わったら新しい依頼として扱う |
| `merge_base` | 両 SHA の merge base。複数 base や取得不能ならその事実を記す |
| `conflict_files` | 衝突した path の順序付き一覧。rename は旧名と新名を併記する |
| `intent` | 各側について、その file に `merge_base..SHA` で触れた commit の subject と file ごとの diffstat（追加・削除行数）。commit SHA も添え、path を固定して `git log` / `git diff` から得る。履歴が取れない場合は「取得不可」と明記する |
| `reason`, `recommendation` | D3/D1 の分類と下表から決定的に引く推奨。gate 失敗なら check 名と log の所在、上限なら回数を添える |
| `actions`, `candidate_sha` | 試行済みの自動解消と候補 SHA。未作成なら null。判断後の再開時に同じ変更を二重適用しないために使う |

| 分類 | 推奨 |
|---|---|
| コードの内容衝突、重複した修正 | 両側の意図を比較し、採る実装または統合案を人が指定して修復 run に渡す |
| 記録の既存行変更・削除、曖昧な rename・番号参照 | どの記録・参照を正とするか人が指定してから再統合する |
| 生成物の再生成失敗・対象外変更 | コマンドと対象範囲を人が確認し、修正後に再生成と gate を行う |
| 自動解消後の gate 失敗 | 失敗した check を人が確認し、修復または再試行を選ぶ |
| main 追従・`merge_base` 修復の上限 | 最新 SHA を人が確認し、再試行するか統合を止めるかを選ぶ |

依頼は ADR-0133 の **notice store** を通して保存・配信し、`target_kind = integration_request` と依頼 id を持つ専用種別として受信箱へ出す。ADR-0133 の既存の一般通知一覧には載せず、人の判断待ちの受信箱項目へ一対一で投影する。notice store に表示だけを正として任せず、依頼の未回答・回答済み状態を正として、回答時には受信箱から消す。既存の delivery detail には `[needs-human] 統合の依頼: <分類と対象の短い要約>` を記し、詳細の `IntegrationRequest` と証跡への参照を載せる。依頼の解決後に判断不要となった配送結果は ADR-0133 の `delivery` notice として出せるが、同じ依頼を受信箱と一般通知へ二重掲載しない。

#### D4 付記: 受信箱への投影（実装対応）

統合の依頼の保存・配信は、上記の notice store 案に代えて task の追記専用 `events` を正とする。配送と段の統合は `Event::IntegrationRequested`（type 名 `integration_requested`）を追記し、D4 の `IntegrationRequest` の全欄（`source_branch`、`source_sha`、`target_branch`、`target_sha`、`merge_base`、`conflict_files`、`intent`、`reason`、`recommendation`、`actions`、`candidate_sha`）と、発生元 `origin = delivery | phase:<integrate unit key>` を保存する。daemon と store に LLM 呼び出しは入れない。

依頼 id は `<task>:<target_sha>:<source_sha>` とする。同じ task で同じ `source_sha`・`target_sha` の未回答依頼が既にあれば、新たな `IntegrationRequested` は追記せず、受信箱にも一件だけ出す。回答は `Event::IntegrationAnswered`（type 名 `integration_answered`）に依頼 id、`answer = integrated | declined | retry`、`note` を載せて追記する。回答済みの依頼は未回答の集合から外す。

`task-ops` は未回答依頼を `AttentionItem` に一対一で投影し、`InboxKind::IntegrationRequest`（wire 名 `integration_request`）として `human_inbox` に出す。人の回答は既存の `POST /api/v1/inbox/items/{id}/answer` を通す。依頼の記録時に `notice_record` を呼ばないため、同じ依頼は ADR-0133 の一般通知一覧に載らない。未回答依頼の問い合わせには migration `0047` で `json_extract(json,'$.type') IN ('integration_requested', 'integration_answered')` の部分 index を追加し、`DeliverySkipped` の migration `0037` と同じ形で events 全体の走査を避ける。

段の統合で人の判断が必要になったときは、統合 WU を `blocked(question)`、task を `blocked` にして止める。ただし、この依頼について `record_question_approval` と `QuestionRaised` は作らない。`blocked` の task から汎用の `question` が組み立てられる経路も、同じ task に未回答の `origin = phase:<統合 WU の key>` の依頼がある間は除外する。したがって判断待ちは `integration_request` の 1 項目だけになる。段の記録元は固定の `phase:merge` ではなく、たとえば `phase:integrate-core` のように実際の統合 WU の key を含める。回答側はこの origin で再開対象の WU を特定する。

段の依頼への回答は `IntegrationAnswered` を追記して受信箱の項目を消すとともに、次のように停止を解く。回答の成功時には依頼だけが消えて task が止まったまま、または汎用 `question` が現れる中間状態を残さない。

| 回答 | 段の統合での効果 |
|---|---|
| `integrated` | 人が統合した内容を再検査できるよう、該当の統合 WU の `blocked_reason` を消して `pending` に戻し、既存の質問回答と同じ `Trigger::Answer` で task を再開する。統合の gate は再実行する |
| `retry` | 人の指示を回答に残し、該当の統合 WU を `pending` に戻して `Trigger::Answer` で task を再開し、統合を再試行する |
| `declined` | この統合を見送る判断として、既存の task 停止（cancel）経路へ写す。統合 WU を再開しない |

配送の依頼では、これらの回答は `IntegrationAnswered` による判断の記録と受信箱からの除去だけを行う。回答を契機に配送を自動再試行しない。配送の再試行は既存の `auto_resolve` と人の操作に任せる。

### D5. 再 review と gate

自動解消した merge は種類にかかわらず、解消 commit を含む**候補 SHA を固定して gate を再実行**する。gate 成功の記録は旧 SHA から流用しない。再 review の要否は次の通り。

| 自動処理 | 再 review | 理由 |
|---|---|---|
| 追記だけの進捗記録の結合 | 不要。gate 再実行で足りる | base の行は不変で、両側の末尾追記を順に保存するだけ。変更の意味を選ばない |
| 番号衝突の振り直しと参照追従 | 必要 | migration の適用順と参照先、ADR の相互参照が変わる。機械的な追従が意図どおりか確認する |
| schema.json の再生成 | 必要 | target 側採用後の生成差分が API や protocol の契約を変え得る |
| main 追従のみ（衝突なしを含む） | 必要 | レビュー済み target SHA と候補 SHA が変わり、レビューの前提が失効する |

複数種類が同じ merge に含まれる場合は「必要」を優先する。**配送では**候補 SHA または target SHA がレビュー時から変われば、上表の「不要」行だけでも ADR-0118 の review 前 target 同期に従って再 review する。旧承認・旧 review は新しい候補の承認として扱わない。段の統合では進捗記録だけなら既存の段 gate を再実行して次へ進み、再 review が必要な種類では新候補を review 経路に渡してから次へ進む。gate が失敗したら D3 に従い人へ回し、再 review 成功だけで gate 失敗を相殺しない。

### D6. 試験方針

試験は各ケースごとに**一時 git repo**を作り、base・両 branch・衝突 commit を試験内で組み立てる。外部の git remote には接続せず、外部ネットワークに出ない。生成物の再生成は設定で差し替えた偽コマンドで行い、main 追従の待ちや時刻に依る判定は docs/testing.md の決定的な方法（注入した時計・出来事待ち）で再現する。records の末尾追記・既存行変更、番号の重複と曖昧な参照、生成物の再生成、main 追従の上限、D3 の四条件、依頼の欄と推奨、再実行の冪等性を表形式で確認する。実際の repo や本番 DB を試験入力に使わない。

生成コマンドは設定から渡す**偽コマンド**を一時 repo 内で実行し、成功・失敗・対象外変更を再現する。main が進む競合と retry は、明示した SHA の切替点または同期フックで起こす。時間依存の検査には `docs/testing.md` の決定的な方法（注入した時計、条件待ち、必要時の対象 process だけの SIGSTOP/SIGCONT）を使い、固定 sleep や CPU 負荷で競合を起こさない。daemon・store・共通 resolver に LLM を入れず、意図の要約も git の subject と diffstat のみから作ることを試験で確かめる。

## 結果

定型の衝突は二つの取り込み経路で同じ規則により解消される。解消できないものは衝突中の worktree を残さず、必要な情報を伴う統合の依頼へ渡す。

後続の実装は、(1) `auto_resolve` の分類・records・番号追従・生成物と一時 repo 試験、(2) `integration.rs` からの呼び出しと段 gate、(3) `delivery.rs` の追従・再 review、(4) `IntegrationRequest` の保存・受信箱・delivery detail を分けて進める。各経路は同じ分類表と依頼の型を使う。

## 付記（agent-docs 配置への追従）

2026-10-03。main が ADR-0128 で文書を `agent-docs/` へ移し、D5 で新しい ADR を `agent-docs/adr/YYYY-MM-DD-<slug>.md` と決めたため、`crates/task-dispatch/src/auto_resolve/{classify,renumber,records}.rs` の対象パスと ADR の扱いを次のように改める。D1a・D1b のうち、ここに書いたことはこの付記を優先する。

1. **記録**: Record に分類するのは `agent-docs/progress/` 配下の `*.md`（`<task>/<wu-key>.md` の入れ子を含む）、凍結済みの `agent-docs/PROGRESS.md`、移行期間の旧 branch のための `docs/PROGRESS.md`・`docs/progress/**/*.md`。records resolver はこれら Record 分類すべてに使うが、`.gitattributes` の union 属性は凍結済みの `agent-docs/PROGRESS.md` と旧 `docs/PROGRESS.md` の 2 つだけに限る（下記「付記（union の範囲）」）。コード・生成物（`*.schema.json` など）・`.md` 以外の file には union も records resolver も使わない。
2. **ADR は番号を振り直さない**: `docs/adr/` と `agent-docs/adr/` を一つの名前空間として見る（`scripts/dev/check-adr-numbers.sh` と同じ）。取り込み側にだけある番号付き ADR（target に無い file）が、target の番号付き ADR と番号で重複する、または同じ名前で add/add 衝突するときは、取り込み側の file を `agent-docs/adr/<日付>-<slug>.md` へ `git mv` する（add/add では target 版を残し、取り込み側の内容を日付名で足す）。日付は取り込み側 branch（`merge-base..source`）でその file を足した最初の commit の committer 日付（`git log --diff-filter=A --format=%cs`）、slug は旧名の番号の後ろ。移した file の 1 行目の `ADR-NNNN` は `ADR <新 stem>` に直す。
3. **参照の追従**: 旧 stem（`NNNN-<slug>`）への参照を新 stem へ置換するのは、取り込み側が `merge-base..source` で足した・変えた file に限る。main（target）にある file と main に入った ADR は動かさず、書き換えない。番号だけの参照（`ADR-NNNN`）は参照先を機械的に決められないので、移動を済ませた上で人に回す（D3）。
4. **人に回す ADR の衝突**: 日付名 ADR 同士の衝突（同じ日付名の add/add・内容衝突、移動先の日付名が既にある）はコード衝突と同じく `NeedsHuman`。取り込み側同士の番号重複（どちらも target に無い）と、base から既にある ADR の内容衝突も人に回す。
5. **migration は今のまま**: `crates/task-core/migrations/NNNN_*.sql` は D1b どおり空き番号へ振り直すが、動かすのは取り込み側の未取り込み file だけで、main に入った番号（本番 DB に適用済み）は決して変えない（人の方針）。

試験（`cargo test -p task-dispatch --lib auto_resolve`）: `auto_resolve::records::tests::agent_docs_progress_nested_both_side_appends_are_joined`（agent-docs/progress の入れ子と agent-docs/PROGRESS.md の両側追記の結合）、`auto_resolve::renumber::tests::numbered_adr_duplicate_moves_source_file_to_dated_name`・`adr_add_add_conflict_keeps_target_version_and_adds_source_under_dated_name`・`numbered_adr_follow_only_touches_source_side_files`・`dated_adr_conflict_goes_to_human`（番号付き ADR の重複が日付名へ移る・日付名同士は人へ）、`auto_resolve::renumber::tests::main_migration_numbers_never_move`（main の migration 番号は動かない）、`auto_resolve::tests::path_classes_and_number_duplicates_are_distinct`（分類表）。

## 付記（union の範囲）

2026-10-03。final review の差し戻し: `.gitattributes` が `agent-docs/progress/*.md` と `agent-docs/progress/**/*.md` にも `merge=union` を設定していたため、task ごとの進捗ファイルの front matter（`status:`・`updated:`）を両側が別の値に変えても `git merge` が exit 0 で終わり、同じ key を 2 つ持つ front matter を黙って作っていた。union は行単位で無条件に結合するため、「末尾への追記だけ」と「既存行の書き換え」を区別しない。D1a が約束した「union の結果が base 保持の条件を満たすか検証する」仕組みは、union 自体が git merge を衝突なしで終わらせてしまう経路には届かない（records resolver は `git diff --diff-filter=U` で見つかる衝突にしか呼ばれず、union が無衝突で解決した path はそもそも resolver に渡らない）。

人の方針（「union は追記だけの記録ファイルに限る」）に従い、union の対象を次の 2 行だけにした。

```
agent-docs/PROGRESS.md merge=union
docs/PROGRESS.md merge=union
```

task ごとの進捗ファイル（`agent-docs/progress/*.md` とその入れ子、旧 `docs/progress/**/*.md`）からは union 属性を外した。これらは git の通常の 3-way merge に委ねる: 両側が異なる箇所に触れれば無衝突で結合され、front matter を含む同じ行を両側が変えれば実衝突（`U` 状態）になり、classify が `ConflictKind::Record` として拾って records resolver に渡す。resolver は base の全行が保持された末尾追記だけを自動結合し、既存行の変更（front matter の書き換えを含む）は `NotHandled` として人へ回す（D3・D1a 既存のまま）。このため、同じ記録ファイルの front matter を両側が別の値に変えた場合、git merge の時点で止まるか、たとえ attributes が効かない経路（`git merge-tree` 等）を通っても resolver が base 保持条件で弾いて人へ回す。

確認した試験: `scripts/dev/tests/progress_union_merge.sh`（引数なしで実行。(a) `agent-docs/PROGRESS.md`・`docs/PROGRESS.md` への両側追記は union で無衝突に両節を残す、(b) `agent-docs/progress/<slug>.md` の front matter を両側が別の値に変えると `git merge` が衝突で止まり、`OK: front matter conflict is not silently merged` を出す）と `auto_resolve::tests::front_matter_status_conflict_requests_human_and_is_not_silently_merged`（実リポジトリの `.gitattributes` を一時 repo に写し、front matter の衝突が `Resolution::NeedsHuman` に回ることを確かめる）。

## 付記（2026-10-04、日付名 ADR の誤検出と回答済み依頼の残留）

2026-10-04 の本番（release d9cf53ab、task `01M420EMSFS1VP5RWF2FGCV6XR`）で見つかった 2 つの不具合に対する修正。D1b・D4 付記のうち、ここに書いたことはこの付記を優先する。

1. **日付名 ADR は番号を持たない。** `classify::adr_number` が `agent-docs/adr/2026-10-04-release-notes.md` の先頭 4 桁（年）を ADR 番号 `2026` と読み、同じ年の日付名 ADR 4 本を「同じ番号の別 file」のグループにしていた。取り込み側がその年の日付名 ADR を 1 本でも足す・変えると、グループ全体が `ConflictKind::Adr` として `renumber::resolve_adr` に渡り、日付名は振り直せないので「日付名 ADR の衝突」の `NeedsHuman` になっていた（merge base が target 先端で `git merge` が衝突なしで通る取り込みでも）。`adr_number` は `dated_adr` に当たる path に `None` を返す。日付名同士の衝突の判定（付記 4）は、`git diff --diff-filter=U` に出た実衝突だけが対象になる。merge base が target 先端の取り込みは結果の tree が取り込み側の tree そのものなので、日付名 ADR では依頼が出ない。migration と番号付き ADR の重複は従来どおり走査する（取り込み側が自分の branch で main の番号と重複させていれば振り直す）。
2. **依頼の終了は記録側が追記する。** 未回答の集合から外す `IntegrationAnswered` は受信箱の answer API だけが追記していたため、(a) 人が `POST /tasks/{id}/answer`（汎用の回答）で統合 WU を再開した、(b) 人が手で統合して再実行の統合が衝突なしで通った、(c) 両端の head が動いて同じ統合 WU が新しい依頼を出した、のどの場合も古い依頼が受信箱に残り続けた。`TaskStore` に `integration_request_answer`（id 指定・未回答のときだけ追記）と `integration_requests_close`（`origin` の未回答を全て閉じる）を足し、次の経路が決定的に閉じる。LLM は使わない。

   | 経路 | 閉じる依頼 | `answer` / `note` |
   |---|---|---|
   | `integration_request_record`（新しい組の依頼を追記するとき） | 同じ `origin` の古い組の未回答依頼 | `superseded` / 新しい依頼 id |
   | 段の統合 `on_integration_finished`（merge が衝突なしで通った。検査の成否は問わない） | `origin = phase:<その統合 WU の key>` の未回答依頼 | `integrated` / 統合 WU の key と HEAD |
   | 配送 `merge_reviewed` の成功（main が head を取り込んだ） | `origin = delivery` の未回答依頼 | `integrated` / 既定 branch と head |
   | `task_ops::gate::answer`（汎用の回答で task を再開するとき） | `blocked(question)` の統合 WU の `origin = phase:<key>` の未回答依頼 | `answered` / 回答文 |

   受信箱の answer API（`integrated` / `declined` / `retry`）は `integration_request_answer` を使い、段の依頼では統合 WU がその依頼で止まっている（task `blocked`・WU `blocked(question)`）ときだけ再開・cancel まで行う。既に汎用の回答や replan で再開・完了した WU の依頼には回答の記録だけ行って受信箱から消す（従来は 410 を返して依頼が残り続けた）。閉じる操作は未回答のときだけ追記するので、同じ依頼に回答が重なることはない。受信箱（`task-ops`）は git を持たないので「target が source を祖先に含む」判定を自分では行わず、統合側の記録を正とする。

試験（一時 git repo・一時 DB、外部に出ない）: `auto_resolve::tests::dated_adrs_added_on_both_sides_without_conflict_request_nothing`・`fast_forward_source_with_dated_adrs_requests_nothing`（修正前は本番と同じ 4 本の日付 ADR を理由に `NeedsHuman` を返して落ちることを確認済み）、`store::tests::integration_requests_close_once_and_newer_request_of_same_origin_supersedes`、`inbox::tests::answered_integrated_and_superseded_integration_requests_leave_the_inbox`、`dispatcher::tests::work_units::an_integration_request_leaves_the_inbox_once_the_human_merged_and_answered`（依頼 → 人が worktree で手で merge → 汎用の回答 → 再実行の統合が済んだ merge を飛ばして done）、`a_clean_integration_closes_the_open_request_of_its_origin_as_integrated`、`delivery::tests::integration_repair_fallback_delivers`（main の取り込み後に配送の依頼が消える）、`task-api` `phase_integration_request_of_a_finished_unit_is_answered_without_resuming`（完了済み WU の依頼に受信箱から答えると記録だけされて消える）。

本番の残留 1 件（request id `01M420EMSFS1VP5RWF2FGCV6XR:f865063c…:7fbc8de6…`）は過去の events なので、この修正は遡って閉じない。修正を含む release に昇格した後、人が受信箱から「統合した」と答えれば `IntegrationAnswered` が追記されて消える（統合 WU は既に done なので再開は起きない）。

## 付記（2026-10-04、許可済み・両側既存の ADR 番号重複）

2026-10-04 の本番（release c1b24fb6、task `01M42XH8AAW5RQRRJT43FFP8YP`）で、wu/merge-docs（8ac0cb3e）を古い target（f157bc6e、0078 を持たない）へ取り込む統合が、`agent-docs/adr/0078-browser-execution-capability.md` と `0078-ssh-master-persist-independent-of-daemon.md` を「取り込み側同士の ADR 番号重複でどちらを動かすか決まらない」として統合依頼にした。この 2 本は main で既に並ぶ許可済みの重複（ADR-0128 D5）で、`git merge --no-ff` は衝突なしで通る。原因は classify が「target との差分に 1 本でも入る同番号グループ」を全て重複として拾い、許可リストを知らなかったこと。

1. **許可リストの正本は `scripts/dev/adr-allowed-duplicates.txt` の 1 か所。** 1 行に完全なファイル名 1 つ、`#` から行末は注釈。`scripts/dev/check-adr-numbers.sh` は埋め込みの `ALLOWED_DUPLICATES` をやめてこの file を読み（無ければ exit 2）、resolver（`auto_resolve::classify::allowed_adr_duplicates`）は統合中の作業ツリーの同じ file を読む（無ければ許可なし）。`ALLOWED_OVER_LAST`（0128 超えの許可）は resolver が使わないので script に残す。
2. **新しく生じた重複だけを扱う。** classify は同番号のグループを、(a) 全員が target の tree に既にある（ADR は `docs/adr/` と `agent-docs/adr/` を 1 つの名前空間として名前で見る。旧配置から移しただけの file も既にある扱い）、(b) ADR で全員の名前が許可リストにあり互いに異なる（script と同じ規則）、のどちらかなら拾わない。それ以外は従来どおり振り直し（migration）・日付名への移動（ADR）・人への依頼に回す。
3. **`resolve_adr` は target にある ADR と許可リストの ADR を動かさない。** 許可済みの組に取り込み側が 3 本目を足したときは、その 3 本目だけが日付名へ移る。

試験（一時 git repo、外部に出ない）: `auto_resolve::tests::allowed_adr_duplicate_brought_in_by_source_requests_nothing`（本番の形）・`duplicate_already_on_both_sides_requests_nothing`（修正前は 2 本とも classify が重複として拾って失敗することを確認済み）、`new_duplicates_are_still_renamed_or_requested`、`repository_allowlist_is_the_single_source`。


## 付記（2026-10-04、終端 task に残る統合依頼の回収）

旧版で手動統合・回答を済ませた task `01M420EMSFS1VP5RWF2FGCV6XR` が done に達しても、
`IntegrationAnswered` のない依頼が受信箱に残った。前の付記の手動回答による後処理を、以下の自動回収で補う。

- `SqliteStore::apply_transition_tx` は done・cancelled・failed への遷移と同じトランザクションで、
  origin を問わずその task の未回答依頼に `IntegrationAnswered { answer: "task_terminal", note: None }` を追記する。
  終端化が理由であり、統合成功や人が回答したことを意味しない。既存のイベント定義の `answer` 欄を使う。
  子・後続への中止の伝播も共通の遷移処理を通る。
- `TaskStore::close_integration_requests_of_terminal_tasks` は既存の依頼・回答を畳み込み、現在終端の task の未回答だけを同じ値で閉じる。
  判定から追記まで一つの immediate transaction で行う。過去の events は変更しない。
  回答済み依頼には再追記しないため冪等であり、非終端 task の依頼は残す。
  問い合わせは既存の migration 0047 の部分 index を使う依頼・回答イベント専用の読み取りを再利用する。
- dispatcher の `reconcile_terminal_records` が起動後最初の tick と 600 秒ごとに回収する。
  失敗はログに残し次の周期で再試行する。LLM は呼ばない。新しい DB migration は不要。

試験は store の `terminal_transitions_close_all_open_integration_requests_by_appending_answers` と
`non_terminal_transition_keeps_integration_requests_open`、dispatcher の
`startup_closes_only_terminal_tasks_integration_requests_once`。
終端 3 種、複数 origin、回答済みの保持、過去イベント行の不変、非終端依頼の保持、再起動・定期回収の冪等性を固定する。
本番の取り残しはこの版を含む release の昇格後、最初の tick で閉じる。本 task の worker は本番 DB の書き込みや昇格を行わない。
