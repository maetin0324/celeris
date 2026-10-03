---
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
---
# ADR-0128: docs の人向け/agent 向けの分離、task ごとの進捗ファイル、ADR 採番の衝突防止、移行期間

- 日付: 2026-10-02
- 状態: 採用（実装は同じ task の後続 WorkUnit: doc-tools → move-docs → refs-* → cleanup → land-verify）
- 付記: 人の確認 2026-10-02 で改訂。archive を廃止し、DESIGN.md の削除と人向け文書の最小化を決定した。
- 関連: ADR-0079 D7（人の決定）、ADR-0118（review 前の target 同期）、ADR-0120（IntegrationRepair）、
  `docs/testing.md`（決定的な試験、移動前のパス）、分類表 `scripts/dev/docs-layout.tsv`（旧パス → 新パス → 分類）

## 状況

- `docs/` 直下に 29 項目・196 ファイルがあり、人が読む仕様・手順と、agent だけが読む作業記録
  （PROGRESS.md、progress/、adr/、日付付き report、notes）が同じ階層に混ざっている。
- 並列に走る task がみな `docs/PROGRESS.md` の末尾（と冒頭の「現在地」行）を書き換えるため、
  main が進むたびに同じ場所で衝突し、2026-10-02 だけで複数の task が final review の取り込み検査で差し戻された。
- ADR 番号は branch を切った時点で「次の空き」を取るため、同時に走る task が同じ番号を取る。
  全 `celeris/*`・`celeris-wu/*` ブランチと main を走査すると、同じ番号で別名の ADR が
  0064, 0078, 0081〜0083, 0087〜0089, 0094, 0099, 0116, 0117, 0124 にある
  （多くは振り直し前の古い草稿。main の tree で現に重複しているのは 0078 の 2 本）。

## 決定

### D1. 境界と root 名

| root | 読み手 | 置くもの | 置かないもの |
|---|---|---|---|
| `docs/` | 人（と agent） | **現行の**仕様・概要（`SPEC.md`）、運用手順（`docs/ops/`）、API・protocol の説明と schema（`docs/api/`, `docs/protocol/`）、GUI・機能の使い方（`docs/guides/`, `docs/gui/`, `docs/web/` の運用文書）、索引（`docs/architecture-map.md`） | 経緯・進捗・判断過程・日付付きの調査・作業メモ・古い文書 |
| `agent-docs/` | agent（人は必要なときだけ） | `adr/`（設計判断）、`progress/`（task ごとの進捗）、`PROGRESS.md`（凍結した旧進捗）、`reports/`（日付付きの調査・報告）、`notes/`（作業メモ）、`gui/`・`web/`・`ops/`・`guides/`（agent 向けの設計・検証・作業規則）、`GOAL_TEMPLATE.md` | 人が現行の手順として読むべきもの |

- 判定の問い: 「その文書の内容が今のコードと食い違ったら、文書を直すべきか」。直すべき（現行の説明）なら `docs/`、
  その時点の記録として残すべき（経緯）なら `agent-docs/`。
- 機械生成の schema（`docs/api/v1/*.schema.json`, `docs/protocol/*.schema.json`）は現行の契約なので `docs/` に残す
  （生成試験の書き出し先を変えない）。
- 入口として広く参照される `docs/SPEC.md` と `docs/architecture-map.md` はパスを変えない
  （名前を変えても読み手の得が無く、参照の追従だけが増える）。散らばった話題別の文書は `docs/guides/`・`docs/ops/` にまとめる。
- 入口の文書: 人向けは `docs/README.md`、agent 向けは `agent-docs/README.md`（読む順: SPEC → architecture-map →
  関係する ADR → `progress/` の索引）。どちらも move-docs で新設する。CLAUDE.md の「最初に読むもの」は
  `docs/SPEC.md` と `agent-docs/README.md` を指す（refs-repo）。
- 分類と新パスの正本は `scripts/dev/docs-layout.tsv`。分類列は `human|agent|delete`。
  human/agent の新パス列は `docs/` か `agent-docs/` で始め、delete の新パス列は `-` とする。

### D2. 腐った記述の扱い

- 腐った記述（実装と食い違う・廃止機能の手順・重複）は、現状に合わせて直す、経緯として価値があれば
  `agent-docs/` へ移す、価値のないものは削除する。判断に迷うものは削除せず agent 側に寄せる。
