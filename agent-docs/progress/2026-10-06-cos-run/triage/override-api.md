---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
completed: 2026-10-06
---
# CoS 代答の取消・差し戻し API

## 実装

`POST /api/v1/cos/operations/{o}/override` は管理者の bearer だけを受け付け、`action: revoke | return` と空でない人の `reason` を要求する。元の CoS operation が `applied` のときだけ変更し、二度目の操作と人の回答変更後は 409 にする。元の operation ID・人の理由・結果は task の `events` に追記し、既存 event は変更しない。

決定は元 ID を withdrawn にし、新しい ID の open な決定と triage revision を作る。認可は元の回答行を残し、新しい未回答行を作って対象 task を blocked に戻す。元操作が作った standing rule は失効させる。回答で ready になった依存 WU は `blocked(decision)` に戻す。後続 run が判断を消費していれば対象 task と子孫に pause を設定し、実行中の task は `Interrupt` により dispatcher に停止を要求する。元回答は `superseded` になり、旧 ID の回答では再開できない。新しい待ちは source に人の差し戻しの出自を残し、resolve と共通 operation の両経路で CoS の再代答を拒否する。

不可逆な操作は元の効果を消さず `needs_remediation` にし、独立して実行できる補償 task を起票して ID を返す。関連 task が無い操作も補償 task を作る。

## 証拠

- `cargo test -p task-api --test cos_triage_override`：revoke・return・認可・補償 task・409・旧 ID の無効化を検査。
- `cargo clippy --workspace -- -D warnings`：最終確認。
- `git diff --check`：差分の空白確認。

## 未解決

なし。

## 提案

schema 段で `OverrideBody` と返却型、route を公開 API schema と gui/web 型へ登録する。
