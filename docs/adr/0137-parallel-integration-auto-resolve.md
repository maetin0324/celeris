# ADR-0137: 並列取り込みの自動解消と統合の依頼

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

`docs/PROGRESS.md` と `docs/progress/*.md` には `.gitattributes` の `merge=union` を設定する。`git merge-tree` など attributes が効かない経路にも同じ規則を適用するため、共通の records resolver を持つ。base、ours、theirs の各行を比較し、両側とも base の全行を順序と内容を変えずに保持して末尾へ追記しただけなら、**base → ours の追記 → theirs の追記**の順に結合する。両側で同じ行を追記した場合も記録として各側の追記を保持する。既存行の変更・削除、途中への挿入、判定不能な rename は自動解消せず人に回す。`merge=union` の結果もこの条件を満たすか検証し、満たさなければ records resolver の判定を優先する。

ADR-0128 D3 に従い新しい進捗は task・WorkUnit ごとのファイルに書く。この規則は共有ファイルへの新規追記を勧めるものではなく、移行期間の旧 branch の取り込みを救う。ADR-0128 D6 の `git mv` による `agent-docs/PROGRESS.md` への rename は追跡し、旧 path から来た末尾追記だけを移動先へ結合する。`docs/progress/` に旧 branch が追加したファイルは D6 の land 手順で `agent-docs/progress/` へ移す。案内行など移行先で既存行が変わっていれば機械的な「追記だけ」とは扱わず人へ回す。移行期間が終われば旧 path への規則を撤去し、task ごとのファイルを使う。

#### D1b. 番号の衝突

対象は `crates/task-core/migrations/NNNN_*.sql` と `docs/adr/NNNN-*.md`。取り込み時点の `main` と全 `refs/heads/celeris/*` の tree を走査し、種類ごとに最大使用番号を求める。同じ番号が別のファイルを指す衝突では、**main にまだ無い取り込み側のファイルだけ**を、最大使用番号＋1 から空きを確認して `git mv` する。複数件は元 path の辞書順で連番を割り当て、結果を再走査して重複がないことを確かめる。旧ファイル名への参照と旧番号による参照は、取り込み側の変更に属し、そのファイルを指すと一意に判定できるものを追従させる。参照先が曖昧な裸の番号や、main の既存ファイルを指す履歴は機械的に書き換えず人へ回す。番号の変更と参照の変更は同じ取り込み commit に含める。

予約制は長く走る branch が番号を占有し、予約と実際の取り込み順の管理が別に必要になるため採らない。取り込み時の tree と参照を正として振り直す。ADR-0128 D5 の日付＋slug は**新規 ADR の通常の命名規則**として維持し、番号の振り直しは移行前の番号付き branch と番号付き ADR を明示的に要求する既存 task の互換措置に限る。衝突時の取り込み側を日付＋slug にする ADR-0128 D5 と、この互換措置が競合する場合は、この ADR の取り込み時振り直しを適用する。移行完了後は日付＋slug の ADR に番号を新たに割り当てない。migration の連番には引き続きこの規則を使う。

#### D1c. 生成物

`docs/protocol/*.schema.json` と `docs/api/v1/*.schema.json` が衝突したら、いったん target 側の内容を採用する。次に設定で与えた再生成コマンドを取り込み後の tree で実行し、生成物の差分を同じ統合の commit に含める。コマンドを固定文字列として resolver に埋め込まず、試験では一時 git repo の偽コマンドを渡す。コマンドが失敗する、対象外のファイルを書き換える、再生成後も整合検査が落ちる場合は人に回す。生成物の衝突を「target 側採用」だけで完了とはしない。

#### D1d. 取り込み前の main 追従

配送ではレビュー対象 SHA と main の SHA を固定して追従を試みる。main が進んだ場合は新しい SHA を取得して取り込み候補を作り直し、自動解消を適用したうえで gate を再実行する。追従中に main が再び進めば再試行するが、上限は設定で与え、既定は **3 回**とする。上限に達したら人へ回す。各試行の base・ours・theirs と実施した actions を残す。同一入力への再試行は同じ結果とし、無制限の retry や古い承認 SHA のままの配送はしない。

### D2. 共通の置き場所と呼び出し境界

判定規則は `crates/task-dispatch/src/auto_resolve.rs` に置き、`classify`、`renumber`、`generated`、`records` の submodule に分ける。外側への結果は `Resolution = Resolved { actions } | NeedsHuman { IntegrationRequest }` とする。入力は固定した branch・SHA・merge base・衝突 path・設定であり、分類と推奨は決定論的に行う。`actions` は移動、参照更新、記録結合、生成コマンド実行と commit の対象を記録し、同一入力で再実行しても二重適用しない。判定 module と daemon・store に LLM 呼び出しを入れない。

配送側は `crates/celeris/src/delivery.rs` の `advance` から、`validate_candidate`／`merge_reviewed` の前に main 追従と共通 resolver を呼ぶ。`merge_base` の局所修復に当たる `RepairClass::MergeBase` の `make_repair` を作る前にも定型衝突を判定し、局所修復が回数上限に達したときは `NeedsHuman` の依頼へつなぐ。自動解消で候補 SHA が変われば旧承認をそのまま流用せず、後続の review 判定へ渡す。段の統合側は `crates/task-dispatch/src/integration.rs` の `integrate` で merge 衝突を検出した時、既存の `merge --abort` と `IntegrationOutcome.conflict` への移行前に共通 resolver を呼ぶ。`Resolved` なら解消を commit して残りの WU を順に統合し、`NeedsHuman` なら衝突状態を片付けて依頼を返す。`dispatcher/phase_integration.rs` の既存の gate 実行と結果処理につなぐ。

### D3. 人に回す条件

### D4. 統合の依頼

### D5. 再 review と gate

### D6. 試験方針

## 結果

定型の衝突は二つの取り込み経路で同じ規則により解消される。解消できないものは衝突中の worktree を残さず、必要な情報を伴う統合の依頼へ渡す。後続の葉で D3〜D6 の詳細を定める。