- 移動するファイルは `git mv` で履歴を保つ。削除するファイルは `git rm` し、D9 の表に記録する。
- `agent-docs/reports/` の日付付き報告は、今の実装と食い違うなら削除する。

### D2a. 人向け `docs/` の最小化

- `docs/` には、人が現行の仕様・使い方・運用を理解するのに必要十分な文書だけを置く。
  一度きりの作業手順、経緯、重複、agent しか使わない説明は `agent-docs/` へ移すか削除する。
- 同じ話題は 1 つの文書に寄せる。cleanup は現行の実装と照らして古い節を削り、重なる guide や API の説明を整理する。

### D3. 進捗ファイルの規則（PROGRESS.md への追記をやめる）

- 新しい進捗は **task ごとのファイル** `agent-docs/progress/YYYY-MM-DD-<slug>.md` に書く。
  日付は task を始めた日、`<slug>` は phase 名か task の題の短い kebab-case。1 行目の前に front matter:
  ```
  ---
  title: <題>
  tasks: [<task id>]
  status: running | done | blocked | abandoned
  updated: YYYY-MM-DD
  ---
  ```
  本文は従来の PROGRESS 節と同じく「完了日・証拠（コマンドと結果）・未解決・提案」を書く。
  仕様変更の提案もこのファイルの `## 提案` 節に書く。
- **そのファイルは持ち主の task だけが書く。** 同じ段で並列に走る WorkUnit は
  `agent-docs/progress/YYYY-MM-DD-<slug>/<wu-key>.md` に分けて書き、親ファイルは段が直列のときだけ書く。
- 既存の `phase-*.md` などの旧ファイルは名前を変えずに `agent-docs/progress/` へ移し、履歴として残す。新しい追記先にはしない。
- 共有ファイル（CLAUDE.md、architecture-map、AGENTS.md）を変えるときは末尾に足さず、該当する節・並びの位置に 1 行で入れる
  （末尾への追記は並列の branch 同士で必ず隣り合い、衝突する）。

### D4. 索引

- **共有の索引ファイルは commit しない。** 2 つの branch がそれぞれ末尾に 1 行足すだけでも git は衝突として扱う
  （同じ位置への挿入）。生成した索引を commit しても、並列の branch が別々に再生成すれば同じく衝突する。
- 索引はファイル自身（front matter）を正とし、`scripts/dev/progress-index.sh` が都度生成して標準出力に出す:
  - 引数なし: 全進捗ファイルを `updated 降順 | status | title | tasks | path` で表示（`running`・`blocked` を先に）。これが「現在地」。
  - `--check`: `YYYY-MM-DD-*.md` 形式のファイルに front matter（title, tasks, status, updated）が揃い、
    status が上の 4 値のどれかであることを確かめ、違反を `path: 理由` で出して exit 1。旧ファイル（それ以外の名前）は対象外。
- ADR の一覧も生成とし、commit した一覧表は持たない（`ls agent-docs/adr` と各 ADR の 1 行目で足りる）。

### D5. ADR の採番

- **新しい ADR は番号を使わず `agent-docs/adr/YYYY-MM-DD-<slug>.md`** に書く（日付は書いた日）。
  参照は `ADR YYYY-MM-DD-<slug>`（または相対リンク）。名前に task ごとの固有の語が入るので、branch を切った時点で衝突しない。
  `agent-docs/gui/adr/`・`agent-docs/web/adr/` の新しい ADR も同じ形にする。
- 番号付きの最後は本 ADR（0128）。0128 を超える番号の新設は禁止。既に走っている branch が持つ 0128 以下の番号は、
  main で一意である限りそのまま取り込む。
- 既存の重複は **振り直さない**（参照・memory・KB に番号が広く残っており、振り直しの方が壊れる）。
  `check-adr-numbers.sh` に許可リストとして完全なファイル名で書き、参照では番号に slug を添えて区別する
  （例: `ADR-0078（browser-execution-capability）`）。許可リストの初期値:
  `0078-browser-execution-capability.md` / `0078-ssh-master-persist-independent-of-daemon.md`（main）、
  `0116-browser-launcher-implementation.md` / `0116-browser-prod-admission-confidential-release.md`、
  `0124-atomic-direct-route.md` / `0124-claude-session-resume.md`（いずれも走っている branch）。
  これ以外の重複が取り込み時に見つかったら、後から入る側を日付+slug の名前へ振り直し、参照を直す。
