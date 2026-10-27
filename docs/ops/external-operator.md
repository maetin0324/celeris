# 外部 operator の手順と CoS との取り決め

---
tasks: [01M5XXF7QQ6KB8MVV0RE96ZKJ8]
---

方針版: `external-operator/1`（2026-10-27）。設計は[ADR](../../agent-docs/adr/2026-10-27-external-operator.md)。
外部 operator は、人が外側で起動する Claude Code 等の運用補助 session を指す。
Celeris の CoS worker や `cos-operator` skill とは別であり、この手順書だけで本番操作の権限は増えない。

## 1. 担当する仕事と起動条件

担当は **配備・rollback・credentiald・本番 config・daemon/DB の復旧**と、
**CoS が人へ回した項目のうち、人が operator に委ねたもの**に限る。
通常の受信箱の回答・再試行・起票は CoS が行う。自己改善の実装は task と reviewer の既存経路を使う。

起動の契機は人の手での依頼、または C3 の通知・escalate（依存表参照）。通知だけでは変更を開始しない。
人の指示が既に有効なら再承認は不要だが、対象・行為・実行者・期限の範囲を開始票で確認する。
release ready は候補の準備完了であり、自動的な昇格許可でも operator の担当指名でもない。

旧 `inbox_watch_6h.sh` の「60 秒ごとに全受信箱を巡回し、既読は再通知せず、release ready は対象外」という運用は廃止する。
既読は解決済みを意味せず、session の既読集合を仕事の正本にしない。未解決状態は API/SQLite を正本とする。
C3 未実装時も旧巡回には戻さず、人の依頼で対象を一度確認する。

特定操作の完了待ちに限り、依頼に指定された期限・間隔で有限の状態確認を行ってよい。
次の場合は巡回・変更の再試行を止め、終了または引き継ぎを記録する。

- 元の待ちが解消済み、目的達成、取消、または人が停止を指示した。
- CoS・他の operator・delivery が既に担当/実行中。同じ仕事を引き取らず既存結果を待つ。
- 期限・予算に達した、認可/認証が足りない、指示が曖昧、外部効果の結果が不明。
- daemon/API に届かず現在状態を検証できない。復旧依頼の範囲で診断し、回答や deploy を推測で続けない。

## 2. 開始票と実行条件

作業前に対象の案件の固定 CoS スレッド（無ければ統括）と関連 task に、次の票を残す。
API が使えなければ秘密を含まない永続の運用記録に保存し、復旧後に同じ記録を関連付ける。
記録先への投稿権限も必要で、未対応の operator 経路は人に転記してもらう。

```text
対象: task / inbox item / source_kind, source_key, source_revision / delivery / release ID
契機: 人の依頼 message ID または通知 ID（通知だけなら調査のみ）
認可: 根拠、対象 host・操作・期限、実行担当一人、引き継ぎ先
実行環境: operator 名、session ID、harness と CLI 版、実際の provider/model ID
方針: external-operator/1、読んだ ADR/skill の版または文書 revision
認証: principal ID、token の用途・scope・期限（値は記録しない）、sudo の許可対象または無し
事前状態: 読み取り時刻、revision、既存 operation ID/state、current/候補/lock
計画: すること、成功条件、停止条件、観測期限、戻す条件と戻す版
```

モデルは起動時に人が選んだ実 ID を記録する。起票元の「Opus 5.5」は過去の session 情報であり、常時の既定値ではない。
model や方針を途中で変えたら変更時刻・理由・版を追記する。別モデルで失敗を隠して再実行しない。

token は認可された用途の資格情報だけを使う。値をコマンド表示・ログ・prompt・チャット・成果物に出さない。
A1 の operator principal が使えない間、管理者 token を借りて回答者を人に見せかけたり、CoS の run credential を借りたりしない。
読み取り用 token で変更を試さない。秘密の追加/更新は人の credential 手順に従う。
sudo は既定で許可なし。必要なら人が対象 host・コマンド・引数・目的・期限を限定して実行する。
包括 sudo、権限探索、db_guard 回避、認証の反復はしない。TOTP は人の GUI 入力を待ち、記録しない。

