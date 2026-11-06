# 誤配置した report の回収を result.json と review.json で 1 つの規則にする（ADR 2026-10-18-result-location-recovery / -scope の拡張）

- 日付: 2026-11-06
- 状態: 実装済み（WU generalize: 回収の一般化。WU wire-review: review.json への配線と reviewer prompt）
- tasks: [01M6S325NYYJS9YWNHDH757E08]

## 背景
reviewer run が判定を期待 path（対象 task の `artifacts/review.json`）ではなく、cwd からの相対 path で repo 内の
`artifacts/review.json` に書く事故が続いている（2026-10-21・10-22・11-02 に各 1 件、11-06 に 3 件以上）。
11-06 の 01M6QZ031R（reviewer run 01M6RZM9XB）では、reviewer は result.json を期待 path に書いたが、review.json は
`cat > artifacts/review.json` で repo 内に書いた。中身は全基準 pass だったが、celeris は
`artifacts/review.json not found or unreadable`（`crates/task-dispatch/src/review.rs`）で全基準を不合格にし、
task は failed になった。人が reopen して worker run と reviewer run をやり直した。

review.json が回収されない理由は 2 つある。
1. `crates/task-worker/src/result_location.rs` の `misplaced_result` は、探索で file 名 `result.json` だけを
   候補にする（file 名が固定）。review.json は見つけようがない。
2. review.json を読む `crates/task-dispatch/src/review.rs` は、期待 path を直接 `read_to_string` し、無ければ
   その場で落とす。回収の関数を一度も呼ばない。

## 決定
- D1: 回収は 1 つの関数 `misplaced_report(file_name, artifacts_dir, cwd, since)` にする。`file_name` は回収する
  report の file 名（`"result.json"` / `"review.json"`。path 区切りを含まない名前）。規則は report の種類に依らず同じ:
  - 期待 path（`artifacts_dir/<file_name>`）にあれば何もしない（期待 path が優先。誤配置元は見ない）。
  - 探索範囲は従来どおり（artifacts の親と cwd 配下。task root 直下の `wu/`・`runs`・`.git`・`target`・
    `node_modules` と symlink は辿らない。ADR 2026-10-18-result-location-scope D1）。候補は同じ file 名の file。
  - mtime が run 開始（2 秒の slack）以後の候補がちょうど 1 件なら、期待 path へ copy して `None` を返し、
    通常の検証に読ませる。誤配置元は残す。
  - 0 件（古い 1 件だけの場合を含む）は `None`。期待 path は空のままなので、呼び出し元の「report 不在」の経路が
    従来どおり落とす。
  - 新しい候補が複数件なら曖昧なので推測せず、permanent error（retryable=false）を返す。message は
    `<file_name> misplaced: expected …; found …; reports were not adopted`。
- D2: result.json 専用の旧い関数 `misplaced_result` は消す。呼び出し元（`claude_code.rs`・`codex.rs`・
  task-dispatch の `worker_task.rs`）は `misplaced_report("result.json", …)` を呼ぶ。result.json の挙動
  （message の文面と `MISPLACED_RESULT_MARKER` = `result.json misplaced:` を含む）は変えない。
- D3: reviewer の判定（review.rs）は、review.json を読む前に `misplaced_report(REVIEW_FILE_NAME, …)` を
  reviewer run の開始時刻で呼ぶ。複数件の error は判定の失敗理由として全基準に載せる。0 件は従来どおり
  `not found or unreadable`。配線は WU wire-review が行う。
- D4: 回収した report の中身は通常の検証（JSON の parse・欄の検査）にかける。回収は「置き場所」だけを直し、
  中身を信用する根拠にはしない。
- D5（WU wire-review で追加）: reviewer run の prompt を直す。確認した事実: reviewer run は `work_dir` を持たず
  （`review.rs` の `work_dir: None`）、cwd は対象タスクの作業場所になる。そのため (1) cwd と成果物の注意
  （`claude_code::work_dir_note`）は `work_dir != workspace` の run だけに出て、reviewer には出ていなかった。
  (2) 成果物ディレクトリが workspace の中にあると `RunRequest::artifacts_rel` は相対 `artifacts` を返し、
  Result 節は「`artifacts/review.json` に書け」と相対で指示していた。reviewer が判定のため `repos/<name>` へ
  cd してから `cat > artifacts/review.json` すると repo の中に落ちる（01M6QZ031R）。
  直し方: `artifacts_rel` は `TaskKind::Review` の run でも絶対パスを返す（Result 節・result.json の指示が
  絶対パスになる。全 harness 共通）。`work_dir_note` は reviewer run にも result.json 側と同じ文面の注意を出し、
  review.json と result.json の両方の絶対パスを書く。codex は同じ関数を使う。

## 代替案
- review.json 用に別の関数を足す: 探索範囲・除外・mtime の規則を 2 か所で持つことになり、片方だけ直す事故が起きる。採らない。
- reviewer の prompt だけを直す: 誤配置は prompt があっても起きている（result.json は期待 path に書けた run でも
  review.json だけ誤る）。prompt の確認は D3 と同じ WU で行うが、決定的な回収を代わりにはしない。

## 検証
`result_location` の単体試験: 既存の result.json 試験（範囲・除外・古い 1 件・複数件・1 件の回収）に加え、
review.json の 1 件の回収、0 件・複数件・古い 1 件の拒否、期待 path 優先、file 名の取り違え（result.json を
review.json として拾わない）を固定する。

WU wire-review: `crates/task-dispatch/src/review/tests.rs` で、repo 内の誤配置 1 件が回収され criteria が判定されること、
複数件は `review.json misplaced` で全基準不合格、0 件・run 開始より古い 1 件は `not found or unreadable` のままで
あることを固定する。reviewer prompt の注意と絶対パスは claude_code / codex の adapter 試験で固定する。
