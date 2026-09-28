# ADR-0041: 自己改善ループを回す前に直す 5 つ — 作業ツリーの分離・検証の直列化と件数検査・main への反映・昇格前の差分表示・煙試験

- 日付: 2026-09-19
- 状態: **Accepted**（人間の問い「agent-platform から agent-platform 自身の自己改善ループをさせるにあたって問題となる部分は
  ありますか？」への回答を人が承認: 「お願いします」）
- 関連: ADR-0040（自己改善のデプロイ）、ADR-0019（リモートの `sync = "worktree"`）、ADR-0039（案件の作業場所）、
  ADR-0036（成果物の置き場）、SPEC §3.6（危ない操作は人が握る）

## 1. 文脈

ADR-0040 で本番はリリースから動き、昇格は人が握るようになった。しかし自己改善の案件を実際に回すと、次で止まるか、危ない昇格を
人が見抜けない:

1. 案件の作業場所 `local: ~/workspace/agent-platform` を**全タスクが同じ作業ツリーで共有**する。並列の実装者が別のブランチを
   `checkout` すると互いの未コミット変更を壊す。人（と人の補助エージェント）も同じチェックアウトで作業している。
2. `verify.sh` の件数一致検査（ADR-0040 D3 の検査 2）は、スナップショットの後に**本番 API を数える**。ループ中は本番が常に
   動いているので、件数がずれて偽陰性になる。ポートと `~/taskd/staging/` も固定で、**同時実行すると衝突**する。
3. 実装者は `self/<task-id>` にコミットし人はその sha を昇格できるが、**誰も `main` に戻さない**。次のタスクは古い `main` から
   分岐し、本番と `main` が乖離する。
4. 昇格は**新しいリリースに同梱された `promote.sh`** を実行し、GUI は gate / verify の合否しか見せない。実装者が
   `scripts/selfdeploy/` や引き継ぎの機構（`crates/taskd/src/instance.rs`）を変えたリリースを、人がそれと知らずに昇格しうる。
5. verify モードは**ワーカーを一切動かさない**。アダプタ・前置き・委譲・レビューの回帰は verify を通過して本番に届く。

禁止事項が指示文だけで強制力が無い（ワーカーが同じ Unix ユーザー）ことは最大の構造的な穴だが、工事が大きいので**この ADR では
扱わない**（PROGRESS の提案へ。上の 5 つを入れてから判断する）。

## 2. 決定

### D1. ローカルの作業場所も、タスクごとに `git worktree` を切る（Phase 49）

ADR-0019 がリモートに対して決めたことを、ローカルにも同じ形で持ち込む。

- 対象: タスクの（継承した）作業場所が `kind = local` で、その `path` が git リポジトリ（`git -C <path> rev-parse --git-dir` が通る）
  のとき。案件の `workspace` に **`mode: "worktree" | "shared"`** を足し、既定は **`worktree`**（git でなければ `shared` と同じ）。
  `shared` は従来どおり `path` をそのまま作業ディレクトリにする（自分専用の使い捨てリポジトリ向け）。
- 置き場所: **`<workspace_root>/<task_id>/tree`**（taskd が持つ場所。リポジトリの中に置かない。ADR-0019 D3 と同じ理由で
  元のディレクトリには触れない）。成果物は従来どおり `<workspace_root>/<task_id>/artifacts/`（ADR-0036）。
- ブランチ: `git -C <path> worktree add -b <branch_prefix><task_id> <dir> <base>`。`branch_prefix` は ADR-0019 の設定
  （`[workspace] worktree_branch_prefix`、既定 `taskd/`）をローカルにも使う。**ADR-0040 D5 の `self/<task-id>` はこれに置き換える**
  （実装者は「taskd が用意したブランチ」にコミットする。自分で切らない）。
- **`base` の決め方（決定的）**: リポジトリの `main`（無ければ `HEAD`）。ただし本番の `current` リリースの sha が分かり
  （`~/taskd/current/manifest.json`。taskd は `[selfdeploy] releases_dir` の親の `current` を読む）、かつそれが `main` の
  **子孫**（`main` が本番に追いついていない）なら **`current` の sha を base にする**。本番より古いコードから分岐させない。
  どちらを base にしたかは前置き（ADR-0039 D3 の作業場所の節）に書く: 「作業ツリー `<dir>`、ブランチ `<b>`、base `<sha>`（main /
  current）」。
