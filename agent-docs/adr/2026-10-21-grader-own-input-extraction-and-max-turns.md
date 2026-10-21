# ADR: 採点は固定入力 tgz を grader 自身が展開して照合する / B の task 雛形は max_turns = 100 を必須にする

- 日付: 2026-10-21
- 状態: 実装済み（agent-docs/progress/2026-10-21-grader-input-and-max-turns.md 参照）

## 背景
mlsys-main-3 の実 pilot（task 01M5BT8JYW）で 2 件が見つかった。

1. grader の recompute 検査（`crates/campaign/src/grader.rs` の `recompute::check`）は作業者の artifact 内
   `inputs/runs/…` の展開済み入力を読む。case.json は `cd inputs && tar -xzf` と指示するだけなので、A は `ex/`、
   B は `inputs/x/` に展開し、全項目が「入力ファイルが無い」で落ち、failure_class が `model_failure` と誤分類された。
2. B の task の `budget.max_turns` が既定 10（`crates/task-ops/src/add.rs` の `DEFAULT_MAX_TURNS`）に黙って落ち、
   worker は 10 turn で止まり、reviewer は `error_max_turns` で失敗し、per_case token 上限に止められた。

## 決定
### D1 grader が自分で展開する
- `RecomputeSpec` に `input_archive`（`{path, sha256}`。path は case file の dir（`case_dir`）基準）を足す。
  grader は archive の sha256 を照合してから `work_root` 配下の自分の一時 dir に展開し、`pinned_raw` と metrics の
  raw 入力はその展開先から読む。作業者 artifact 内の `inputs/` `ex/` `inputs/x/` は読まない。
- archive が無い・sha256 不一致・展開失敗・`pinned_raw` の sha256 不一致（grader 側の固定入力の不整合）は
  `CheckResult::env_failure`（failure_class = `env_failure`）にする。`model_failure` にしない。
  作業者の報告が不正（metric 行なし・許容差外）なときだけ model_failure。
- `input_archive` を持たない case（raw を作業者が成果物として作る dagbench 型）だけが従来どおり artifact 内を読む。
  実資産 case（k3/k4/short-ml）の原票 tgz は repo に無いため、`input_archive` への移行は次 campaign の再 freeze で行う
  （「本番への移行」）。
- case の task.md 本文にも展開先（`inputs/` 直下）を明記する。

### D2 max_turns は雛形に必須
- `celeris_exec::check_template` は `max_turns` が正の整数でなければ拒否する（preflight の task-template 検査と
  `--task-template` 読み込みの両方が通る）。次 campaign の雛形（launcher / `docs/ops/measure-run-final.md`）は
  `"max_turns": 100`。既定 10 に黙って落ちる経路は無い。
- worker と reviewer は同じ `req.task.budget.max_turns` を `--max-turns` に渡す（task-worker の claude_code）。
  これを試験で固定する。

## 試験（無 LLM・決定的）
(a) 別の場所（ex/・inputs/x/）に展開した workspace でも同じ判定、(b) archive 欠落/sha256 不一致が env_failure、
(c) 雛形 max_turns=100 が task budget と reviewer の `--max-turns` に届く、(d) max_turns の無い雛形が拒否される。

## 本番への移行
凍結済み mlsys-main-3 の成果物は書き換えない。次 campaign の再 freeze 時に case.json へ `input_archive` を入れ、
task-template.json に `"max_turns": 100` を足して preflight を通す。
