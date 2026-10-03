# ADR 2026-10-04-release-notes: リリースの説明と昇格の要約を決定的に作る

---
tasks: [01M420NG5Y1FX1T6G3B2GDGKRW]
---

- 日付: 2026-10-04
- 状態: 採用
- task: `01M420NG5Y1FX1T6G3B2GDGKRW`
- 関連: ADR-0040 D6（リリース一覧と昇格）、ADR-0041 D3/D4（`promoted.json`・`changes.json`）、ADR-0051（delivery）、ADR-0128（文書の置き場）

## 状況

人の依頼（2026-10-04）: 「リリースごとに何が入ったかの説明がほしい。昇格するときに、いまの本番から昇格先までに入る内容をまとめて見たい」。

既存の `changes.json`（ADR-0041 D4）は `base..sha` の commit 題の生の列で、ほとんどが `integrate wu/…` のため、何の task が入ったのか分からない。リリースは複数たまってから昇格されることが多く（A, B と作って B だけ昇格する、など）、B の `changes.json` は A の分を含まないことも、含むこともある。人が知りたいのは「この昇格で、どの task が、どの DB 移行・設定変更とともに入るか」と「停止が要るか」。

## 決定

### D1. `notes.json` と `notes.md`（リリース 1 件の説明）

`release.sh` は `<release>/notes.json`（機械可読。型は `ReleaseNotes`）と `<release>/notes.md`（人が読む版）を `celerisctl release notes` で書く。内容:

- `tasks[]`: Celeris の task 単位（D2）。題・要約・status（取れたとき）、`source`（`delivery` / `branch`）、属する commit、子 task。
- `direct_commits[]`: どの task にも属さない first-parent の commit。
- `migrations[]` と `schema`（`from` / `to` / `changed`）: `crates/task-core/migrations/` の変更と schema_version の変化。
- `adrs[]`: `agent-docs/adr/`・`docs/adr/` で足された・変わった ADR と、その 1 行目の `# ` 題。
- `config_example`: `config/celeris.example.toml` の変更。足された行（コメント・空行を除く、最大 40 行）と節見出し、`needs_review`（足された行が 1 行以上）。
- `gate_skips[]`: `gate.json` の `steps[]` のうち `skipped: true` の段と理由。
- `first_parent[]`: 範囲の first-parent の sha（新しい順、上限 2000）。D4 の切り出しに使う。

### D2. task への帰属の規則

範囲 `base..sha` の first-parent を新しい方から歩く。

1. 配送記録（`GET /deliveries`）の `head`（`reviewed_sha`・`merge_candidate_sha` も）に当たった commit から、その配送の `base` に着くまでを、その task の区間とする（`source: delivery`）。配送は早送りで main を進めるので、配送した head は main の first-parent に載る。
2. `refs/heads/celeris/<ULID>` の先端に当たった commit は、配送記録が無くても task とする（`source: branch`。配送が優先）。
3. merge commit の題に `celeris/<ULID>` / `celeris-wu/<ULID>/…` があれば、その task とする（`source: branch`）。
4. どれにも当たらない commit は `direct_commits`。first-parent に載らない配送 head・branch 先端（merge の second parent 側）は、その task を 1 件の commit で足す。
5. 題と要約は `GET /tasks/{id}`（題・status・親・最後に `done` で終わった worker run の `outcome_text`、最大 600 文字）。取れない task は commit 題だけで書く。親 task が同じ一覧にいる子 task は親の `children` にまとめる。

### D3. `base` は release.sh を走らせた時点の `current`

`base` はビルド時の `current` の完全な sha。`current` が無い・repo が知らないときは `null` で、`sha` だけの空の説明になる（失敗にはしない）。後で `current` が動いても `notes.json` は書き換えない（不変のリリースの一部）。

### D4. 昇格の要約の集約（`aggregate`）

対象リリース T から始める。

1. T の `notes.base` が `current` なら 1 段で終わり。
2. `current` が T の `first_parent` の中にあれば、そこで切る（`current` より新しい部分だけを採る。task は切り出した範囲に commit を持つものだけ）。
3. どちらでもなければ `base` のリリースの `notes.json` へ進み、1 に戻る（A(base C) ← B(base A) の連鎖）。辿れなければ `complete: false` と `problem`（対象側の notes にある分は出す）。
4. 同じ task は 1 回（commit・子は和集合、親の下に子がいれば単独の子は消す）。migration・ADR は path で重ねる（古い方で `added` ならそのまま `added`）。
5. `schema.from` は `current` の、`to` は T の `schema_version`。`mode` は T の `verify.json` の `live_ok`（真 → `live`、偽 → `stop-start`、無い → `null`）。`gate_skips` は T の分。
6. `releases[]` は辿った段に加え、範囲に sha が入っている中間のリリースも含める（新しい順、T が先頭）。

### D5. API

- `GET /api/v1/releases/{sha12}/promotion-preview` → `ReleasePromotionPreview`。無い sha12 は 404 `release_not_found`。
- `GET /api/v1/deliveries` → `DeliveryList`（`task_id` 昇順。task と commit を結ぶ欄だけ）。
- `GET /api/v1/releases` の `items[]` に `notes` と `promotion`（`current` 自身と notes の無いリリースは `null`）。
- どれも読み取りで、追加のみ（v1 のまま）。スキーマ・DB マイグレーションの変更は無い。

### D6. promote.sh のログと promoted.json

`promote.sh` は昇格の前に `celerisctl release preview <sha12>` をログに出し、`promoted.json` に含まれるリリースと task の id を記録する。人が昇格後に「何を入れたか」を引ける。

### D7. GUI の表示

GUI「リリース」画面は、各リリースに `notes` の task 一覧を、昇格ボタンの近くに `promotion`（含まれるリリース・task・schema の変化・`mode`・config の見直し・gate の飛ばした段）を出す。`complete: false` のときは理由を出す。

### D8. 失敗の扱い、LLM は使わない

notes の生成が失敗しても（git・API・JSON のどれでも）リリースの作成も昇格も止めない。release.sh は警告だけにして先へ進み、`notes.json` が無いリリースは `notes: null` になる。API が届かないときは配送記録を読めなかったとして branch 名だけで判別する（`deliveries_known: false`）。要約はワーカーの `outcome_text` を写すだけで、決定的。LLM もワーカーも呼ばない。

## 結果と限界

- **promote.sh は `current` に同梱のものが走る**（ADR-0041 D4）。ログ出力と `promoted.json` への記録は、この変更を含むリリースが `current` になった後の昇格から効く。API の `promotion-preview` と GUI は daemon 側なので、daemon が新しくなればすぐ効く。
- `deliveries` 表は task ごとに最新の 1 件だけを持つ。同じ task が再度配送されると古い head の対応は失われ、古いリリースの notes は `notes.json` に書いた時点の帰属が正（書き換えない）。
- D4 の切り出しで、first-parent に載らない配送 head（merge の second parent 側）だけを根拠にした task は、切った範囲に commit を持たないものとして落ちる。
- `first_parent` は 2000 件で切る（`truncated`）。それより長い範囲では `current` を見つけられず、`complete: false` になりうる。
- 旧リリース（`notes.json` が無い）は一覧に説明を出せない。作り直さない。
