# ADR 2026-10-21-cargo-lock-regenerate-on-integrate: 統合の自動解決で Cargo.lock の衝突を再生成で解く

---
tasks: [01M5GE1BGEJDRK1AQ1Y1TK85X0]
---

- 日付: 2026-10-21
- 状態: 採用・実装済み
- 関連: ADR 2026-10-02-parallel-integration-auto-resolve（D1c 生成物の再生成）、ADR-0118、ADR-0120、ADR 2026-10-11-breaking-changes-by-default

## 状況

2026-10-21 の moshi-brewed（task 01M5G016QF 配下）で、並列 WU・子 task がそれぞれ crate と依存を足し、`Cargo.lock` だけ（または `Cargo.lock`＋両側追加のみ）の衝突が integration_request として 3 回人に回った（01M5G5YPCV、01M5G8TZS9、01M5G9TYCW）。いずれも「target 側の `Cargo.lock` を採って `cargo metadata --format-version 1` で再生成」で機械的に解けた。answer しても同じ source/target sha で統合を繰り返すだけで進まなかった。

現状の D1c は `crates/task-dispatch/src/auto_resolve/generated.rs` の固定 `DEFAULT_GLOBS`（docs/protocol・docs/api の schema）と、delivery の設定 `[delivery.auto_resolve.generated]` の 1 本の command だけを使う。task 内の統合（WU の段の統合・子 task の親への統合、`crates/task-dispatch/src/integration.rs`）は `generated_command: None` を渡しており、生成物の再生成が一切効かない。

## 決定

### D1. 規則の集合として渡す（`generated_command` を消す）

`ResolveContext.generated_command: Option<Vec<String>>` を消し、`ResolveContext.generated: Vec<GeneratedRule>` に置き換える。`generated::resolve` は衝突 path に一致する最初の規則で再生成する。どの規則にも一致しなければ従来どおり人に回す。`GeneratedRule::with_default_globs` と「`generated_command` を `DEFAULT_GLOBS` として解釈する」経路は消す。

### D2. Cargo.lock は repo の種類から自動で効かせる（設定不要）

`Cargo.lock` の再生成は内蔵規則とし、設定で宣言しない。理由: 再生成手順は Cargo が決める一定のもので（target 側を採って `cargo metadata` で lock を合わせる）、project ごとに変わる余地がない。設定にすると task 内の統合（project 設定を引かない経路）で抜けやすく、今回の事故の再発になる。

- 対象: repo 内の任意の階層の `Cargo.lock`（glob `Cargo.lock` と `**/Cargo.lock`）。
- 有効条件: 同じディレクトリに `Cargo.toml` があること（Cargo workspace の根）。無ければ人に回す。
- 手順: 衝突 path を target（HEAD・ours）側に戻し、その `Cargo.lock` のあるディレクトリで `cargo metadata --format-version 1` を argv で実行する（shell を介さない）。stdout は捨てる（大きいので読み切って破棄）。
- timeout: 既存の `DEFAULT_TIMEOUT_SECS`（600 秒）。超えたら子 process を kill して人に回す。
- `CARGO_TARGET_DIR`: 既存規則どおり作業場所の外。`cargo metadata` は build しないが、呼び出し側の環境の `CARGO_TARGET_DIR` を継ぎ、無ければ作業場所の外の一時 dir を渡す。
- 再生成後、作業ツリーで `Cargo.lock` 以外が変わっていれば（既存の「対象外の変更」判定）人に回す。

### D3. schema の規則は delivery 設定のまま

`[delivery.auto_resolve.generated]` の command は `DEFAULT_GLOBS`（schema）に対する規則として delivery が組む（呼び出し側で `GeneratedRule` を作って `generated` に足す）。task 内の統合は schema 規則を持たない（再生成 command が project 設定に無いため）。Cargo.lock 規則は 3 経路すべてで常に入る。

### D4. 3 経路で同じ規則

WU の段の統合と子 task の親への統合（`task_dispatch::integration`）、delivery（`celeris::delivery::auto_resolve::merge_in_scratch`）のすべてで `generated` に Cargo.lock 規則を入れる。`generated_command: None` の経路は残さない。

### D5. 確定は既存の統合検査の後

再生成した解決は merge commit に含め、確定（task branch 前進・delivery の merge）は既存の統合検査（段の checks・review・delivery の検証）を通ってから。`Cargo.toml` など他の path にも衝突があれば、その path は既存の resolver が扱えず `NeedsHuman` になり、merge 全体を abort して人に回す（3-way の union はしない）。

## 結果

- 両側が別の依存を足しただけの `Cargo.lock` 衝突は人に回らない。
- 本番の config の書き換えは不要（D2 は内蔵、D3 は既存の設定欄のまま）。

## 付記（実装突き合わせ, 2026-10-22）

- D1: `task_dispatch::auto_resolve::ResolveContext.generated: Vec<GeneratedRule>`。`GeneratedRule::with_default_globs` と `generated_command` は削除。
- D2: `generated::cargo_lock_rule()`（globs `Cargo.lock`・`**/Cargo.lock`、`cargo_workspace: true` で衝突 path のディレクトリで実行し `Cargo.toml` が無ければ人へ）。`classify::kind` は `Cargo.lock` を `Generated` にする。`CARGO_TARGET_DIR` が環境に無ければ `std::env::temp_dir()/celeris-auto-resolve-target`。
- D3/D4: `generated::rules(schema_cmd)`。`task_dispatch::integration::integrate` は `rules(None)`、`celeris::delivery::auto_resolve::merge_in_scratch` は `rules(settings.generated.command())`。
- D5（delivery）: `review_free` は Record に加えて再生成した `Cargo.lock` だけの解消を merge commit にして新しい候補として積み直す（state `MergeQueued` → 次の advance で `Reviewing`）。schema の再生成は従来どおり `NeedsReview` の fallback。