- taskd はコミットしない（ADR-0019 D2）。run の終端で `git status --porcelain` が**空なら worktree を消す**（コミットはリポジトリに
  残る。ブランチも残す）。空でなければ残し、`WorkerFinished` の後に `WorkerProgress`「未コミットの変更が残っています: <dir>」を
  1 行積む。`taskctl workspace prune`（任意）は今回作らない。
- 前置きの指示文（役割ではなく作業場所の節）に「このブランチにコミットせよ。`main` に直接コミットするな。`git checkout` で
  ブランチを変えるな」を入れる。
- 委譲の子は親の作業場所を継ぐ（ADR-0039 D2）ので、**子ごとに別の worktree**になる。親が子の成果を統合するときは子のブランチを
  merge する（LLM の仕事。前置きに子のブランチ名を `context.children` で渡す）。

### D2. `verify.sh` の件数検査は「同じスナップショットのマイグレーション前後」で比べ、実行は直列化する（Phase 50）

- 検査 2 の目的は「マイグレーションで行が消えたり変わったりしていない」こと。本番 API を数える必要は無い。
  **スナップショット（`.backup` 直後、マイグレーション前）を `sqlite3` で数え**、staging API（マイグレーション後）と比べる。
  対象は従来どおり tasks / projects / milestones / org_nodes / approvals / reports / messages の件数と、`tasks` の `(id, status)` の digest。
  `verify.json.counts` は `{snapshot, staging}` の 2 つになる（`prod` / `prod_after` は消す）。
- **直列化**: `verify.sh` は `flock` で `~/taskd/staging/.lock` を取る。待ち時間の上限 `SD_VERIFY_LOCK_WAIT`（既定 1800 秒）。
  取れなければ exit 75（EX_TEMPFAIL）で「他の検証が走っている」と出す。`release.sh` も `~/taskd/releases/.build/.lock-<sha12>`
  で同じ sha の同時ビルドを 1 つにする（`cargo` の target lock で直列化はされるが、`.build/<sha12>` のワークツリー作成が競合する）。
- N-1 互換（検査 5）はそのまま。

### D3. 昇格したリリースは `main` に反映されているかを見せる。base の規則で乖離を止める（Phase 50）

- `promote.sh` は成功時に `<release>/promoted.json` `{promoted_at, mode, from}` を書く。**リポジトリには触らない**
  （人のチェックアウトを fast-forward しない）。
- `GET /releases` の各 item に **`promoted_at`**（`promoted.json`）と **`on_main: bool`**（`git -C <repo> merge-base --is-ancestor
  <sha> main`。`[selfdeploy] repo` で作業チェックアウトの場所を指定。既定 `~/workspace/agent-platform`。読めなければ `null`）を足す。
  GUI: `current` が `on_main = false` なら「本番は main に未反映: `git merge --ff-only <sha12>`」と出す。
- 反映の作業自体は人が行う（`docs/selfdeploy.md` に手順）。D1 の base の規則により、未反映でも次のタスクは本番の sha から分岐する
  ので乖離はしない。

### D4. 昇格の前に「何が変わるか」を見せ、安全に関わる変更は目立たせる。昇格は `current` 側のスクリプトで（Phase 50 / G15）

- `release.sh` はリリースに **`changes.json`** を書く: `{base: <current の sha | null>, commits: [{sha, subject}] (最大 50),
  files: [path...], sensitive: [path...]}`。`base` はビルド時の `~/taskd/current` の sha。無ければ `commits`/`files` は空。
  `sensitive` は次のパターンに当たる `files`: `scripts/selfdeploy/`, `deploy/`, `crates/taskd/src/instance.rs`,
  `crates/taskd/src/releases.rs`, `crates/task-api/src/releases.rs`, `crates/task-core/migrations/`, `CLAUDE.md`, `gui/CLAUDE.md`,
  `.claude/`, `config/`, `docs/adr/0040-`, `docs/adr/0041-`（一覧は `scripts/selfdeploy/lib.sh` の 1 か所に置く）。