- 統合時の検査 `scripts/dev/check-adr-numbers.sh`（D7）を land / delivery の検査に入れ、規則違反を main に入れない。

### D6. 移行期間（旧 PROGRESS.md と走っている branch）

（2026-10-03 改訂、sync-main-3、task 01M40D0QW6HX3XEZK3GBCQV5ZM。旧版の「本文を変えなければ rename 追従で入る」方式を写し直し方式に替えた。経緯は下の「改訂の経緯」。）

一時 repo（git 2.48、merge-ort）で次を確かめた:
(a) `docs/PROGRESS.md` を `git mv` して冒頭に数行足すと、旧 branch が旧パスの末尾に足した節は新しいパスへ衝突なく入る（rename の追従）。
ただしこれは移した側の本文が旧 branch の基点と十分に似ている間だけ成り立つ。
(b) 旧パスに案内だけの別ファイルを置くと rename と見なされず、旧 branch の追記と内容衝突する。
(c) ディレクトリを丸ごと移すと、旧 branch がそのディレクトリに新設したファイルは `CONFLICT (file location)` になる。
旧ディレクトリに 1 ファイルでも残せば衝突せず、新設ファイルは旧ディレクトリに入る。

- 旧 `docs/PROGRESS.md` は **凍結した履歴**として `agent-docs/PROGRESS.md` に置き、旧パスには何も置かない。新しい進捗は task ごとの進捗ファイル（D3）に書く。
- main を取り込むたび（main にまだ `docs/PROGRESS.md` がある間）、`docs/PROGRESS.md` の modify/delete 衝突は
  `sh scripts/dev/progress-transition.sh <main-ref>` で解く。台本は `<main-ref>:docs/PROGRESS.md` の**全文**を `agent-docs/PROGRESS.md` へ写し直し、
  先頭に案内 1 行だけを足して、旧パスを消し、両パスを index に登録する（commit はしない。merge の途中で使う）。案内の行:
  `> **このファイルへの追記は終了（ADR-0128）。** 新しい進捗は agent-docs/progress/YYYY-MM-DD-<slug>.md へ。現在地は sh scripts/dev/progress-index.sh。`
  `<main-ref>` に旧ファイルが無ければ何も変えない。land-verify が確かめる「旧 PROGRESS.md の移行案内」はこの 1 行を指す。
- 写し直したうえで、main が足した節のうち該当 task の進捗ファイルが `agent-docs/progress/` に無いものは、取り込んだ task が D3 の形
  （`YYYY-MM-DD-<slug>.md`、WorkUnit は `YYYY-MM-DD-<slug>/<wu-key>.md`）へ移す。既にあるものには main 側の更新を足す。
  `agent-docs/PROGRESS.md` 自体は写した全文のまま手で書き換えない（次の取り込みでまた全文で置き換わる）。
- この方式は `sh scripts/dev/tests/progress_transition_merge.sh` で確かめる（一時 repo）: main が本文を組み替えて追記した状態で、
  (1) 旧方式の branch が main を取り込むと modify/delete で止まり、台本で解くと未解決が残らず、写しが main の全文と一致し 1 行目が案内になる、
  (2) その解決を main へ fast-forward した後、旧パスの末尾に追記した別 branch X が衝突なく入る、
  (3) 旧方式の解決（基点の本文を移しただけ）のままだと同じ X の取り込みが衝突する、の 3 つ。
- 旧 `docs/progress/` と `docs/adr/` は中身を移したあと、それぞれ `README.md`（「agent-docs/… へ移った。ここに新しいファイルを置かない」）を残す。
  旧 branch・main が新設した進捗・ADR はそこに衝突なく入るので、取り込んだ task が新配置（`agent-docs/progress/` の D3 の形・`agent-docs/adr/`）へ移す。
- 移行期間中は `agent-docs/PROGRESS.md` と `agent-docs/progress/phase-F.md`（旧「以後の追記先」）をリンク検査の対象外にする（D7）。
  写し直しの台本と移行試験も旧パスを名指しするので対象外にする。
