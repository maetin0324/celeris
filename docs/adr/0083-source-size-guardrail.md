# ADR-0083: source-size-report — 手書き production・inline test・生成物を区別する warning-only guardrail

---
tasks: [01M3Q6F0Y8M0HDMF6Y68G8519M]
---

- Date: 2026-09-30
- Status: Accepted
- 適用先: `scripts/dev/source-size-report.py`、`scripts/dev/source-size-report.toml`、`scripts/tests/test_source_size_report.py`、`scripts/selfdeploy/release.sh`

## 文脈

この案件の一連の Phase（P0〜P2、tests-* 系 WU）は、手書き production を巨大な inline test から分離し、
巨大ファイルを責務別モジュールへ分けてきた（ADR-0079、ADR-0082 ほか）。しかしこれは一度直せば終わりではない。
コードベースが育つ限り、production ファイルは再び肥大化し、inline `#[cfg(test)] mod` は再び production の中に
書き足されうる。この案件で行った分割は、次に同じ問題が起きたときに人やエージェントが気づく仕組みが無ければ
やがて元に戻る。

もう 1 つ、この案件を通じて分かった別種のリスクがある: crate 内モジュール分割では、production ファイルから
`mod foo;` で子モジュールを切り出す。もし `foo.rs` が `.gitignore` のパターンに誤って一致し、かつ
`git add` されないまま作業ツリーに残っていると、その場ではビルドが通ってしまうが、fresh clone やリリースの
`git clean -ffdx` 後の作業ツリー（`scripts/selfdeploy/release.sh` の D2 参照）では compile が壊れる。これは
size の問題ではなく正しさの問題だが、同じ「production source の構造を継続的に見る」ツールに乗せるのが自然である。

P0 の構造監査（`wu/audit/artifacts/audit.md` §9「guardrail の閾値の案」）は、この後続チェックを次の Phase
（このタスクでは "guardrail" work unit）に送っており、閾値の初期案（production 2,000 行・inline test mod
300 行、生成物・schema・`types.rs` は例外）も audit.md にすでに書かれている。本 ADR はその案を実装として固定する。

## 決定

### D1. warning-only、`--strict` だけが exit 非 0

`scripts/dev/source-size-report.py` は既定で **exit 0** を返す。閾値超過やヒューリスティックな警告があっても
リリースを止めない。`--strict` を渡したときだけ、除外されていない warning が 1 件でもあれば exit 1 になる。

理由: 多くの production ファイルが「まだ 2,000 行を超えている」状態は、この案件の `final` work unit が
理由付きで報告する対象であり、機械的な hard fail にすると、cohesive だからと判断して残した既存の大物
（`crates/task-worker/src/paperqa.rs` など）まで毎回ビルドを壊す。閾値は「気づかせる」ためのものであり、
「強制する」ためのものではない（CLAUDE.md の禁止事項にはないが、この案件の目的は behavior-preserving な
構造リファクタリングであって新しい CI gate の導入ではないため、既定は無害側に倒す）。

### D2. 分類: production / inline test / external test / 生成物

`.rs` と `.ts`/`.tsx` を次のように分類する:

- **production**: それ以外のどれにも当たらない手書きソース。
- **inline test**: production ファイルの中の `#[cfg(test)]` で始まる item（`mod { … }` ブロックだけでなく、
  同じ属性が付いた `fn` / `const` / `impl` などの単発 item も含む）。`mod tests;`（外部ファイルへの宣言）は
  0 行としてここには含めない — その宣言先のファイルが実際の中身を持つ。
- **external test**: `tests/`・`benches/` ディレクトリ配下、`tests.rs`、`*_tests.rs`/`*_test.rs`、
  TS 側は `*.test.ts(x)`/`*.spec.ts(x)` と `gui/e2e/`・`web/e2e/`・`gui/test/`・`web/test/` 配下。
- **生成物**: `gui/app/celeris/types.ts`（`pnpm gen:types` の出力）、`**/generated/` 配下、
  `*.schema.json`・`**/schemas?/`（`docs/api/v1/*.schema.json` 等）、`Cargo.lock`/`pnpm-lock.yaml` 等の
  lockfile、`**/fixtures?/`・`testdata/`・`mock-celeris/`、`**/migrations/`（`crates/task-core/migrations/*.sql`）、
  `docs/` および `*.md`。これらは行数を数えて report の `totals` には出すが、閾値チェックの対象にしない。

`web/`（ADR-0081 の新 SPA）はまだこの worktree に存在しないが、上記のパターンはディレクトリ名を
`gui/`/`web/` にハードコードしていない箇所は汎用（`generated/`・`fixtures?/`・test 拡張子）、
ハードコードが要る箇所（test ディレクトリの列挙）は両方を明示している。将来 `web/` が増えても
このファイルの改修は「両方を書く」程度で済む。

### D3. 閾値と例外は 1 つの設定ファイル

`scripts/dev/source-size-report.toml`（TOML、`tomllib` で読む。Python 3.11+ が前提。このリポジトリの CI
（cargo）は Python バージョンを固定していないため、3.11 未満ではこのチェックだけが分かりやすく落ちる）:

```toml
[thresholds]
production_lines = 2000
inline_test_lines = 300

[[exceptions]]
path = "crates/task-api/src/types.rs"
reason = "..."
checks = ["production_size"]
```

`reason` が空の例外はロードの時点で拒否する（`load_config` が `ValueError` を投げる）。理由の無い例外を
足の踏み場にして閾値チェックを空文化させないため。既定の例外は 1 件（`crates/task-api/src/types.rs`、
audit.md §9 の想定どおり）。`checks` を省略すると、その path はどの size チェックからも除外される
（`untracked_gitignored_mod` チェックは対象外 — 例外は「大きくてよい」の意味であって「壊れた mod 参照でよい」
の意味ではないため、`checks` で明示的に指定しない限りこのチェックは除外されない）。

### D4. `mod` が指す untracked かつ gitignore 対象のファイルを警告

Rust 2018+ のモジュール解決規則（`lib.rs`/`main.rs`/`mod.rs` は同じディレクトリ、それ以外の `foo.rs` は
`foo/` 配下）で `mod bar;` の解決先候補（`foo/bar.rs` と `foo/bar/mod.rs`）を求め、それが **ディスク上に
存在し**、**`git ls-files` に無く**（untracked）、かつ **`git check-ignore` に一致する**（gitignore 対象）
の 3 条件がそろったときだけ警告する。単に untracked なだけ（新規ファイルを `git add` し忘れただけの通常の
作業中状態）では警告しない — そこまで警告すると diff の途中を毎回怒ることになり、ノイズで無視される。

### D5. `release.sh` の gate に表示

`scripts/selfdeploy/release.sh` の `cargo-clippy` の直後に `source-size-report` 段を追加し、
`python3 scripts/dev/source-size-report.py`（`--strict` なし）を実行する。既定 exit 0 なので、この段が
gate を落とすのは script 自体が壊れたとき（読み込み不能な config、`tomllib` 無し等）だけである。
出力は他の段と同様 `.gate-source-size-report.log` に残り、`gate.json` の `steps[]` に載る。

## 検証

`scripts/tests/test_source_size_report.py`（`python3 -m unittest scripts.tests.test_source_size_report`、
28 tests、stdlib の `unittest` のみ・ネットワーク不使用・一時 git repo を都度 `git init` して使う）で、
分類・inline test 抽出・`mod` 解決・例外の読み込み拒否・untracked/gitignore 判定・CLI の exit code を検査する。
実リポジトリに対する実行結果（2026-09-30、HEAD 時点）は `docs/progress/phase-guardrail.md` を参照。
