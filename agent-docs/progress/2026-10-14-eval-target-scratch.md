---
title: "評価試行の cargo target を試行ごとの scratch にして後始末する（/var/tmp 溢れの修正）"
tasks: [01M4WHYHVZTYNBRGM5C3PWHMZE]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# 評価試行の cargo target を試行ごとの scratch にして後始末する

2026-10-14 の本評価（評価 task 01M4VAAKFJ、266 試行）でルート fs（504G）が約 2 時間で 100% になった件の修正。
評価試行の worktree はすべて basename が `repo` なので、hook が書く `/var/tmp/agent-platform-build/repo` を全試行が共有し、
成果物が積み上がって消えなかった。

設計判断は [ADR 2026-10-14-eval-trial-scratch-target](../adr/2026-10-14-eval-trial-scratch-target.md)（D1〜D3）。
WorkUnit ごとの記録は `2026-10-14-eval-target-scratch/<wu-key>.md`（adr-script・eval-target・var-tmp-audit）。

## やったこと（要約）
- D1（eval-target）: 評価試行は owner `agent-eval-<trial_id>` の scratch lease を作り、その target を `CARGO_TARGET_DIR` で
  adapter と `RunRequest.cargo_target_dir` に渡す。hidden check も同じ target で走る。採点の後・異常終端・回復・停止で
  作業ツリーと target を消し lease を release する。scratch が無効なら試行は `error`。
- D2（adr-script）: `scripts/dev/worktree-target-dir.sh` は `CARGO_TARGET_DIR` が env にあれば `.cargo/config.toml` を書かない。
  置き場は `CELERIS_BUILD_CACHE`、無ければ `<CELERIS_SCRATCH_DIR>/dev-targets`。ルート fs 上なら書かずに止める。
  `/var/tmp/agent-platform-build` の既定は撤去。名前は `<basename>-<path の sha256 先頭 12 桁>`。
- D3（var-tmp-audit）: `target_sweep` の既定 roots（celeris config・dispatcher fallback・celerisctl）から
  `/var/tmp/agent-platform-build` を消し、`<scratch>/dev-targets` にした。ふだんの task run（ADR-0075 の scratch）と
  release（`SD_CARGO_TARGET`、/local）は /var/tmp を使っていない。

## 証拠（closeout、統合後 HEAD 31ce9505 で実行）
- `bash scripts/dev/test-parallel.sh` → exit 0、`test-parallel: ok`。
  CELERIS_TEST_SUMMARY: passed 5094 / failed 0 / ignored 13、nextest_exit 0、doctest_exit 0、tmp_leftovers 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/tests/worktree-target-dir.sh` → exit 0、`all passed`（同じ basename の 2 worktree が別 target になる項目を含む）。
- `git grep -n agent-platform-build -- crates config docs scripts` → 0 件（exit 1）。
- `cargo test -p task-dispatch --lib -- --exact dispatcher::tests::eval_trial_dispatch::eval_run_request_sets_per_trial_cargo_target_dir`
  → `1 passed`。
- `cargo test -p task-dispatch --lib -- --exact dispatcher::tests::eval_trial_dispatch::eval_trial_target_removed_after_scoring`
  → `1 passed`。

## 本番への移行
運用セッション（人）が実行する。daemon・config・`/var/tmp` の操作は worker からはしていない。

1. release の昇格: この task が main に入った後の release を作り検証して昇格する。
   - `scripts/selfdeploy/release.sh` → `verify.sh` で `verify.json.ok` が真であることを確かめ、
     `scripts/selfdeploy/promote.sh <sha12>`。
   - 確認: `curl -s localhost:<port>/health` の版が `<sha12>`、`systemctl --user status celeris@<sha12>` が active。
   - 前提: 本番の `[scratch]` が有効（`/local/celeris/data/scratch`、ADR-0136）。無効だと評価試行はすべて
     `error`（"scratch is disabled; …"）になる。確認: `grep -n -A2 '^\[scratch\]' ~/.config/celeris/config.toml`。
   - `[maintenance.target_sweep] roots` に `/var/tmp/agent-platform-build` を明示していれば外す
     （`grep -n roots ~/.config/celeris/config.toml`）。
2. 既存の `/var/tmp/agent-platform-build` 残骸の削除確認:
   - `du -sh /var/tmp/agent-platform-build/* 2>/dev/null` と `df -h /` で残りを確かめ、残っていれば
     `rm -rf /var/tmp/agent-platform-build`。
   - 手元の worktree の `.cargo/config.toml` は次の cargo 実行で hook が `<scratch>/dev-targets/<name>-<hash>` に書き換える。
     旧い置き場を指す物が残っていないこと: `grep -rl agent-platform-build ~/workspace/*/.cargo/config.toml 2>/dev/null` が空。
3. `[routing_eval] reserved_runs` を 2 に戻す: `~/.config/celeris/config.toml` の `reserved_runs = 0` を `2` に戻し、
   config を再読込（または daemon 再起動）。確認: `grep -n reserved_runs ~/.config/celeris/config.toml`。
4. 評価再開:
   - 止めた batch（01M4W9H8…）の未完了分は ADR 2026-10-14-eval-trial-result-placement D4 の手順どおり
     `celeris-routing-eval run --trial-offset <n>` で投入し直す（旧い行は消さない）。
   - 再開直後に試行の target が `/local/celeris/data/scratch/targets/` の下に `agent-eval-<trial_id>` ごとに作られ、
     採点後に消えることを確かめる: `ls /local/celeris/data/scratch/targets/ | grep -c agent-eval` が同時走行数（≤2）を超えない、
     `df -h /` が増えない、`ls /var/tmp/agent-platform-build` が無い。

## 未解決事項
- 本番の昇格・config 書き戻し・評価再開は人の操作待ち（上の節）。

## 提案
- `<scratch>/dev-targets` は lease を持たないので `scratch_gc` は消さない。`target_sweep` の既定 roots に入れたので古い target は
  sweep で消えるが、worktree が消えた target を即消す保守が要るかは運用の様子を見て判断する。