- docs の配置を前提にする branch（CLAUDE.md の「最初に読むもの」が旧パスを指す）は、main を取り込めば新しい CLAUDE.md を読む。
  旧パスを読もうとして見つからない agent のために、`docs/README.md` の冒頭に agent 向けの入口（`agent-docs/README.md`）を書く。

移行期間の終わり（人が旧ファイルを消す条件）:
- 本 ADR の取り込み（`agent-docs/PROGRESS.md` が main に入る commit）以後、main に `docs/PROGRESS.md` が無く、
  かつ merge-base がその commit より前の `celeris/*` task branch が無くなったら終わる（`git for-each-ref refs/heads/celeris/` と `git merge-base --is-ancestor` で確かめる）。
- 終わったら人が（または人の確認を得た cleanup の task が）、旧 `docs/progress/README.md`・`docs/adr/README.md` を消し、
  `agent-docs/PROGRESS.md` のリンクを新配置へ直し、リンク検査の対象外（`MIGRATION_EXCLUDE`）を空にし、`progress-transition.sh` と移行試験を消す。

改訂の経緯:
- 旧版の D6 は「`git mv` で移し、移した本文を変えなければ、旧 branch の追記は rename の追従で入る」を前提にしていた（上の (a)）。
- 本 ADR が main に入る前に、main 側で `docs/PROGRESS.md` が組み替えられ（目次化・節の並べ替え・大量の追記）、本 branch の移した本文（基点の版）と
  main の本文が大きく離れた。このため git は rename と見なさず、main の取り込みのたびに `docs/PROGRESS.md` が modify/delete で衝突した。
  さらに、基点の本文を残して解くと main の追記が失われ、後から入る旧 branch の追記もまた衝突する（試験の (3)）。
- そこで「移した本文を守る」のをやめ、取り込みのたびに main の全文で写し直す方式にした。写した後は本文が main と一致するので、
  main へ取り込んだ後の旧 branch の追記は再び rename の追従で入る（試験の (2)）。

### D7. 検査の台本（仕様）

いずれも POSIX sh（dash で動く。bash 専用構文を使わない）。違反は `ファイル:行: 内容` の形で標準出力に出す。

- `scripts/dev/check-doc-links.sh`
  - 呼び方: 引数なし = リポジトリ全体、`<path>...` = その範囲だけ、`--self-test` = `scripts/dev/testdata/doc-links/` の
    fixture（正常例 → exit 0、壊れた例 → exit 1 と期待どおりの出力）を検査。
  - (1) 全追跡 `.md` の Markdown 相対リンク `[..](target)`: `http(s):`・`mailto:`・`#…` だけのものは除き、`#` 以降を落として
    そのファイルの位置から解決し、実在（ファイルかディレクトリ）を確かめる。
  - (2) 生きた参照の素のパス文字列: `CLAUDE.md`, `AGENTS.md`, `crates/**/*.rs`, `scripts/**`, `.claude/**`, `docs/architecture-map.md`,
    `web/**`・`gui/**` のソース（`node_modules`・生成物を除く）に出る `docs/…`・`agent-docs/…` のパスの実在を確かめる。
    `NNNN`・`<…>`・`*`・`{…}`・`$` を含む雛形、`scripts/dev/docs-layout.tsv`（旧パス列を持つ）と `scripts/dev/testdata/` は除外。
  - progress・ADR・report の本文に出る素のパス文字列は履歴なので見ない（Markdown リンクだけ見る）。
    移行期間中の `agent-docs/PROGRESS.md`・`agent-docs/progress/phase-F.md`、写し直しの台本 `scripts/dev/progress-transition.sh` と移行試験は対象外（台本内の 1 変数に列挙）。
  - 壊れた参照が 1 件でもあれば exit 1、無ければ exit 0。
