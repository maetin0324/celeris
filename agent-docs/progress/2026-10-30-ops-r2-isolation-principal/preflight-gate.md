---
title: ops-r2 preflight の deny_readable gate
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
status: completed
updated: 2026-10-30
---
# ops-r2 preflight の deny_readable gate

完了日: 2026-10-30

## 変更

- commit 件名: `preflight-gate: 正式試行の preflight が主体ごとに runtime probe を走らせ deny_readable で止める`
- 新 module `crates/campaign/src/preflight_isolation.rs`。`preflight.rs` は `check_isolation` を `check_measure_env` の直後に呼ぶだけ（B/C の能力 probe より前。B/C probe は他の不足が 0 件のときだけ走るので、deny_readable が残れば測定 daemon に task も作らない）。
- `Host::runtime_probe`（既定は実行、試験は差し替え）を追加。主体と包み方:
  - A: `Sandbox::runtime_args`（A の実起動と同じ関数）。toolchain は `--cap-env` の `a`、実行ファイルは preflight が解決した A の program。probe binary は一時 workspace に stage。
  - B（A 以外の arm が有効なとき）・operator（正式試行では常に）: `ops-r2-sandbox.sh` を `--print-args` で許可表を得てから、同じ引数で実際に起動。probe binary は `--state/.preflight-probe/` に一時的に置き、終了後に消す。
  - L1: 人の uid の process 自身が deny list のうち `--eval-root` の下の path を開く。
- mount の照合: 許可表の bind 元（`--ro-bind`・`--bind`・`--dev` の host 側）が載っている host mount の source と pseudo fs（tmpfs・proc・devpts・mqueue）を `--allow` に渡す。source 欄は device 名なので「許可表の外の filesystem（NFS の home 等）が見えていないか」の照合になる。
- CLI 引数（preflight・run・watch 共通）: `--deny-list <file>`（1 行 1 絶対 path、`#` 行と空行は無視。glob は展開しない）、`--eval-root`、`--sandbox-script`（既定 `scripts/campaign/ops-r2-sandbox.sh`）、`--probe-program`（既定は自身）、`--b-state/--b-release/--b-account/--b-ro`、`--operator-state/--operator-release/--operator-account/--operator-ro`。
- `measure-launcher.sh` は env file の `DENY_LIST`・`EVAL_ROOT`・`SANDBOX_SCRIPT`・`PROBE_PROGRAM`・`B_*`・`OPERATOR_*` を同じ引数で渡す（任意。欠ければ CLI の preflight が拒否する）。
- missing の形（check は全て `deny_readable`）:
  - 読めた・親を一覧できた path: `deny_readable:<主体>:<path>`
  - 許可表の外の mount source: `deny_readable:<主体>:mount:<source>`
  - probe が走らない（bwrap 不可・JSON 無し）: `deny_readable:<主体>:probe:<理由>`
  - 入力不足（fail-closed）: `--deny-list` 無し・空・相対 path、B/operator の state・release・account 無し、`--eval-root` 無しか deny list にその下の path が無い
- 正式試行（`--measure-config` 指定）では常に走る。測定でなくても `--deny-list` を渡せば走る。
- 既存の CLI e2e 試験（launcher・arm_set_cli_normal・trial_state_cli_e2e・preflight の CLI 試験）と Inputs を組む試験に隔離の入力を足した。B・operator は stub の sandbox script、bwrap を stub にした launcher 試験は `--probe-program` の stub を使う。実 bwrap が使える試験では A の probe は実 bwrap で走る。
- `runner_checker` の「子 process を起こしてよい module」に `preflight_isolation.rs` を追加（bwrap と sandbox script だけを起動し、LLM・network は使わない）。

## 計画 v2 の範囲 check の不合格と v3

- v2 の run の後、範囲 check（許可は campaign の src 6 本・tests/preflight.rs・tests/runtime_probe.rs・agent-docs のみ）が不合格になった。fail-closed の結果、measure 指定で preflight を通す既存試験・共通 fixture・runner_checker の許可一覧・`campaign_lock_e2e.rs`・`measure-launcher.sh` の変更が必須なため。試験だけで fail-closed を外す迂回は入れず、plan_issue で返した。
- 計画 v3 でこれらが範囲に入った。code は変えず、v3 の run で下の検査を再実行した。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `bash scripts/dev/test-affected.sh` → exit 0、2,866 passed・9 skipped（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `python3 scripts/dev/check-source-size.py` → exit 0、0 failure（preflight.rs の追加は 30 行程度）。
- `cargo nextest run -p celeris-campaign --test preflight` → 30 passed。新規 3 件:
  - `isolation_probe_passes_when_every_principal_is_denied`: 全拒否で ok、4 主体（A・B・operator・L1）の包み方を確認。
  - `one_readable_path_in_any_principal_stops_the_run`: どの主体でも 1 件読めれば `deny_readable:<主体>:<path>`、`run_gated` は Err・transport 呼び出し 0 回・out 無し。mount 外 source と probe 失敗も止まる。
  - `measured_run_without_deny_list_or_principal_inputs_stops`: deny list 無し・空、operator の account 無し、eval root の不一致で止まる。測定でなく deny list も無ければ走らない。
- `CELERIS_USERNS_TESTS=require cargo nextest run -p celeris-campaign --test runtime_probe` → 6 passed。新規 `preflight_principals_probe_with_their_real_wrapping` が実 bwrap（A）と実 `ops-r2-sandbox.sh`（B・operator）の中で、許可表の外の file を読めず・親を一覧できず・mount 外 source 0 件、`--state` の中に置いた file は読める（probe が実際に開いている）ことを確認。L1 は同じ uid の file を開けるので `deny_readable:L1:<path>` になる（chmod では防げない）。
- 同じ試験を env 無しで実行 → stderr に `skip: preflight principal probes require CELERIS_USERNS_TESTS=1 (or require)`。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、変更は campaign の src 5 本・tests 15 本、`crates/celeris/tests/campaign_lock_e2e.rs`、`scripts/campaign/measure-launcher.sh`、この進捗（計 23 path、v3 の範囲内）。

## 未解決

- 実 runtime の拒否確認は、この run の host（bwrap が使える）で一時 dir の fixture に対して行った。本番の deny list（`EVAL_ROOT` の private、repo clone の grader-private、`~/.claude` 等）と測定専用 account・release に対する確認は未実施。評価器の uid（`celeris-eval`）と `EVAL_ROOT` が無い今は L1 が必ず失敗する（同じ uid では開ける）ので、人の準備が済むまで正式試行の preflight は通らない。
- deny list は glob を展開しない。`/local/celeris/data/workspaces/*/repos/*` のような項目は、承認票を作るときに実在の path へ展開して書く。
- 親 dir を一覧できるだけでも hit にする（probe の `ok` と同じ規則）。deny list には dir そのものより中の file を書く（dir を書くと、その親が一覧できる場所では誤って hit になる）。
- 本番 host の uid・権限・設定は変更していない。人の準備手順は close-docs unit が書く。

## 提案

なし。