- `GET /releases` の item に `changes: {base, commit_count, file_count, sensitive: [...], commits: [...]}` を足す。`base` が今の
  `current` と違えば `changes.stale = true`。
- GUI「リリース」画面: 行を開くとコミット一覧と変更ファイル数。`sensitive` が空でなければ**赤いバッジ「安全に関わる変更 N 件」**と
  そのパス一覧を最初から開いて出す。その場合の「昇格」は `window.confirm` ではなく **sha12 を入力させる確認**にする。
- `POST /releases/{sha12}/promote` は **`current` のリリースの `scripts/promote.sh`** を使う（`current` に `scripts/` が無い
  Phase 48 以前のリリースなら新しい方を使い、応答に `script_from: "current" | "target"` を出す）。新しいコードの昇格スクリプトは、
  それ自身が一度昇格されてから次の昇格で使われる。

### D5. 検証に煙試験を足す: verify モードで**偽のアダプタだけ**を使って 1 件流す（Phase 51）

- `--mode verify` を拡張する（新しいモードは作らない）: verify の taskd は、**`genre = "smoke"` のタスクだけ**を dispatch する。
  `smoke` の役割・分野は verify モードが**組み込みで足す**（設定ファイルにあっても上書きしない。`[[roles]] id = "smoke"
  adapter = "fake"`、`FakeAdapter` の既定コマンド）。それ以外のタスクは従来どおり一切 dispatch しない。通知・報告の圧縮・途中目標
  レビュー・クラスタ・アカウントも従来どおり動かさない。**`daemon_instances` にも書かない**。
- `verify.sh` の検査 6 `smoke`: staging に `POST /tasks {title: "smoke", genre: "smoke", …}`（管理系。staging のトークン）→
  `accept` → 60 秒以内に `done` になり、`WorkerStarted` / `WorkerFinished` の event があり、`GET /tasks/{id}/report`（あれば）が
  返ることを確かめる。これで dispatch → ワーカー起動 → 結果の取り込み → レビュー → 終端 → 報告の生成、までの回帰を検証が拾う。
- 検査 6 も `verify.json.ok` の条件に入れる。N-1 互換（検査 5）では煙試験をしない（旧バイナリが `smoke` を知らないため）。

## 3. 採らない

- ワーカーを別 Unix ユーザー／サンドボックスで動かす（最大の穴だが今回のスコープ外。PROGRESS の提案へ）。
- `promote.sh` が `main` を fast-forward する（人のチェックアウトを機械が動かさない）。
- 自然文の判断で「安全に関わる変更」を判定する。パスのパターンで決める。
- 煙試験で本物の LLM を呼ぶ。`fake` だけ。

## 4. 受け入れ条件

- **Phase 49（D1）**: `mode` の既定 `worktree`、`<workspace_root>/<task_id>/tree` に worktree、ブランチ `taskd/<task_id>`、
  base の規則（main / current。テストは tempdir の git リポジトリで両方）、終端でクリーンなら worktree を消す・汚れていれば残して
  `WorkerProgress`、前置きの文、子は別 worktree、`shared` は従来どおり。`cargo test --workspace` / clippy。
- **Phase 50（D2–D4）**: 検査 2 がスナップショット前後比較になり本番 API を数えない、`flock`（同時 2 本目が待つ／exit 75）、
  `promoted.json` / `changes.json`、`GET /releases` の `promoted_at` / `on_main` / `changes`、GUI のバッジと sha 入力の確認、
  `promote` が `current` のスクリプトを使う。テストは tempdir の git リポジトリと偽のリリースで。GUI 一式。
- **Phase 51（D5）**: verify モードで `smoke` だけ dispatch される（他の ready は動かない）、`verify.sh` の検査 6、`verify.json.ok` に含む。
  実機: `release.sh main` → `verify.sh` で検査 6 が通る。
- **実機（Phase 49–51 の後）**: 自己改善案件の最初のタスクが worktree で動き、`release.sh`/`verify.sh` を自分で通し、GUI の
  「リリース」画面に差分と `on_main` が出て、人が昇格する。

## 5. Phase 83 追記（2026-09-21）: 検査 4b — GUI の e2e（read-only）

