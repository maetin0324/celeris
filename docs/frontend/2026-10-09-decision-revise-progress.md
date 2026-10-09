# 回答済みの決定を web から変更する UI
---
tasks: [01M4F4DC82NXFJSVZM0KB2N77A]
---

実装・検証完了（2026-10-09）。`crates/` の変更、本番操作、デプロイは行っていない。

## 実装

- タスク詳細の概要内「判断 → 決定」に subtree の決定一覧を表示する。回答済みの choice は「答えを変える…」から別の選択肢と理由を入力し、共通確認ダイアログを経て既存の `POST /decisions/{id}/revise` へ送る。自由記述と理由だけの変更も API の契約に合わせて扱う。
- 詳細に現在の回答・回答者・日時・選択肢・回答履歴を表示する。履歴は `GET /tasks/{id}/events?types=decision_answered` をページ取得し、seq 順に表示する。timeline API の先頭 2,000 イベント上限には依存しない。最後の回答が有効で、取り下げ済みの場合は有効と表示しない。
- 応答の `effect`・`notified_children` を使って結果とコメントの通知先リンクを表示する。`resume` でも `resumed` が空なら再開したとは表示しない。応答で一覧キャッシュを更新し、回答イベントの再取得と SSE による更新も行う。
- 受信箱とチャットの決定カードから `/tasks/{taskId}#decision-{id}` へ移動できる。スマホ幅では「判断」区画と対象の決定を開く。
- 未回答・取り下げ済み・choice 以外は操作を表示せず理由を表示する。409・422・権限不足・通信結果不明を区別する。履歴取得失敗は明示し、再取得ボタンを出す。終端タスク等の変更可否は既存 API の判定に従う。
- ADR-0079 の GUI 付記に web の実装状況を追記した。

## 検証結果

| コマンド | 結果 |
| --- | --- |
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0。既存 styles.css の reduced-motion 用 `!important` 警告 4 件のみ |
| `TMPDIR=<短い試験専用ディレクトリ> corepack pnpm@12.6.0 -C web test` | exit 0。Vitest 87 files / 638 tests、gateway 78 tests、失敗 0 |
| `corepack pnpm@12.6.0 -C web e2e e2e/work/decision-revise.spec.ts e2e/chat/cards.spec.ts` | exit 0。12 tests 成功。起動時の Vite build も成功 |
| `corepack pnpm@12.6.0 -C web check:boundaries` | exit 0 |
| `corepack pnpm@12.6.0 -C web mobile-audit --only /tasks/T1` | exit 0。360 / 390 / 412 / 1440 px |
| `git diff --check` | exit 0 |

ブラウザ試験は確認キャンセル時の送信ゼロ、変更成功と POST 本文、回答履歴の更新、通知先リンク、409、取り下げ済み・非 choice の変更不可、履歴取得失敗と再取得、受信箱・回答済みチャットカードからの導線を検証する。変更フォームと確認ダイアログは 4 幅で横溢れゼロ、操作領域 44px 以上、初期フォーカスとキャンセル後のフォーカス復帰、axe serious / critical 違反ゼロを確認した。API は loopback の fixture と browser route mock だけを使用した。

初回の `web test` は Vitest 638 件成功後、既存の browser-live 試験で停止した。run の `TMPDIR` が 93 文字あり、試験の `owner.sock` が Unix socket のパス上限を超えていたため、SIGINT（exit 130）で終了した。`mktemp -d /tmp/cdr.XXXXXX` で作った短い試験専用ディレクトリに小さな fixture・socket だけを置いて再実行すると全件成功した。ディレクトリは終了時に削除し、ビルド出力・リポジトリ・pnpm store は移していない。

ログ・画面確認用の画像は run の中間成果物として次に保存した。

- `/local/celeris/data/workspaces/01M4F4DC82NXFJSVZM0KB2N77A/artifacts/web-{typecheck,lint,test-short-tmp,decision-e2e,boundaries,mobile-audit}.log`
- 同ディレクトリの `web-test.log` は初回停止時のログ。
- 同ディレクトリの `decision-screenshots/decision-{before,confirm,after}-{360,390,412,1440}.png` は変更前・確認中・変更後の 12 枚。
