# ADR 2026-10-18: 初期 tree の baseline build で compile error の出どころを分ける

- 日付: 2026-10-18
- 状態: 採用・実装済み
- 関連: 2026-10-16-mlsys-independent-grader、2026-10-17-campaign-lock、2026-10-18-k2-judged-scope、2026-10-18 本測定移行 P1-3（build-diag-fix）

## 背景
host の実 build（host-build2）で K2 の known-bad / known-good が同じ rustc error（E0433/E0599、`src/ffi/io_buffer.rs`）で落ちた。
原因は評価 command が `--features transport-locusta` を有効にしておらず、初期 tree 自体が compile できなかったこと。
採点器は `error[E` を成果物の失敗の印にしていたので、case の不良がモデルの失敗に数えられ、known-bad の「不合格」も偽の合格になっていた。

## 選択肢
1. 評価 command の feature を直すだけ。→ 次に case/asset が初期 tree と食い違ったとき、また黙ってモデル失敗に数える。
2. rustc error の場所（`--> path`）が成果物で未変更の file なら env とみなす。→ 成果物が別 file で定義を消した場合も env にしてしまい、モデル失敗を隠す。
3. case に baseline build を持たせ、compile error が出たときだけ grader 側の初期 tree で同じ build を走らせる。**採用**。

## 決定
- D1 `celeris.grade-case/1` の `implementation.baseline_command`（任意）。隠しテストを挿入しない、評価と同じ build 設定の compile だけの command。
  生成元は `celeris.campaign-case-private/1` の `baseline_command`。持たない case の grade-case は変わらない。
- D2 `GradeOptions.initial_tree`（CLI `grade --initial-tree`、`grade-out --initial-trees <dir>/<problem_id>`）。grader が lock どおりに組んだ初期 tree。
- D3 評価が compile error で落ち、baseline を持つ case では、初期 tree で baseline を走らせる。落ちれば env_failure（case/asset の不良）、
  通ればモデル失敗。初期 tree が無ければ出どころを決められないので env_failure（初期 tree を渡して採点し直す）。成功条件は変えない。
- D4 K2 の評価 command・`visible_command` に `--features transport-locusta`、能力に `rdma-verbs`。grade-case/private の hash が変わるので
  `grade_case_ref`・`private_ref` を更新し、旧 lock では採点できない。