D5 の煙試験は「dispatch → ワーカー起動 → 結果の取り込み → レビュー → 終端 → 報告の生成」という celeris 側の
回帰は拾うが、GUI 側の回帰（画面が壊れて 200 は返すが中身が描画されない・コンソールエラーが出る等）は
検査 3/4 の「200 か JSON か」までしか見ていなかった。`gui/CLAUDE.md` の e2e（`pnpm e2e`、`gui/e2e/*.spec.ts`）は
使い捨ての celeris にタスクを作って承認する結合テストで、ADR-0041 D2/D5 が前提にした「staging は検査 6 の
煙試験だけが書き込む」を壊すので、そのままでは verify.sh に組み込めなかった（ADR-0055 D3 が残した既知のギャップ）。

`verify.sh` に**検査 4b（gui-e2e）**を足す。検査 4 が起こした staging の GUI/celeris に対して、
**読み取り専用**（ナビゲーションと `?tab=` の切り替えだけ。`POST` は一切しない）の
`pnpm e2e:staging`（`gui/scripts/e2e-check.mjs`）を走らせる。**検査 6（煙試験）より前**に置く（検査 6 が
足す 1 件が e2e のナビゲーションに写り込まないように）。`verify.json.ok` の条件は「1〜4・4b・6 が全部真」に
広がる（`live_ok`＝検査 5 の意味は変わらない）。

release の `gui/`（`release.sh` の `pnpm install --prod`）には Playwright が devDependency なので入っていない。
検査 4b は `$SD_REPO/gui`（作業チェックアウトの `pnpm install` 済みの方）があればそちらを使い、無ければ
release の `gui/` を試し、どちらにも `@playwright/test` が無ければ `false — not installed` にする
（クラッシュさせない。ADR-0041 D2 の「本番には触れない」は変えない設計判断: Playwright が無い環境でも
verify.sh 自体は最後まで走り切り、原因が分かる形で false になる）。詳細は `docs/selfdeploy.md` の
「検査 4b」節、受け入れ条件は `docs/PROGRESS.md` の Phase 83。

## 6. Phase 89 追記（2026-09-22）: release ゲートに mobile-audit と e2e:mock を足す

検査 4b（上）と D5 の煙試験は**昇格前の staging**を見るので、GUI の見た目の退行（画面は 200 を返すが
壊れている・コンソールエラーが出る等）を拾えるのは verify の段になってからだった。それでは「壊れた
リリースが `~/.local/celeris/releases/<sha12>/` として**作られてしまう**」こと自体は防げない
（D2 の gate は cargo と GUI の型検査・単体テスト・ビルドまでしか見ていない）。

`release.sh` の gate に、`pnpm-build` の直後（cargo 側と GUI 側の折り返し地点）に 2 段足した:

- **`pnpm-mobile-audit`**（`pnpm mobile-audit`。ADR-0055 D1）: 全画面 × light/dark を Playwright Chromium
  で機械監査する。1 件でも違反があれば非 0。
- **`pnpm-e2e-mock`**（`pnpm e2e:mock`。Phase 83 / G36 の読み取り専用 e2e の、外部に何も繋がない
  オフラインモード）。

どちらも直前の `pnpm-build` が作った `$BUILD/gui/build` を使い回す（`MOBILE_AUDIT_SKIP_BUILD=1` /
`E2E_SKIP_BUILD=1`）ので、gate 全体としては `pnpm build` を 1 回しか走らせない。`timeout
"${SD_AUDIT_TIMEOUT:-600}"`（既定 600 秒）で壁時計の上限を掛け、他の段と同じ `run_step` を通るので
lock の fd（8, 9）は継がない（Phase 66c の対策がそのまま効く）。gate.json への記録の形は他の段と同じ
（`{step, exit, secs, log}`）。

Playwright の Chromium 実行ファイルは `pnpm install --frozen-lockfile` では入らず、ホストの
`~/.cache/ms-playwright/` を worktree 間で共有する前提（`pnpm-lock.yaml` が固定なのでバージョンは
ずれない）。**release を作るホストは事前に `pnpm exec playwright install chromium` を 1 度実行して
おく必要がある**。無い場合はブラウザ起動を試みる前の軽い存在チェックで検知し、ハングせずに
`false — playwright browser not installed` として即座に非 0 で終わる（`.gate-pnpm-mobile-audit.log` /
`.gate-pnpm-e2e-mock.log` に理由を残す）。`verify.sh` の検査 4b（`gui/scripts/e2e-check.mjs` の
`pnpm e2e:staging`）はそのまま変更していない。