本番の systemctl・直接の promote/rollback・unit 設置・config 編集・DB 復旧は、
[ADR-0095 D-d](../../agent-docs/adr/0095-worker-runs-see-the-db-read-only.md#d-d-本番-host-の操作は人が実行する手順として書く)により
**人が実行する手順として準備する**。operator 向け task の起票だけでは人限定の制約は解除されない。
DB 復旧も events を書き換え・削除せず、credential を消さず、承認された backup/migration 手順を使う。

## 3. 判断・実行・確認の分担

| 工程 | 判断/担当 | 残す記録 |
|---|---|---|
| 自己改善の成果確認 | reviewer と決定的 check。CoS/operator は二重レビューしない | task の受け入れ結果、検査、review の参照 |
| 配備する版の決定 | CoS が差分・検証・危険箇所を要約し、人が版と実行担当を決める | 指示 message、release ID、current、promotion preview、gate/verify の結果 |
| build・verify | 既存 delivery を優先。手動分は指名された実行者だけ | prepare.log、gate.json、verify.json、終了コード、lock の確認 |
| promote | 人の指示を受けた CoS の `release.promote`、または人の Web 操作の一方。直接 shell は人のみ | operation ID/key/state、202/409、promote.log、promoted.json または失敗記録 |
| rollback・credentiald・config・daemon/DB 復旧 | operator が原因・手順・確認/戻し方を準備し、人が実行 | 認可、変更前後の非秘密の差分、backup、実行者、時刻、結果 |
| 配備後の確認 | 指名された operator が読み取りで確認し、CoS が人へ報告。追加変更/rollback の判断は人 | API と Web の health/release、active/draining、current/previous、対象不具合の確認 |

`202` は起動受付であり配備成功ではない。`status.sh`、`GET /api/v1/releases`、Web の `/healthz` と
[selfdeploy](selfdeploy.md) §4eの確認手順で稼働版を照合する。
不一致や確認不能は「未確認」と記録し、二本目の promote で解決しようとしない。

## 4. CoS との二重対応を防ぐ手順

1. 開始票を共有し、同じ source triple、task、release を扱う既存担当・operation・delivery を確認する。
   担当が曖昧なら人に一本化してもらう。宣言だけでは CoS の自動処理をロックできない。
2. 操作直前に元の待ちと revision を読み直す。既読・escalated の残行だけでは未解決と判断しない。
   解消済みなら何も答えず記録して終了する。CoS が既に同じ起票/再試行をしたらその ID を使う。
3. 人の指示は **B3 の取り次ぎ経路**で出典と対象を保持して伝え、operator の回答は **A1 の principal**で行う。
   自分の推測を人の発言として注入しない。未実装時は人が認証済み Web で入力する。
   既存 CoS の `instructed_by` は当該 run の人の入力 message だけに有効であり、operator が ID を作って流用しない。
4. 既存 CoS は監査経路の `idempotency_key`・`expected_revision`・`reason`・`policy_version` を保持する。
   同じ送信の再送は同じ key。operator と CoS で key を揃えるだけでは主体間の排他にはならない。
5. **409 は正常な競合**として detail と最新状態を読む。解消済み/実行中なら終了または観測へ移る。
   未検証等の前提不足なら止める。すべての 409 を成功とは記録せず、強制や新 key の連打で通さない。
6. 外部効果が pending / needs_remediation、通信切断、timeout なら結果不明。現物と監査を照合し、人に引き継ぐ。
   別経路で回答・再試行・起票・配備し直さない。
7. 終了票に実行者・実行時刻・対象 revision・認可参照・operation・exit/HTTP code・観測した結果・未解決・次の担当を記す。
   session memory や scratchpad のみには残さず、CoS が読める元の task/固定スレッドに関連付ける。

## 5. 同じ release を重ねない

開始時に `GET /api/v1/tasks/<id>/changes` の delivery と `GET /api/v1/releases`、
`scripts/selfdeploy/status.sh`、既存 prepare/promote のログを確認する。パスは本番設定の releases_dir に従う。

| 対象 | 確認する lock と扱い |
|---|---|
| build | `<releases_dir>/.build/.lock-<sha12>` と `<releases_dir>/.lock-release`。前者は同版、後者は全版の共有作業を直列化 |
| verify | `<state_dir>/staging/.lock`。別 verify を起動して並走させない |
| promote | `<releases_dir>/<sha12>/promote.lock` の生存 PID、API の promoting/current、既存 operation と実行ログ |

lock ファイルの存在だけでは flock の保持を判定できない。人の読み取り診断では `lslocks` と PID/コマンドを照合する。
判定不能なら起動しない。実際の排他取得は既存 script/API に任せ、手で lock を削除・移動しない。
release.sh の exit 75 は競合であり再 build の合図ではない。promote の 409 も §4 に従う。
既存候補の gate/verify が成功していれば、それを使う。同版を「念のため」再 build しない。
異なる版の promote も同時に依頼しない。ロックは全主体の判断・指示を一括で直列化する仕組みではない。

## 6. push_error（non-fast-forward）を見たとき

[selfdeploy](selfdeploy.md) §9aに従う。
release の準備・稼働版と remote への push は別に確認する。remote に対象 commit があると証明できれば
復旧証拠を元 task に追記し、過去の push_error が残ることを明記する。表示を消すために再配備しない。

## 7. 依存の状況と本番への移行

A1/B3/C3/D1/D2 は起票元の改善計画の識別子。本 checkout で成立を確認できない部分は**未実装として扱う**。
本書は新 API の仕様や利用可能性を保証しない。

| 依存 | この手順で使う契約 | 現状・使えるまでの扱い |
|---|---|---|
| A1 | 認証された回答主体（operator principal） | 新 principal 経路は未確認・未実装扱い。既存 admin/CoS で代用せず、人が Web で回答 |
| B3 | 人の指示の出典を保持する operator → CoS の取り次ぎ | 新経路は未確認・未実装扱い。人が固定 CoS スレッドへ入力。既存 instructed_by を拡張したとは扱わない |
| C3 | 通知・escalate を契機に起動 | 新しい外部 operator 連携は未確認・未実装扱い。既存 CoS escalation/Discord は稼働機能だが自動 operator 起動ではない。人の手動依頼を使う |
| D1・D2 | 自己改善の成果確認・版決定・実行・事後確認の関連整理 | 詳細仕様/対応実装をこの checkout では確認できない。統合未実装扱い。§3 の既存 reviewer・delivery・人の版決定を使い、新たな自動権限を推定しない |

新経路の利用開始時は、対応 ADR/API、配備済み版、principal と監査、409 と再送の検証証拠を確認し、本表を更新する。
D1/D2 の正式な分担との突き合わせもその条件に含む。
旧巡回台本は本 repo の追跡対象に無い。人が外部 session の巡回停止、常駐/自動起動設定からの除去を確認して移行記録を残す。
本変更では本番台本の停止・本番 DB の修復・配備は実施していない。
