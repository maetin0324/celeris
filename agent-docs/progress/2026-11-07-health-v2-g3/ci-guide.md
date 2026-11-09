---
title: 健全性v2 G3 ci-guide — CI からの導入手順 docs/guides/repo-health-ci.md
tasks: [01M6TYVJSWEY3ZE50S02ETYDNM]
status: done
updated: 2026-11-08
---
# 健全性v2 G3 ci-guide — CI からの導入手順 docs/guides/repo-health-ci.md

完了日: 2026-11-08

WorkUnit `ci-guide`（task 01M6TYVJSWEY3ZE50S02ETYDNM）。[G3 の ADR](../../adr/2026-11-08-health-enforcement-loop.md) の D5.4 と write_set の `^docs/guides/(repo-health-ci|source-health)\.md$` に従う。ci.sh の引数は main に入った実装（`sh scripts/dev/repo-health/ci.sh --help`）に合わせた。基準は `$CELERIS_WU_BASE`。

## 変更
- `docs/guides/repo-health-ci.md`（新規）。書いたこと:
  - 検査器を repo に置く方法: `scripts/dev/repo-health/` を `profiles/` を除いて写し、`scripts/dev/source-size-report.py` も写す。組み込み generic で始める方法、最小の profile、導入の PR と検査器・profile の更新の扱い。
  - CI での呼び方: base の決め方（PR は merge-base、GitLab は `CI_MERGE_REQUEST_DIFF_BASE_SHA`）、履歴の深さ（`fetch-depth: 0`・`GIT_DEPTH: "0"`、shallow のときの exit 2 と deepen の手順）、exit 0/1/2 の意味の表、report の置き場所（`--out` と artifact）。
  - base の検査器を `git worktree add --detach` で取り出して走らせる方法（Celeris の `with_base_checker` と同じく候補の検査器を使わない）。
  - GitHub Actions と GitLab CI の設定例（文書の中の code block。`.github/` などの実 file は作っていない）。
  - Celeris の review・統合と同じ検査器・同じ report の形（report/1、exit 0/1/2）であること、Celeris は CI の結果を証跡にしないこと。
  - 強制できる範囲: Celeris は main への直接 push や CI の外の変更を止められない。必須の check にする branch 保護は repo の持ち主の作業。
- `docs/guides/source-health.md`: 節「入口」に ci.sh の 1 行と導入手順への link を足し、front matter の `tasks` にこの task を足した。

## 文書の内容の確かめ方（一時 repo、`$TMPDIR` の下）
Celeris の main と同じ検査器（`git diff --quiet main HEAD -- scripts/dev/repo-health scripts/dev/source-size-report.py` → exit 0）を、`git archive … | tar -x` で一時 repo に写して実行した。
- 検査器を足す PR を `ci.sh --base main` で検査 → exit 1（`policy.changed` の fail、`policy.source=builtin`）。
- その PR を merge した後、app.py を変えた PR → exit 0。
- 初めて `profiles/default.toml`（`version = "1"` だけ）を足す PR → exit 0、`policy.source=bootstrap`、`inputs.profile.source=repo`。その profile を変える PR → exit 1。
- `source-size-report.py` が無い → `deps.cycle`・`size.responsibility-growth` が `unsupported`（`tool_missing:source-size-report.py`）、verdict error（exit 2）。
- C の file を足す PR → 構造 rule が `unsupported`（`adapter_missing:c`）。profile に `[adapters] ignore_languages = ["c"]` を足して merge した後 → exit 0。
- `git clone --depth 1` の写しで `--base HEAD~1` → exit 2、stderr に `fetch --deepen=` と `fetch --unshallow` の手順。
- 先頭に `exit 0` を足した ci.sh を commit した PR: head の ci.sh → exit 0。`git worktree add --detach <dir> <base>` の ci.sh で同じ head を検査 → exit 1（`policy.changed` の fail）。
- 文書の GitHub Actions の `run:` の中身（`BASE_REF=main`、origin のある clone）→ exit 0、`repo-health-report.json` が checkout の下にできた。GitLab の `script:` の中身を、base に ci.sh の無い commit（導入の PR）で実行 → exit 2、「base … に scripts/dev/repo-health/ci.sh が無い」。
- 未確認: GitHub Actions と GitLab CI の上では実行していない（外部ネットワークに出ない規則）。文書にも「未確認」と書いた。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。最初の check-health.sh は doc-only で verdict pass、`policy.changed` pass、`doc.links`・`human-text`・`progress.index` pass、`fail_grade=0 new=0 worsened=0`。続けて `test-affected: 試験対象の変更なし（docs のみ等）`。
- `bash scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。`sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh scripts/dev/repo-health/ci.sh --base "$CELERIS_WU_BASE" --head HEAD` → exit 0（この branch の commit を検査）。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（Rust は変えていない）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → `docs/guides/repo-health-ci.md`・`docs/guides/source-health.md`・この進捗の 3 件だけ。

## 未解決
- ADR の D5.1 は「policy の file だけの変更は G2 D6.2 と同じ扱いにする」と書くが、main に入った ci.sh は D6.2 の扱いをせず、policy の file だけの変更も exit 1 にする（ci.sh の冒頭注記と進捗 [ci-entry](ci-entry.md)）。この文書は実装に合わせた。ADR の付記は closeout が書く。

## 提案
- closeout で G3 の ADR に付記を足す: D5.1 の「policy の file だけの変更は G2 D6.2 と同じ扱い」を「ci.sh は D6.2 の扱いをせず exit 1 を返す」に改める。
- 検査器の写しの手順（`git archive` と `profiles/` の削除）を、Celeris の release に同梱する `<release>/repo-health/`（ADR D1.4、unit evidence）から写す形にできる。同梱が入った後に、この文書の写し元を release dir にするか決める。