gate は D2 の記述どおり「7 段」から**9 段**になった。受け入れ条件は `docs/PROGRESS.md` の Phase 89。

## 7. Phase SD-1 追記（2026-09-28）: gui/ に変更の無いリリースは GUI の検査だけの段を飛ばす・verify の 4b と 5 を並行に

release.sh の gate の 11 段のうち、`pnpm-mobile-audit`（実測 98 s）・`pnpm-e2e-mock`（12 s）・`pnpm-test`（3 s）の入力は
**gui/ の下だけ**（偽の celeris も `gui/test/mock-celeris` / `gui/scripts/lib/celeris-fixture.mjs`。`gen:types` は手動でビルドの一部ではない）。
Rust だけを変えたリリースでも毎回 2 分近く回していた。

- **規則**: `current`（gate を全段通り、昇格されたリリース）の sha から、ビルドする sha までの `git diff --name-only <current> <sha> -- gui/`
  が空のときだけ、上の 3 段を**回さずに**gate.json に `{"skipped": true, "reason": "no change under gui/", "exit": 0, "secs": 0}` と書く。
  判断に使った base は gate.json の `gui_skip_base`（sha12。飛ばさなかったら `null`）。
- **飛ばさない**: `current` が無い、`current/manifest.json` の sha をリポジトリが知らない、`git diff` が失敗した、gui/ に 1 ファイルでも
  差がある、`SD_GATE_FORCE_GUI=1`。
- **必ず回す**: Rust の段（fmt / test / clippy / build）、`pnpm-install` / `pnpm-typecheck` / `pnpm-build`（梱包に `gui/build` が要る。
  合わせて 6 s 程度なので、前のリリースの `gui/build` を写して使い回すことはしない〈出所の記録と引き換えにするほど速くならない〉）。
- `changes.json` の base（D4）と同じ `current` を base にする。`current` は gate を通ったリリースしか指さないので、「同じ gui/ で gate が
  通った」ことが保証される（その `current` 自身が段を飛ばしていても、飛ばした根拠の gui/ も同じなので連鎖して成り立つ）。
- `GET /releases` の `gate.steps[]` は `skipped` / `reason` を読まない（exit 0・0 秒の段として出る）。GUI に「飛ばした」と出すのは提案
  （`docs/progress/phase-G.md` SD-1 の P-SD1-3）。

**verify.sh**: 検査ごとの秒数を `checks[].secs` に、`durations`（`lock_wait_s` / `prepare_s` / `parallel_4b_5_s` / `total_s`）を
verify.json に書く。検査 4b（gui-e2e: 7711 / 7701 を読むだけ）と検査 5（N-1: 7712 に旧 celeris を起こして GET だけ）は**並行**に回す。
どちらも同じスナップショットのファイルを開くが、新しい celeris が起きたまま検査 5 を回すのは以前からで、どちらも書き込まない
（D2 / D5: 書き込むのは検査 6 の煙試験だけで、両方が終わってから回す）。旧 celeris は親のシェルで起こして後始末の対象にし、
待ち・集計・判定だけを裏のサブシェルで行う。verify.json の `checks` の順（1, 2, 3, 4, 4b, 5, 6）は変えない。

## 8. Phase SD-2 追記（2026-09-28）: release の gate の `cargo-test` をテストバイナリ並列に

人の判断（2026-09-28「バイナリ並列で動かすようにしてください」、`docs/progress/phase-G.md` SD-1 の P-SD1-2）。
SD-1 の計測で gate の `cargo-test` は 214 s、うち約 200 s が**実行**で、`cargo test` はテストバイナリを 1 本ずつ回していた
（task_core 50 s・task_worker 31 s・task_ops 31 s・tests/scenarios 30 s が直列に足される）。

