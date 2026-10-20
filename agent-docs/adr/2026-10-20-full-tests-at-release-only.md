# 全体の試験は release gate だけで流す — 途中の検査は変更 crate と逆依存 crate に絞る

- 日付: 2026-10-20
- 状態: 採用
- 決定者: 人（2026-10-20 の運用セッションでの指示。task 01M5BZY0AM）
- 関連: ADR 2026-10-11（破壊的変更を是とする）、ADR-0041 §8（release gate の並列試験）、ADR-0125（時間依存待ちの決定化）、
  ADR-0126（userns 試験は opt-in）、ADR 2026-10-16（fmt check）、`agent-docs/guides/testing.md`

## 背景

これまで CLAUDE.md・worker / planner の前置き・repo の検査コマンドは、WU・段の統合・task の受け入れ・review・
各 Phase の完了のたびに workspace 全体の試験（`bash scripts/dev/test-parallel.sh` = `cargo test --workspace` と同じ範囲）を求めていた。
web の変更では `pnpm -C web e2e`、browser の変更では本物の Chrome / userns を使う試験も途中で流すことがあった。

並列の WU が増え、同じ host で全体の試験が何本も同時に走るようになった結果:

- host の load average が 50 前後に達し、本物の Chrome を起動する試験（CDP の応答待ち・launcher の起動待ち）が
  2026-10-19 と 2026-10-20 に時間切れで落ちた。変更と無関係な失敗で WU と段の統合が止まり、repair と replan が無駄に回った。
- 1 本の WU が触るのは 1〜2 crate なのに、毎回 全 crate を build・試験しており、待ち時間の大半がこれだった。
- 時間依存の失敗は負荷を掛けても決定的には再現できない（`agent-docs/guides/testing.md`）。負荷の下で全体を何度も流しても
  品質の保証は増えず、偽の失敗だけが増える。

全体の試験・本物の browser / userns 試験・web の e2e が意味を持つのは、出荷する sha を決める release gate
（`scripts/selfdeploy/release.sh`）だけである。release gate は 1 本ずつ走り、userns も使える（`CELERIS_USERNS_TESTS=1`）。

## 決定

D1. **全体の試験は release gate だけ。** 次は `scripts/selfdeploy/release.sh` の gate でだけ流す:
    - `bash scripts/dev/test-parallel.sh`（workspace 全体の nextest。`cargo test --workspace` を含む）
    - web の e2e（`pnpm -C web e2e` / `e2e:nfr` / `e2e:all`）
    - 本物の browser / runtime / launcher・userns を要する試験（`CELERIS_USERNS_TESTS=1` / `CELERIS_ISOLATION_TESTS=require`）

D2. **途中の検査は影響範囲に絞る。** WU・段の統合・task の受け入れ・review・Phase の完了で求める検査は次だけ:
    - Rust（`crates/` か workspace 共通 file）の変更: `bash scripts/dev/test-affected.sh`
      （変更 crate と、その逆依存 crate の閉包を `cargo nextest run -p …` と `cargo test --doc -p …` で流す）、
      `cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`
    - `web/` だけの変更: web の unit 試験（`web/package.json` の `test` script = vitest と `node --test`。e2e は流さない）。
      `test-affected.sh` が自動でこれを選ぶ
    - docs だけの変更: 試験は流さない（`test-affected.sh` は何もせず exit 0）。文書の検査（`check-adr-numbers.sh`・
      `check-doc-links.sh` など）は従来どおり

D3. **`scripts/dev/test-affected.sh` の契約。**
    - 基準: `${CELERIS_WU_BASE:-$(git merge-base HEAD main)}`。変更 path は `git diff --name-only <基準>` と未追跡 file から求める。
    - 変更 crate は path から決め、逆依存は `cargo metadata --format-version 1` の workspace 内依存（dev-dependencies を含む）の閉包で求める。
      jq は使わない（host に無い）。
    - workspace 共通 file（`Cargo.lock`、ルートの `Cargo.toml`、`.config/nextest.toml`、`rust-toolchain*`）の変更は全 crate 扱いにし、警告を出す。
    - `--list` は試験を流さず、対象 crate 名を 1 行ずつ出す（check と単体試験はこれで範囲を確かめる）。
    - 重い負荷は掛けない（並列度は nextest の既定と `.config/nextest.toml` に任せる）。

