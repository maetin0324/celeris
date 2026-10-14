# ADR 2026-10-14: 評価試行の cargo target は試行ごとの scratch — worktree-target-dir.sh は env 優先・ルート fs を既定にしない

- 日付: 2026-10-14
- 状態: 採用（D2 は葉 adr-script で実装済み。D1・D3 は同じ task の葉 eval-target・var-tmp-audit が実装する）
- 関連: ADR-0075（target は使い捨ての scratch pool の entry・lease・`scratch_gc`）、ADR-0136（hot データは `/local`、
  scratch は `/local/celeris/data/scratch`）、ADR 2026-10-07-build-tmp-hygiene（共有 target の掃除）、
  `agent-docs/ops/dev-builds-local-target-dir.md`

## 1. 文脈

2026-10-14 の本評価（266 試行）で、ルート fs（`/dev/mapper/pve-vm--100--disk--0`、504G）が約 2 時間で 100% になった。
運用セッションが投入を止め、`/var/tmp/agent-platform-build/` の `repo`（221G）・`eval-01M4W9H8`（85G）・`agent-platform`（31G）を
消して復旧した。

- 評価試行の worktree（`/local/celeris/data/workspaces/eval/<trial>/repo`）で harness の PreToolUse hook が
  `scripts/dev/worktree-target-dir.sh` を走らせ、`.cargo/config.toml` に `target-dir = "/var/tmp/agent-platform-build/repo"` を書いた。
  名前が worktree の basename だけなので、全試行が 1 つの target を共有した。試行ごとに base commit が違うので成果物が積み上がった。
- script の既定の置き場 `/var/tmp/agent-platform-build` はルート fs にある。ADR-0136 で hot データを `/local` に移したが、この既定は残っていた。
- script は `CARGO_TARGET_DIR` が env にあっても config を書いていた（env が優先されるので実害は試行のように env が無いときだけだが、
  env の無い経路に落ちた瞬間に古い置き場へ書く）。
- 評価試行の RunRequest（`crates/task-dispatch/src/dispatcher/eval_trials.rs` の `eval_run_request`）は、ふだんの task run が
  受ける scratch の target（`CARGO_TARGET_DIR`、scratch pool の lease）を受けていないと見られる（葉 eval-target が確かめる）。

## 2. 決定

### D1. 評価試行は試行ごとの scratch target を `CARGO_TARGET_DIR` で受け、試行の終端で消す

- 評価試行の RunRequest に、試行ごとの `CARGO_TARGET_DIR` を ADR-0075 の scratch pool（`/local/celeris/data/scratch`、ADR-0136）から渡す。
  同時に走る試行どうしで target を共有しない（owner は試行の id）。
- hidden check（`check_cmd`）も同じ target を使う。採点の後、試行の終端で target を消す（lease を返し、中身を消す）。
- 実装と決定的な試験（RunRequest に試行ごとの `CARGO_TARGET_DIR` が入り、終端で消える）は葉 eval-target。

### D2. `scripts/dev/worktree-target-dir.sh` は env を優先し、ルート fs を既定にしない

- `CARGO_TARGET_DIR` が env にあれば `.cargo/config.toml` を書かずに exit 0。Celeris の run・評価試行は D1 と ADR-0075 の target を
  env で受けるので、hook はそれを上書きしない。
- 置き場は `CELERIS_BUILD_CACHE`。無ければ `<CELERIS_SCRATCH_DIR>/dev-targets`（`CELERIS_SCRATCH_DIR` の既定は ADR-0136 の
  `/local/celeris/data/scratch`）。scratch が無い host では `CELERIS_BUILD_CACHE` の明示を求める。
- 書かずに止める条件: `CELERIS_BUILD_CACHE` が存在しない、scratch が無い、置き場がルート fs 上（`stat -c %d` が `/` と同じ）。
  止めるとき、この script が前に書いた `.cargo/config.toml`（生成印のあるもの）は消す（古い置き場へ書き続けない）。
  hook は常に exit 0（ブロックしない）のまま。
- 名前は `<worktree の basename>-<worktree の絶対 path の sha256 先頭 12 桁>`。basename が全試行で `repo` でも target を共有しない。
  本体 checkout の `-main` 印は hash で区別できるので廃止。
- `/var/tmp/agent-platform-build` の既定は撤去する（破壊的変更の方針。互換の fallback を残さない）。
- 試験は `sh scripts/tests/worktree-target-dir.sh`（一時 git repo で env 優先・既定が scratch 配下・置き場なし／ルート fs で書かない・
  同 basename の 2 worktree が別 target）。`WORKTREE_TARGET_DIR_ROOTFS` はルート fs と見なす path を差し替える試験専用の口。

### D3. `/var/tmp/agent-platform-build` を使う残りの経路を消す

- ふだんの task run・release・保守（例: ADR 2026-10-07-build-tmp-hygiene D1.1 の `target_sweep` の既定 roots）に
  `/var/tmp/agent-platform-build` を既定に持つ経路が残っていれば同じく撤去する（葉 var-tmp-audit）。

## 3. 帰結

- 人の手元の worktree の target は `/local/celeris/data/scratch/dev-targets/<name>-<hash>` に移る（初回は作り直し）。
  `/var/tmp/agent-platform-build/` の残りは人が消す（`agent-docs/ops/dev-builds-local-target-dir.md` の掃除）。
- `dev-targets` は scratch pool の lease を持たないので `scratch_gc` の対象外。worktree を消したら人が対応する dir を消す。
- `~/.cargo/config.toml` の家訓のコメントが `/var/tmp/agent-platform-build` を指していれば人が直す。