- **なぜ並列か**: 遅いバイナリの中身は待ち（タイムアウトを待つテスト・子プロセスの kill を待つテスト）で CPU を使っていない。
  バイナリをまたいで重ねれば、全体は「一番遅い 1 本のテスト」（task_core の `pause::tests::truncate_shrinks_…` 49 s）まで縮む。
- **採った方法**: `cargo nextest run --workspace`（(a)。`cargo-nextest` 0.9.146 を `cargo install --locked` で入れ、版は
  `tools/nextest/VERSION`。バイナリはリポジトリに置かない）+ nextest が回さない doc-test を `cargo test --doc --workspace` で別に。
  入口は `scripts/dev/test-parallel.sh`（release.sh の `cargo-test` 段と開発者の両方）。(b)（`--no-run` で実行ファイルを集めて
  `xargs -P`）は採らなかった: 実行ファイルごとの cwd（crate のディレクトリ）と env（`CARGO_BIN_EXE_*` 等）の再現を自前で持つことになり、
  テスト単位の並列・タイムアウト・group 制御も無い。
- **並列数**: `CELERIS_TEST_JOBS`、既定 `min(8, max(2, nproc/3))`（24 CPU → 8）。daemon の run が同居するので nproc いっぱいにしない。
  8・16・24・8 + 負荷（busy loop 6 本）で回し、8 と 24 の差は 13 s（`truncate_…` 1 本が律速）なので 8 で十分。
- **gate が証明すること**: `CELERIS_TEST_SUMMARY {json}` の 1 行を gate.json の `cargo_test` に写す（`runner`・`nextest_version`・`jobs`・
  `binaries` = nextest のバイナリ数 + doc-test の crate 数・`passed` / `failed` / `ignored`・各 exit と秒数・`summary_parsed`）。
  nextest か doc-test が非 0、または nextest の `Starting N tests across M binaries` / `Summary … tests run:` が読めない（全部走った
  証拠が無い）なら段が落ちる。`--no-fail-fast` で全部の失敗を 1 回で出し、nextest が落ちても doc-test は回す。retry はしない（flaky を
  隠さない）。1 テスト 10 分で止める（`slow-timeout`。cargo test には無かった上限）。
- **直列の前提の洗い出しと扱い**（`.config/nextest.toml`）:
  - 固定ポートで bind するテストは無い（全部 `127.0.0.1:0`。7710 / 7700 / 18xxx は文字列として設定の検証に出るだけ）。一時ファイルは
    `tempfile` の一意なディレクトリ。SQLite もテストごとの一時ディレクトリ。→ 何もしない。
  - プロセス全体の状態（`std::env::set_var` の `celeris::instance` のテスト、`waitpid(-1)` の `task-worker::reap_finished_children`、
    `task-worker::process_group` の `static` の Mutex による直列化）: nextest は**テストごとに別プロセス**なので、互いに干渉しない
    （同じバイナリの中の `static` の Mutex は nextest の下では効かないが、守っていたのはそのプロセスの中の状態なので不要になる）。
    `cargo test` では従来どおり Mutex が効く。→ 何もしない。
  - ssh の多重接続（ControlMaster `celeris-localhost`）を共有する `task-worker::ssh_localhost`（9 本）と `e2e::cluster_scenarios`（6 本）:
    cargo test では 2 つのバイナリが重ならなかったが、nextest では重なって sshd の MaxSessions（既定 10）を超えうる。→ test-group
    `ssh-control-master`（max-threads 4）で 2 つを合わせて縛る（多重接続の無いホストではどちらも skip）。
  - `free_port()`（空きポートを選んで閉じ、子の celeris が bind する）の僅かな競合は、並列が増える分だけ起こりやすくなる。7 回の実行
    （手元 6 回 + release 1 回、計 18,207 テスト）では 1 度も起きなかった。起きたら該当テストを test-group に入れる（未対処の残り）。
- **逃げ道**: `SD_GATE_TEST_RUNNER=cargo-test` で従来の直列の `cargo test --workspace`（gate.json の `runner` が `cargo-test`）。
  `cargo-nextest` が無ければ release.sh は作業ツリーを作る前に入れ方を示して落ちる。
- 開発者向けの「`cargo test --workspace`」（CLAUDE.md）は変えない。両方が通ることを保つ。