- `scripts/dev/check-adr-numbers.sh`
  - 対象: `agent-docs/adr/`、`agent-docs/gui/adr/`、`agent-docs/web/adr/`、および移行期間中は旧 `docs/adr/`・`docs/gui/adr/`・`docs/web/adr/`
    （存在するものだけ。名前空間ごとに、新旧のディレクトリを合わせて 1 つとして数える）。`README.md` は除く。
  - 規則: ファイル名は `NNNN-<slug>.md`（gui/web は既存の `NNNN-`・`web-NNNN-` も可）か `YYYY-MM-DD-<slug>.md`。
    同じ番号が 2 本以上 → 許可リスト（D5）にあるファイル名の組でなければ違反。main の名前空間で 0128 を超える番号 → 違反
    （「新しい ADR は YYYY-MM-DD-<slug>.md で書く」と出す）。日付形式で slug が重複 → 違反。形式に合わない名前 → 違反。
  - `--refs`: この branch で足した ADR（main との merge-base からの追加分）と、`refs/heads/main`・`refs/heads/celeris/*`・
    `refs/heads/celeris-wu/*` の tree にある ADR とで番号・日付+slug が重なるものを出して exit 1（取り込み前の確認用）。
- `scripts/dev/check-doc-layout.sh <tsv>`（move-docs）: human/agent の各行は、旧≠新なら旧が無く新が追跡されていること、
  旧=新なら存在することを確かめる。delete の行は旧が無く、進捗ファイルの削除表に旧パス・理由・最後の commit があることを確かめる。
- `scripts/dev/progress-index.sh [--check]`（D4）。
- land-verify と以後の land 系の check は `check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check` を必ず含める。
  WU の check の雛形（task-worker の prompt）には、ADR と進捗の新しい置き場所と、この 3 本を書く（refs-worker）。

### D8. docs/DESIGN.md（人の決定 `design-md`）

人の確認（2026-10-02）により `docs/DESIGN.md` は削除する。本文を変更して別文書に統合しない。
CLAUDE.md の「最初に読むもの」にある参照は `docs/SPEC.md` へ変える。削除は move-docs、参照変更は refs-repo で行う。

### D9. 削除の記録

削除した各ファイルの **旧パス・削除理由・削除前の最後の commit** を表に残す。
本 task では `agent-docs/progress/2026-10-02-docs-layout.md` を親の記録とし、並列 WorkUnit は
`agent-docs/progress/2026-10-02-docs-layout/<wu-key>.md` に自分の削除分を書く。
最後の commit は削除前に `git log -1 --format=%H -- <旧パス>` で確かめる。
親ファイルは WorkUnit の統合後に記録を集める。これにより Git から内容を取り出せる。

## 結果

- 並列 task は自分の進捗ファイルと自分の ADR（日付+slug）だけを足すので、記録同士が衝突しない。
- 走っている branch は、旧 PROGRESS.md の末尾への追記・旧ディレクトリへの新設のどちらでも、移動後の main と衝突せずに取り込める。
- 「現在地」は commit された 1 行ではなく生成になる。人が GitHub 上で一覧を見たいときは `progress-index.sh` の出力を使う。
- 番号での短い呼び名（ADR-0xxx）は 0128 で止まる。以後は日付+slug で呼ぶ。

## 付記（2026-10-03、sync-main-2 migrate-docs、task 01M3Z8CXYG1J6BZCQ87YS3FC67）: 取り込み前に main にあった 0128 超えの番号

本 ADR が main に入る前に、main には 0129・0132〜0135・0138 の番号付き ADR が旧 `docs/adr/` に足されていた（本 ADR の D5 より前に書かれたもの）。
D5 の「既存の重複は振り直さない」と同じ理由（参照・memory・KB・コードのコメントに番号が広く残っている）で、これらも振り直さない。

- D6 に従い `agent-docs/adr/` へ同じファイル名で `git mv` し、`check-adr-numbers.sh` の `ALLOWED_OVER_LAST` に完全なファイル名で書く。
  許可するのは「本 ADR の取り込み時に main に既にあった」ものだけ。
- 以後に別 branch から入る 0128 超えの番号（例: 0137）は D5 どおり新設禁止の違反として扱い、
  land の task が日付+slug へ振り直すか、main に先にあったことを示して許可リストに足すかを決める（後者は人の確認を取る）。
- 追記（2026-10-03、sync-main-3）: `0136-local-hot-data-layout.md`（main a2124b5f）と `0139-langmem-proxy-bearer-and-verify-proxy-bind.md`（main 9fb850c6）は
  本 ADR の取り込み前に main に入っており、crates のコード・コメントが番号で参照しているので、同じ理由で許可リストに足した。