D4. **release gate は変えない。** gate の段（fmt → test-parallel → clippy → web build・unit・e2e → browser ledger 等）は現状のまま。
    途中で全体を流さなくなる分、範囲外の退行は release gate で初めて見つかることがある。そのときは release を出さず、
    原因の crate を直す task を立てる（gate が最後の砦であることを前提にする）。

D5. **旧い文言は残さない（ADR 2026-10-11 D1・D5）。** 「各 Phase / WU / 受け入れで `test-parallel.sh`（`cargo test --workspace`）を流す」
    という記述は、置き換える同じ一連の変更で消す。「念のため全体も流す」「任意で全体を流してよい」といった併記や、
    旧い検査コマンドを残したまま新しいものを足すことはしない。review は旧い文言・旧い検査の残りを不備として指摘する。
    戻したくなったら git revert で本 ADR と置き換えの commit を戻す（ADR 2026-10-11 D2）。

## 置き換える文書・箇所

本 ADR（leaf `adr`）は決定だけを書く。下の書き換えは**本 leaf の対象外**で、後続の task（同じ task 木の後続 leaf / 子 task）が行う。
`test-affected.sh` 本体と単体試験は同じ工程の leaf `script` が作る。

| 箇所 | 今の記述 | 置き換え |
| --- | --- | --- |
| `CLAUDE.md`（= `AGENTS.md`）「各Phase完了時に必ず」 | `bash scripts/dev/test-parallel.sh` を必須 | `bash scripts/dev/test-affected.sh` + clippy + fmt。全体は release gate だけと書く |
| `crates/task-worker/src/claude_code/prompt.rs`（planner の check 指針） | Rust を変える unit は `test-parallel.sh` と clippy の隣に fmt を置く、web/docs だけなら `cargo test --workspace` を外す | Rust の unit は `test-affected.sh`・clippy・fmt。web だけは web unit。e2e・全体・userns は release gate |
| `crates/task-worker/src/preamble.rs`（task 作成の指針） | web/docs だけの task は `cargo test --workspace` を必須にしない | 受け入れでは全体の試験を求めない（`test-affected.sh` を使う）に一本化 |
| repo の検査コマンド（project の repo 設定。前置きの「このリポジトリの検査コマンド」） | `bash scripts/dev/test-parallel.sh` / clippy / fmt | `bash scripts/dev/test-affected.sh` / clippy / fmt。本番の設定変更は人の手順として進捗ファイルの「本番への移行」節に書く |
| daemon の workspace / 段の統合 check（`crates/task-core/src/workspace_config.rs` の例、`docs/guides/workspace.md` の例） | `check = ["cargo test --workspace", …]` | `bash scripts/dev/test-affected.sh` と clippy |
| `agent-docs/guides/testing.md` §web e2e | nfr と `e2e:all` は visual-qa・最終の受け入れ・release gate の段で流す | e2e（functional を含む）は release gate だけ |
| `agent-docs/guides/work-unit-checks.md` | WU の check の例 | `test-affected.sh --list` / 実行を例に |
| `docs/ops/nextest.md`・`docs/ops/selfdeploy.md` | test-parallel.sh を手元で流す案内 | release gate 専用と明記し、手元・WU は `test-affected.sh` を案内 |
| 試験（`crates/task-ops/src/repo_requirement.rs`・`crates/task-core/src/route_record/tests.rs` などの文字列 fixture） | `test-parallel.sh` を検査コマンドの例に使う | 旧い要求を固定する試験なら新しいコマンドへ直す（単なる文字列の例は対象外） |

後続 task は、上の表を `grep -rn 'test-parallel\|cargo test --workspace' CLAUDE.md agent-docs/guides docs crates/task-worker/src crates/task-core/src`
で取りこぼしなく確かめる。`agent-docs/progress/`・過去の ADR の記述は経緯の記録なので書き換えない。

## 結果

- WU と段の統合の待ち時間が短くなり、負荷由来の偽の失敗（本物 Chrome の時間切れ）が途中の段から消える。
- 範囲外の退行の発見が release gate まで遅れうる。逆依存の閉包で直接の影響は拾えるので、残るのは workspace 外の
  経路（script・web と crate の境界など）だけで、これは gate が受け持つ。
- 「全体を流した」という報告は release gate の結果（`checks[]`）でだけ示す。worker の報告は `test-affected.sh` の対象 crate と結果を書く。
