# ADR: 破壊的変更を是とし、旧い仕組みを残さない

- Date: 2026-10-11
- Status: Accepted
- 決定者: 人（2026-10-11 の運用セッションでの指示）

## Context

Celeris の利用者と運用者は 1 人で、外部の利用者・他チームの client・公開 API の互換約束は無い。
それにもかかわらず、これまでの変更は互換を恐れて旧い仕組みを残すことが多かった。例:

- provider の同時実行数（2026-10-08 account-concurrency-limits）は「deprecated・読めて warn・無視」として欄・API field・表示を残し、
  provider の役割（LLM source と harness を疎結合に結ぶ層）が曖昧なままになった。
- model routing は legacy の tier・lane と新しい選択が並び、どちらが効いているか分かりにくい。

こうした「新旧の併存」は、読む人・agent が現行の仕様を誤解し、検査の範囲も倍になる。
旧い挙動が必要になったときは git の履歴と release の rollback で救出できるので、併存させて守る必要は無い。

## Decision

D1. **破壊的変更を既定とする。** 仕組みを置き換えるときは、同じ変更の中で旧い仕組みを消す。
    消す対象: コード経路・config の欄・API の field と endpoint・web/gui の表示と入力・文書・旧挙動を固定する試験。
    「deprecated として残す」「互換の分岐を残す」「旧値を読んで警告だけ出す」「新旧を flag で切り替える」は原則しない。

D2. **救出は git で行う。** 良くない変更だったと分かったら、`git revert`・旧 commit からの cherry-pick・
    `scripts/selfdeploy/rollback.sh`（release 単位）で戻す。旧い仕組みを「念のため」残す理由にしない。
    消した仕組みの所在が後から辿れるよう、commit message か ADR に「何を消したか」を書く。

D3. **config の旧い欄は黙って無視しない。** 消した欄が config にあれば、読み込みを失敗させ、
    新しい書き方を示すエラーにする（fail fast）。本番 config の書き換えは deploy 手順（進捗ファイルの
    「本番への移行」節）に書き、運用セッションが deploy と同時に行う。

D4. **データは失わない。** 破壊的変更でも次は守る（これは互換ではなく保全）:
    - SQLite の既存データは migration で新しい形に移す。列・表を消すときも、必要な値は移してから消す。
    - events は追記専用（CLAUDE.md の禁止事項）。古い event の型は読めるまま残す。
    - credential・秘密は消さない・表に出さない。
    - 本番の DB は promote 時の backup（`/local/celeris/state/backups/`）から戻せることを前提にする。

D5. **review の基準。** reviewer（人・agent とも）は、置き換えた旧い仕組みの残り（deprecated の欄、使われない互換分岐、
    旧挙動を守るだけの試験、二重の設定経路）を**不備として指摘する**。「互換のために残した」は合格の理由にならない。

D6. **既存の残骸。** これまでに deprecated として残したものは、その領域を次に触る task で消す。

## Consequences

- 変更は大きくなりうるが、現行の仕様が 1 つに定まり、読む範囲・試験の範囲が減る。
- 本番 config の書き換えが deploy と同時に必要になることが増える。deploy 手順に必ず書く。
- 旧い挙動が要るときは git と release rollback に頼る。release の backup と git の履歴を消さない。
- Claude 以外の agent（codex・opencode・pi など）も同じ方針に従うよう、`CLAUDE.md` と同内容の `AGENTS.md` から本 ADR を参照する。
