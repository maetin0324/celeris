#!/usr/bin/env bash
# scripts/selfdeploy/release.sh <git-ref> — ADR-0040 D1/D2 の「リリース」段。
#
#   作業チェックアウトとは別の detached の作業ツリー（$CELERIS_STATE_DIR/releases/.build/<sha12>）で
#   cargo test（Phase SD-2: scripts/dev/test-parallel.sh でバイナリ並列）→ clippy → source-size-report
#   （ADR-0083、warning のみ）→ build --release → GUI pnpm install/typecheck/test/build
#   → pnpm mobile-audit → pnpm e2e:mock を順に回し、
#   全部 exit 0 のときだけ $CELERIS_STATE_DIR/releases/<sha12>/ を作る。
#   1 つでも非 0 なら**リリースを作らず**、.build/<sha12>/gate.json だけ残す。
#
# Phase 89（ADR-0041 追記、ADR-0055 D3）: `pnpm-build` の直後に `pnpm-mobile-audit`（`pnpm mobile-audit`。
# ADR-0055 D1 の全画面 × light/dark）と `pnpm-e2e-mock`（`pnpm e2e:mock`。Phase 83 / G36 の読み取り専用
# e2e をオフラインの偽 celeris に対して）を足した。どちらも `pnpm build` 済みの `$BUILD/gui/build` を
# 使い回す（`MOBILE_AUDIT_SKIP_BUILD=1` / `E2E_SKIP_BUILD=1`）ので、ビルドをやり直さない。
# これで見た目のデザイン退行（画面が壊れているのに 200 は返る等）を積んでリリースを作ることが無くなる。
#
# Phase SD-1（ADR-0075 追記「Phase SD-1 実装時の逸脱」、ADR-0041 追記 §7）: 1 周（release + verify）が
# 14 分かかっていたのを縮める。
#   - 作業ツリーは場所を固定（`.build/tree`）して使い回し、毎回 `git checkout --force <sha>` + `git clean -ffdx`
#     でその sha のきれいな checkout にする。target は全リリースで 1 つの scratch owner `release-build`。
#     git は内容の変わったファイルしか書き直さないので、cargo は変わった crate だけを作り直す。
#   - `current` から gui/ に 1 つも変更が無いリリースでは、GUI の検査だけの段（`pnpm-test`・
#     `pnpm-mobile-audit`・`pnpm-e2e-mock`）を飛ばし、gate.json に `skipped: true` と理由を残す。
#     `pnpm-install` / `pnpm-typecheck` / `pnpm-build` は梱包に要るので必ず回す。Rust の段は必ず回す。
#   - 梱包の `pnpm install --prod --frozen-lockfile` は lockfile ごとに 1 度だけ
#     `releases/.pnpm-prod-cache/<key>/` で行い、リリースの `gui/node_modules` はそこへの相対 symlink にする。
#
# ADR 2026-10-04-release-notes: gate.json を書いた後、梱包した `bin/celerisctl release notes` で
# `<release>/notes.json` と `notes.md`（`current` からこの sha までの task・migration・schema・ADR・config 例・
# gate の飛ばした段）を作る。失敗は警告だけでリリースは作る。`SD_RELEASE_NOTES_BIN` / `SD_RELEASE_NOTES_API` で差し替え。
#
# 本番には一切触れない（プロセスも DB も config も）。人でもワーカーでも実行してよい（D5）。
# ADR-0126 B4: userns の要る試験は既定 skip だが、この gate は既定で `CELERIS_USERNS_TESTS=1` を立てて走らせる。
set -euo pipefail

SD_PROG=release
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

usage() {
  cat >&2 <<'EOF'
usage: release.sh <git-ref>

  <git-ref>  ビルドする commit（ブランチ名 / タグ / sha。`HEAD` も可）

env:
  CELERIS_CONFIG_DIR  既定 ~/.config/celeris（config.toml と秘密）
  CELERIS_STATE_DIR   既定 ~/.local/celeris（releases / backups / staging …）
  SD_REPO     既定 ~/workspace/agent-platform（git worktree を生やす元のリポジトリ）
  SD_AUDIT_TIMEOUT  既定 600（秒）。`pnpm-mobile-audit` / `pnpm-e2e-mock` それぞれの壁時計の上限
                    （Phase 89。ADR-0041 追記）。Playwright の Chromium 実行ファイルが
                    ~/.cache/ms-playwright に無いホストでは、この 2 ステップは待たずに
                    「false — playwright browser not installed」で失敗する（`docs/ops/selfdeploy.md` 参照）。
  SD_GATE_FORCE_GUI  1 なら gui/ に変更が無くても GUI の検査の段を飛ばさない（Phase SD-1）
  SD_RELEASE_SCRATCH_OWNER  既定 release-build（全リリースで共有する scratch の owner。Phase SD-1）
  SD_RELEASE_TARGET_TTL     既定 172800（秒）。共有 target の lease の TTL（最後のリリースからこの間は P0）
  SD_RELEASE_PRUNE   0 なら最後の掃除（古いリリースと使われない .pnpm-prod-cache の削除）を飛ばす
  SD_GATE_TEST_RUNNER  既定 nextest（`scripts/dev/test-parallel.sh`: テストバイナリを並列。Phase SD-2）。
                       cargo-test なら従来の `cargo test --workspace`（直列。非常用）
  SD_TEST_PARALLEL     既定 ビルドする sha の scripts/dev/test-parallel.sh（テスト用の差し替え口）
  CELERIS_TEST_JOBS    並列に走らせるテストの数（既定 min(8, max(2, nproc/3))。test-parallel.sh が読む）
EOF
  exit 2
}

[ $# -eq 1 ] || usage
REF="$1"

sd_require_json_tool
command -v cargo >/dev/null 2>&1 || sd_die "cargo not found"
# 既定の runner は**ビルドする sha の** `scripts/dev/test-parallel.sh`（作業ツリーを作った後で決める）。
# release.sh の隣を見ない: リリースに同梱した release.sh（`current/scripts/release.sh`、配送の prepare.sh が
# 起こす）は `scripts/` に平らに置かれ、`dev/` を持たないので「test runner … not found」で落ちていた。
if [ "${SD_GATE_TEST_RUNNER:-nextest}" = nextest ]; then
  if [ -n "${SD_TEST_PARALLEL:-}" ]; then
    [ -f "$SD_TEST_PARALLEL" ] || sd_die "test runner $SD_TEST_PARALLEL not found"
  fi
  # 早めに（作業ツリーを作る前に）分かりやすく落とす。版の照合は test-parallel.sh が行う。
  cargo nextest --version >/dev/null 2>&1 \
    || sd_die "cargo-nextest is not installed; install once: cargo install cargo-nextest --locked --version $(git -C "$SD_REPO" show "$REF:tools/nextest/VERSION" 2>/dev/null | tr -d ' \n' || true) (docs/ops/nextest.md), or set SD_GATE_TEST_RUNNER=cargo-test"
fi
sd_use_pnpm

SHA12="$(sd_sha12 "$REF")"
SHA_FULL="$(sd_sha_full "$REF")"
sd_mkdirs

# Phase SD-1: ビルドは場所を固定した作業ツリー（`.build/tree`）で行う。`.build/<sha12>/` は gate が落ちたときの
# gate.json とログの置き場（ただのディレクトリ。docs/ops/selfdeploy.md §2）。
BUILD="$SD_BUILD_TREE"
FAILED_DIR="$SD_BUILD_ROOT/$SHA12"
REL="$(sd_release_dir "$SHA12")"
RELEASE_T0="$(sd_now)"
GATE_TSV="$(mktemp)"
trap 'rm -f "$GATE_TSV"' EXIT

# ADR-0041 D2: 同じ sha のビルドは 1 本だけ（`.build/<sha12>` のワークツリー作成が競合する。
# `cargo` の target lock は「待つ」が、`git worktree add` は先に消してから作るので壊れる）。
# **待たない**（同じ sha を 2 度ビルドしても結果は同じなので、待つ意味が無い）→ exit 75。
sd_lock_or_tempfail 9 "$SD_BUILD_ROOT/.lock-$SHA12" 0 "release.sh of $SHA12"

# 異なるSHAも同じCargo成果物を使う。Cargo自身のロックはテスト実行中には
# 外れるため、別SHAのビルドがdoctestのrlibや梱包前のバイナリを上書きできる。
# worktreeの変更から梱包・掃除まで、release全体を同じロックで保護する。
sd_lock_or_tempfail 8 "$SD_RELEASES/.lock-release" "${SD_RELEASE_LOCK_WAIT:-1800}" "release.sh sharing Cargo artifacts"

CUR="$(sd_current_sha)"
PREV="$(sd_previous_sha)"
if [ -d "$REL" ]; then
  if [ "$SHA12" = "$CUR" ] || [ "$SHA12" = "$PREV" ]; then
    sd_die "release $SHA12 is currently \`current\`/\`previous\`; refusing to rebuild over it"
  fi
  sd_log "release $SHA12 already exists; rebuilding it (old directory will be replaced)"
fi

sd_log "ref=$REF sha=$SHA_FULL sha12=$SHA12"
sd_log "build worktree: $BUILD"

# ---- 作業ツリー（Phase SD-1: 場所を固定して使い回す） ----------------------
#
# `.build/tree` が同じリポジトリの有効な worktree なら `git checkout --detach --force <sha>` と
# `git clean -ffdx`（追跡外・ignore 済みも全部消す: gui/node_modules・gui/build・前回の .gate-*.log）で
# **その sha のきれいな checkout**にする。無い・壊れているときだけ作り直す（`git worktree add`）。
# git は内容の変わったファイルだけを書き直す（mtime が今になる）ので、同じ target を使い続ける cargo は
# それを見て変わった crate だけを作り直す。内容の同じファイルの mtime は前回の checkout のまま＝前回の
# 成果物より古いので、再利用される（中身が同じなので正しい）。この作業ツリーと共有 target を触るのは
# `.lock-release` を持つこの release.sh だけ（2 本目は上で待つ）。

prepare_build_tree() {
  local common_repo common_tree
  BUILD_TREE_REUSED=false
  common_repo="$(git -C "$SD_REPO" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
  if [ -f "$BUILD/.git" ]; then
    common_tree="$(git -C "$BUILD" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
    if [ -n "$common_repo" ] && [ "$common_tree" = "$common_repo" ] \
      && git -C "$BUILD" checkout --quiet --detach --force "$SHA_FULL" >&2 \
      && git -C "$BUILD" clean -ffdxq >&2; then
      BUILD_TREE_REUSED=true
    else
      sd_log "build worktree $BUILD is not usable; recreating it"
    fi
  fi
  if [ "$BUILD_TREE_REUSED" != true ]; then
    git -C "$SD_REPO" worktree remove --force "$BUILD" >/dev/null 2>&1 || true
    rm -rf "$BUILD"
    git -C "$SD_REPO" worktree prune
    git -C "$SD_REPO" worktree add --detach "$BUILD" "$SHA_FULL" >&2
  fi
  # 念のため: HEAD がその sha で、追跡外・変更が 1 つも無いこと（gate がその sha の中身だけを見る保証）。
  [ "$(git -C "$BUILD" rev-parse HEAD)" = "$SHA_FULL" ] \
    || sd_die "build worktree $BUILD is not at $SHA_FULL after checkout"
  [ -z "$(git -C "$BUILD" status --porcelain --ignored --untracked-files=all | head -n 1)" ] \
    || sd_die "build worktree $BUILD is not clean after checkout"
}
prepare_build_tree
sd_log "build worktree: $BUILD (reused=$BUILD_TREE_REUSED, HEAD=$SHA12)"

# 前回までに同じ sha の gate が落ちたときの記録（と、Phase SD-1 より前の `.build/<sha12>` の worktree）を消す。
git -C "$SD_REPO" worktree remove --force "$FAILED_DIR" >/dev/null 2>&1 || true
rm -rf "$FAILED_DIR"
git -C "$SD_REPO" worktree prune

# ADR-0075 D7（Phase G1）+ Phase SD-1: target は scratch pool の lease（owner は全リリースで 1 つの
# `release-build`）。adopt の安全条件の checkout 時刻は `.build/tree/.git` の mtime。終了時は成功・失敗とも
# **touch**（release しない。lib.sh の sd_scratch_lease の説明）。lease の TTL は `SD_RELEASE_TARGET_TTL`。
# celerisctl が無い・scratch が無効なら従来どおり `$SD_RELEASES/.cargo-target`。
if [ "${SD_USE_CALLER_CARGO_TARGET:-0}" = 1 ] && [ -n "${CARGO_TARGET_DIR:-}" ]; then
  SD_CARGO_TARGET="$CARGO_TARGET_DIR"
  sd_log "using caller CARGO_TARGET_DIR: $SD_CARGO_TARGET"
elif sd_scratch_lease "$SD_RELEASE_SCRATCH_OWNER" "$SHA_FULL" "$BUILD"; then
  trap 'rm -f "$GATE_TSV"; sd_scratch_touch' EXIT
fi
sd_log "CARGO_TARGET_DIR: $SD_CARGO_TARGET (owner ${SD_SCRATCH_OWNER:-<none: fallback>})"
mkdir -p "$SD_CARGO_TARGET"

# 共有 target が**この作業ツリーから**作られたものか（Phase SD-1）。違えば（新しい target・他の owner から
# adopt した target・Phase SD-1 より前の `.cargo-target`）workspace のメンバーを先に掃除する（従来の
# `cargo-workspace-clean` 段。外部依存は残す）。同じなら掃除しない（掃除すると全メンバーを作り直すことになり、
# 共有した意味が無くなる）。
TARGET_TREE_MARK="$SD_CARGO_TARGET/.celeris-release-tree"
WS_CLEAN_NEEDED=true
if [ "$(cat "$TARGET_TREE_MARK" 2>/dev/null || true)" = "$BUILD" ]; then
  WS_CLEAN_NEEDED=false
fi

export CARGO_TARGET_DIR="$SD_CARGO_TARGET"
# gate はネットワークに出ない前提（Cargo.lock / pnpm-lock.yaml は固定）。
export CARGO_TERM_COLOR=never
# Phase SD-1: 共有 target の fingerprint を呼び出し元の env に左右させない（incremental は profile の一部で、
# 切り替わると workspace のメンバーを全部作り直す）。`target/*/incremental/` も作らない（ADR-0075 G2 と同じ）。
export CARGO_INCREMENTAL=0
# ADR-0079 付記「R7-12」D4: 環境に依存する browser テスト（実 bwrap / Chromium / netns）は、環境が無いと理由を出して
# 飛ばす（worker の sandbox で無関係な task の受け入れ条件を落とさない）。release では飛ばさない（環境が無ければ失敗）。
export CELERIS_ISOLATION_TESTS="${CELERIS_ISOLATION_TESTS:-require}"
# ADR-0126 B4: userns の要る試験（実 browser/runtime/launcher、unshare/CLONE_NEWUSER）は既定で skip になった
# （worker run の sandbox が userns を作れないため）。release gate は host（userns が使える）で走るので、既定で
# 外したことで今まで release で守っていた退行の検出が消えないよう、ここで既定を 1 に戻す（環境が無ければ fail）。
# userns が使えない host で release するなら人が CELERIS_USERNS_TESTS=0 を明示する（gate の記録に残る）。
export CELERIS_USERNS_TESTS="${CELERIS_USERNS_TESTS:-1}"

printf 'step:s exit:i secs:f log:s skipped:b reason:s\n' >"$GATE_TSV"
GATE_OK=true
GATE_FAILED_STEP=""

# `run_step <name> <workdir> -- <cmd...>`
run_step() {
  local name="$1" workdir="$2"
  shift 2
  [ "$1" = "--" ] && shift
  local log="$BUILD/.gate-$name.log" start end secs rc
  if [ "$GATE_OK" != true ]; then return 0; fi
  sd_log "step $name: $* (cwd $workdir)"
  # ADR-0075 D2: 長い step の前に scratch の lease を touch する（TTL で回収されないように）。
  sd_scratch_touch
  start="$(date +%s.%N)"
  rc=0
  # lock の fd（8, 9）を子に継がせない（応答しない子が残ると lock が外れない。lib.sh の sd_lock_or_tempfail 参照）。
  ( cd "$workdir" && "$@" ) >"$log" 2>&1 8>&- 9>&- || rc=$?
  end="$(date +%s.%N)"
  secs="$(awk -v a="$start" -v b="$end" 'BEGIN { printf "%.3f", b - a }')"
  printf '%s\t%s\t%s\t%s\tfalse\t\n' "$name" "$rc" "$secs" ".gate-$name.log" >>"$GATE_TSV"
  if [ "$rc" -eq 0 ]; then
    sd_log "step $name: exit 0 in ${secs}s"
  else
    sd_log "step $name: exit $rc in ${secs}s — see $log"
    tail -n 30 "$log" >&2 || true
    GATE_OK=false
    GATE_FAILED_STEP="$name"
  fi
}

# `skip_step <name> <reason>` — 段を回さずに gate.json に `skipped: true` と理由を残す（Phase SD-1）。
# gate が既に落ちていれば何もしない（run_step と同じ）。
skip_step() {
  local name="$1" reason="$2"
  if [ "$GATE_OK" != true ]; then return 0; fi
  sd_log "step $name: skipped — $reason"
  printf '%s\t0\t0\t\ttrue\t%s\n' "$name" "$(printf '%s' "$reason" | tr '\t\n' '  ')" >>"$GATE_TSV"
}

# ---- web/ の段（web ADR-W3 D3、P6-02）: 非 blocking ---------------------------------
#
# gui/ の gate の**後ろ**で web/（ADR-0081 の SPA + gateway）の install / typecheck / test / release を回す。
# 落ちても `GATE_OK` は倒さない（リリースは作られ、昇格は gui/ だけのリリースとして進む）。結果は gate.json の
# `steps[]`（exit ≠ 0 のまま）と `web`（`ok` / `failed_step` / `blocking: false`）に残し、落ちた段より後ろの
# web/ の段は `skipped: true` にする。ビルドする sha に `web/` が無い・`SD_GATE_SKIP_WEB=1` なら全部 skipped。
# 2026-10-02（task 01M3YT4PT3）: node_modules が入らない不具合そのものは bundle_web（web-bundle 段。node_modules の
# 有無と `node -e 'import.meta.resolve(...); await import("./server/app.js")'` による import 解決）で直り、
# node_modules が無い・import できないリリースは web.ok=false になって web-follow が切り替えない（ADR-0135）。
# ただし NFS 上での web/app の展開（offline の prod install 含む）に 40〜60 分かかる問題は未対応で残っている。
# それを解決するまで既定を skip にするかは人の判断なので、既定は 1（skip）のまま変えない。web の段を走らせるとき
# は SD_GATE_SKIP_WEB=0 を明示する。
WEB_OK=true
WEB_FAILED_STEP=""
WEB_SKIP_REASON=""
WEB_STEPS=""
WEB_PNPM_VERSION=""
WEB_BUNDLE_JSON="null"

# `web_step <name> <workdir> -- <cmd...>` — run_step と同じ記録だが、失敗しても gate を倒さない。
web_step() {
  local name="$1" workdir="$2"
  shift 2
  [ "$1" = "--" ] && shift
  local log="$BUILD/.gate-$name.log" start end secs rc
  if [ "$GATE_OK" != true ]; then return 0; fi
  WEB_STEPS="$WEB_STEPS $name"
  if [ -n "$WEB_SKIP_REASON" ]; then
    skip_step "$name" "$WEB_SKIP_REASON"
    return 0
  fi
  if [ "$WEB_OK" != true ]; then
    skip_step "$name" "web stage failed at $WEB_FAILED_STEP (non-blocking)"
    return 0
  fi
  sd_log "step $name (non-blocking): $* (cwd $workdir)"
  sd_scratch_touch
  start="$(date +%s.%N)"
  rc=0
  ( cd "$workdir" && "$@" ) >"$log" 2>&1 8>&- 9>&- || rc=$?
  end="$(date +%s.%N)"
  secs="$(awk -v a="$start" -v b="$end" 'BEGIN { printf "%.3f", b - a }')"
  printf '%s\t%s\t%s\t%s\tfalse\t\n' "$name" "$rc" "$secs" ".gate-$name.log" >>"$GATE_TSV"
  if [ "$rc" -eq 0 ]; then
    sd_log "step $name: exit 0 in ${secs}s"
  else
    sd_log "step $name: exit $rc in ${secs}s — see $log (non-blocking: the gui release still proceeds)"
    tail -n 30 "$log" >&2 || true
    WEB_OK=false
    WEB_FAILED_STEP="$name"
  fi
}

# web/ の pnpm は `corepack pnpm@<web/package.json の packageManager の版>` に固定する（host の pnpm と gui/ の版に依存しない）。
# `SD_WEB_PNPM` で丸ごと差し替えられる（テストの偽 corepack もこれで入る）。
web_pnpm() {
  if [ -n "${SD_WEB_PNPM:-}" ]; then
    # shellcheck disable=SC2086
    $SD_WEB_PNPM "$@"
  else
    command -v corepack >/dev/null 2>&1 || { echo "corepack not found in PATH (web stage needs corepack pnpm@$WEB_PNPM_VERSION)" >&2; return 127; }
    corepack "pnpm@$WEB_PNPM_VERSION" "$@"
  fi
}

# `pnpm -C web release`（P6-01）: 配布物の名前にビルドする sha12 を入れる（`web/release/celeris-web-<ver>-<sha12>.tar.gz`）。
web_pnpm_release() {
  CELERIS_WEB_RELEASE="$SHA12" web_pnpm release
}

decide_web_skip() {
  if [ "${SD_GATE_SKIP_WEB:-1}" = 1 ]; then
    WEB_SKIP_REASON="SD_GATE_SKIP_WEB=1"
  elif [ ! -f "$BUILD/web/package.json" ]; then
    WEB_SKIP_REASON="no web/ directory"
  else
    WEB_PNPM_VERSION="$(sd_json_get "$BUILD/web/package.json" packageManager 2>/dev/null | sed -n 's/^pnpm@//p')"
    [ -n "$WEB_PNPM_VERSION" ] || WEB_SKIP_REASON="web/package.json has no packageManager pnpm@<version>"
  fi
  [ -z "$WEB_SKIP_REASON" ] || sd_log "web steps: skipped — $WEB_SKIP_REASON"
}

web_json() {
  local steps_json="" n
  for n in $WEB_STEPS; do steps_json="$steps_json${steps_json:+, }$(sd_json_str "$n")"; done
  printf '{"ok": %s, "blocking": false, "failed_step": %s, "skipped": %s, "reason": %s, "pnpm": %s, "steps": [%s], "bundle": %s}' \
    "$WEB_OK" "$(sd_json_str "$WEB_FAILED_STEP")" "$([ -n "$WEB_SKIP_REASON" ] && echo true || echo false)" \
    "$(sd_json_str "$WEB_SKIP_REASON")" "$(sd_json_str "${WEB_PNPM_VERSION:+pnpm@$WEB_PNPM_VERSION}")" "$steps_json" "$WEB_BUNDLE_JSON"
}

# Phase 89（ADR-0041 追記）: `pnpm-mobile-audit` / `pnpm-e2e-mock` の前置きチェック。
# `pnpm install --frozen-lockfile`（`pnpm-install` ステップ）は `@playwright/test` という npm パッケージを
# node_modules に入れるだけで、Playwright の Chromium 実行ファイルそのもの（`pnpm exec playwright install
# chromium` で落とすもの）は入れない（このリポジトリに postinstall フックは無いことを確認済み）。
# Chromium はホストの `~/.cache/ms-playwright/` に**バージョンごとに 1 つ**入っていて、`pnpm-lock.yaml` が
# 固定されている限り worktree をいくつ切っても同じキャッシュを共有できる（Phase 89 で確認: このホストには
# 既に chromium-1243 系が入っていた）。無ければ `chromium.launch()` の呼び出しがエラーで落ちる
# （ハングはしないが、ステップの出力が Playwright のインストール手順の長い ASCII アートになって
# 「何が悪いか」が gate.json だけからは分かりにくい）。ここで先に軽く（ブラウザを起動せずパスの存在だけ）
# 確かめて、無ければ即座に分かりやすい 1 行で失敗させる。
sd_check_playwright_chromium() {
  node -e '
    const { chromium } = require("@playwright/test");
    const fs = require("fs");
    const p = chromium.executablePath();
    if (!fs.existsSync(p)) {
      console.error(
        "false — playwright browser not installed (missing " + p + "; " +
        "run: pnpm exec playwright install chromium on this host, cached under ~/.cache/ms-playwright)"
      );
      process.exit(1);
    }
  '
}

run_pnpm_mobile_audit() {
  sd_check_playwright_chromium || return 1
  MOBILE_AUDIT_SKIP_BUILD=1 timeout "${SD_AUDIT_TIMEOUT:-600}" pnpm mobile-audit
}

run_pnpm_e2e_mock() {
  sd_check_playwright_chromium || return 1
  E2E_SKIP_BUILD=1 timeout "${SD_AUDIT_TIMEOUT:-600}" pnpm e2e:mock
}

# `cargo test` のログから「走ったテストバイナリの数・合格・失敗・無視」を数える（Phase SD-1）。共有 target で
# 作り直さなかったバイナリも cargo は必ず走らせるので、数が前回と同じなら「全部走った」証拠になる。
# Phase SD-2: 並列の runner（test-parallel.sh）は最後に `CELERIS_TEST_SUMMARY {json}` を 1 行出すので、あればそれを
# そのまま使う（`binaries` / `passed` / `failed` / `ignored` に runner・nextest の版・並列数・内訳が付く）。無ければ従来の
# `cargo test` の出力（Running / Doc-tests / test result 行）を数える。
cargo_test_summary_json() {
  local log="$BUILD/.gate-cargo-test.log"
  { [ -f "$log" ] && [ "$SD_JSON_TOOL" = python3 ]; } || { printf 'null'; return 0; }
  python3 - "$log" <<'PY'
import json, re, sys
for line in reversed(open(sys.argv[1], encoding="utf-8", errors="replace").read().splitlines()):
    if line.startswith("CELERIS_TEST_SUMMARY "):
        try:
            print(json.dumps(json.loads(line.split(" ", 1)[1])))
            sys.exit(0)
        except ValueError:
            break
binaries = passed = failed = ignored = 0
with open(sys.argv[1], encoding="utf-8", errors="replace") as fh:
    for line in fh:
        s = line.strip()
        if s.startswith("Running ") or s.startswith("Doc-tests "):
            binaries += 1
        m = re.match(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", s)
        if m:
            passed += int(m.group(1))
            failed += int(m.group(2))
            ignored += int(m.group(3))
print(json.dumps({"runner": "cargo-test", "binaries": binaries, "passed": passed, "failed": failed, "ignored": ignored}))
PY
}

# gate.json を書く（成功でも失敗でも同じ形）。Phase SD-1: `steps[]` に `skipped` / `reason`、
# `build`（作業ツリー・共有 target の使い回し）、`gui_skip_base`（GUI の段を飛ばす判断に使った base）、
# `cargo_test`（上の集計）、`bundle`（梱包の所要）を足した（どれも読む側は無視してよい追加の欄）。
write_gate_json() {
  local dest="$1" steps
  steps="$(sd_tsv_to_json "$GATE_TSV")"
  {
    printf '{\n'
    printf '  "sha": %s,\n' "$(sd_json_str "$SHA_FULL")"
    printf '  "sha12": %s,\n' "$(sd_json_str "$SHA12")"
    printf '  "ref": %s,\n' "$(sd_json_str "$REF")"
    printf '  "at": %s,\n' "$(sd_json_str "$(sd_ts)")"
    printf '  "ok": %s,\n' "$GATE_OK"
    printf '  "failed_step": %s,\n' "$(sd_json_str "$GATE_FAILED_STEP")"
    printf '  "build": {"tree": %s, "tree_reused": %s, "cargo_target": %s, "scratch_owner": %s, "workspace_clean": %s},\n' \
      "$(sd_json_str "$BUILD")" "$BUILD_TREE_REUSED" "$(sd_json_str "$SD_CARGO_TARGET")" \
      "$(sd_json_str "$SD_SCRATCH_OWNER")" "$WS_CLEAN_NEEDED"
    if [ -n "$GUI_SKIP_BASE" ]; then
      printf '  "gui_skip_base": %s,\n' "$(sd_json_str "$GUI_SKIP_BASE")"
    else
      printf '  "gui_skip_base": null,\n'
    fi
    printf '  "cargo_test": %s,\n' "$(cargo_test_summary_json)"
    printf '  "bundle": %s,\n' "${BUNDLE_JSON:-null}"
    printf '  "web": %s,\n' "$(web_json)"
    printf '  "steps": %s\n' "$steps"
    printf '}\n'
  } >"$dest"
}

# `current` の完全な sha（`current/manifest.json` の `sha`。無ければ sha12）。`current` が無ければ空。
# changes.json（ADR-0041 D4）と、GUI の段を飛ばす判断（Phase SD-1）の base。
base_ref_of_current() {
  local ref=""
  [ -n "$CUR" ] || return 0
  ref="$(sd_json_get "$(sd_release_dir "$CUR")/manifest.json" sha 2>/dev/null || true)"
  [ -n "$ref" ] || ref="$CUR"
  printf '%s' "$ref"
}

# ---- GUI の段を飛ばすか（Phase SD-1、ADR-0041 追記 §7） -----------------------
#
# `current`（gate を全段通って作られ、昇格されたリリース）から**この sha までに gui/ の下が 1 ファイルも
# 変わっていない**ときだけ、GUI の検査だけの段（pnpm-test・pnpm-mobile-audit・pnpm-e2e-mock）を飛ばす。
# これらの段の入力は gui/ の下だけ（偽の celeris も `gui/test/mock-celeris` / `gui/scripts/lib`）なので、
# 同じ gui/ で `current` の gate が通っていれば結果は同じ。base が分からない（`current` が無い・その sha を
# リポジトリが知らない・git diff が失敗した）ときは飛ばさない。`SD_GATE_FORCE_GUI=1` で常に回す。
GUI_SKIP_BASE=""
GUI_SKIP_REASON=""
decide_gui_skip() {
  local base diff_out
  [ "${SD_GATE_FORCE_GUI:-0}" = 1 ] && { sd_log "gui steps: SD_GATE_FORCE_GUI=1; running all"; return 0; }
  base="$(base_ref_of_current)"
  [ -n "$base" ] || { sd_log "gui steps: no \`current\` release; running all"; return 0; }
  git -C "$SD_REPO" rev-parse --verify --quiet "${base}^{commit}" >/dev/null \
    || { sd_log "gui steps: base $CUR is not in $SD_REPO; running all"; return 0; }
  diff_out="$(git -C "$SD_REPO" diff --name-only "$base" "$SHA_FULL" -- gui/ 2>/dev/null)" \
    || { sd_log "gui steps: git diff against $CUR failed; running all"; return 0; }
  if [ -n "$diff_out" ]; then
    sd_log "gui steps: $(printf '%s\n' "$diff_out" | wc -l | tr -d ' ') file(s) under gui/ changed since $CUR; running all"
    return 0
  fi
  GUI_SKIP_BASE="$CUR"
  GUI_SKIP_REASON="no change under gui/"
  sd_log "gui steps: no change under gui/ since current ($CUR); skipping pnpm-test, pnpm-mobile-audit, pnpm-e2e-mock"
}
decide_gui_skip

# `gui_step <name> <workdir> -- <cmd...>` — GUI の検査だけの段。飛ばす判断が立っていれば skip_step。
gui_step() {
  local name="$1"
  if [ -n "$GUI_SKIP_REASON" ]; then
    skip_step "$name" "$GUI_SKIP_REASON"
  else
    run_step "$@"
  fi
}

# ---- gate（D2 の順番どおり） ----------------------------------------------

# Relative dep-info and mtimes from another worktree can leave stale workspace
# metadata even after serialized builds. Keep third-party dependencies cached.
# Phase SD-1: 共有 target が固定の作業ツリー（`$BUILD`）から作られたもの（`$TARGET_TREE_MARK` が一致）なら
# 「別の worktree の dep-info・mtime」は無いので掃除しない（上の WS_CLEAN_NEEDED）。掃除できたら印を書く。
if [ "$WS_CLEAN_NEEDED" = true ]; then
run_step cargo-workspace-clean "$BUILD" -- python3 -c '
import json, subprocess
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version=1"]))
members = set(metadata["workspace_members"])
args = ["cargo", "clean"]
for package in metadata["packages"]:
    if package["id"] in members:
        args.extend(["-p", package["name"]])
if len(args) == 2:
    raise SystemExit("no workspace packages found; refusing an unrestricted clean")
subprocess.run(args, check=True)
'
  [ "$GATE_OK" = true ] && printf '%s' "$BUILD" >"$TARGET_TREE_MARK"
else
  skip_step cargo-workspace-clean "shared target was built from this build worktree ($BUILD); cargo rebuilds only changed crates"
fi
# Phase 116 追記: Celeris の reviewer は `cargo fmt --check` を見るので、main が未整形だと配送が全部落ちる。
run_step cargo-fmt-check "$BUILD" -- cargo fmt --all -- --check
# Phase SD-2（ADR-0041 §8、人の判断 2026-09-28「バイナリ並列で動かす」）: テストバイナリを並列に回す
# （`scripts/dev/test-parallel.sh` = `cargo nextest run --workspace` + `cargo test --doc --workspace`）。
# 直列が要るテストは**ビルドする sha の** `.config/nextest.toml` の test-group で縛る。
# cargo-nextest が無ければこの段は分かりやすく落ちる（入れ方は docs/ops/nextest.md）。
# `SD_GATE_TEST_RUNNER=cargo-test` で従来の `cargo test --workspace`（直列）に戻せる（非常用）。
run_cargo_test_step() {
  case "${SD_GATE_TEST_RUNNER:-nextest}" in
    nextest)
      # 無ければ bash が exit 127 で落ち、gate.json の cargo-test 段に残る（sd_die だと gate.json が残らない）。
      run_step cargo-test "$BUILD" -- bash "${SD_TEST_PARALLEL:-$BUILD/scripts/dev/test-parallel.sh}" ;;
    cargo-test) run_step cargo-test "$BUILD" -- cargo test --workspace ;;
    *) sd_die "SD_GATE_TEST_RUNNER must be nextest or cargo-test (got ${SD_GATE_TEST_RUNNER})" ;;
  esac
}
run_cargo_test_step
run_step cargo-clippy "$BUILD" -- cargo clippy --workspace -- -D warnings
# ADR-0083: 手書き production の肥大化・巨大 inline test・gitignore された未追跡 mod を毎回可視化する。
# 既定は warning のみ（exit 0）なので、この段が gate を落とすのは source-size-report.py 自体が
# 壊れたときだけ（`--strict` を渡していないので閾値超過そのものでは落ちない）。
run_step source-size-report "$BUILD" -- python3 scripts/dev/source-size-report.py
run_step cargo-build "$BUILD" -- cargo build --release -p celeris -p celerisctl -p celeris-credentiald
run_step pnpm-install "$BUILD/gui" -- pnpm install --frozen-lockfile
run_step pnpm-typecheck "$BUILD/gui" -- pnpm typecheck
gui_step pnpm-test "$BUILD/gui" -- pnpm test
run_step pnpm-build "$BUILD/gui" -- pnpm build
# Phase 89（ADR-0041 追記、ADR-0055 D3）: デザイン退行（画面は 200 を返すが壊れている）を積んだまま
# リリースを作らないための 2 つ。どちらも直前の `pnpm-build` の `$BUILD/gui/build` を使い回す。
gui_step pnpm-mobile-audit "$BUILD/gui" -- run_pnpm_mobile_audit
gui_step pnpm-e2e-mock "$BUILD/gui" -- run_pnpm_e2e_mock

# web ADR-W3 D3（P6-02）: web/ の段。gui/ の gate の後ろ、非 blocking（上の web_step）。
decide_web_skip
web_step web-pnpm-install "$BUILD/web" -- web_pnpm install --frozen-lockfile
web_step web-pnpm-typecheck "$BUILD/web" -- web_pnpm typecheck
web_step web-pnpm-test "$BUILD/web" -- web_pnpm test
web_step web-pnpm-release "$BUILD/web" -- web_pnpm_release

if [ "$GATE_OK" != true ]; then
  # 作業ツリーは次のリリースが作り直すので、記録は `.build/<sha12>/` に写して残す。
  mkdir -p "$FAILED_DIR"
  write_gate_json "$FAILED_DIR/gate.json"
  for log in "$BUILD"/.gate-*.log; do
    [ -f "$log" ] || continue
    cp -p "$log" "$FAILED_DIR/$(basename "$log")"
  done
  sd_log "gate failed at step '$GATE_FAILED_STEP'; no release directory created"
  sd_log "gate.json: $FAILED_DIR/gate.json (the .gate-*.log files are copied next to it)"
  exit 1
fi

# ---- リリースの組み立て ----------------------------------------------------

BUNDLE_T0="$(sd_now)"
STAGE="$REL.partial"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/gui"

for b in celeris celerisctl celeris-credentiald; do
  [ -x "$SD_CARGO_TARGET/release/$b" ] || sd_die "built binary missing: $SD_CARGO_TARGET/release/$b"
  cp -p "$SD_CARGO_TARGET/release/$b" "$STAGE/bin/$b"
done

[ -d "$BUILD/gui/build" ] || sd_die "gui build output missing: $BUILD/gui/build"
cp -r "$BUILD/gui/build" "$STAGE/gui/build"
for f in server.js package.json pnpm-lock.yaml pnpm-workspace.yaml; do
  [ -f "$BUILD/gui/$f" ] || sd_die "gui file missing: $BUILD/gui/$f"
  cp -p "$BUILD/gui/$f" "$STAGE/gui/$f"
done

# ---- gui の本番依存（Phase SD-1: lockfile ごとの cache への symlink） ----------
#
# 以前はリリースごとに `$STAGE/gui` で `pnpm install --prod --frozen-lockfile` していた（NFS 上に 4,000 余の
# ファイル・約 90 秒）。hardlink での複製も NFS では 1 ファイル数百 ms かかり速くならない（実測。phase-G.md SD-1）。
# そこで同じ入力（package.json / pnpm-lock.yaml / pnpm-workspace.yaml と node / pnpm の版）の install は
# `releases/.pnpm-prod-cache/<key>/` で**1 度だけ** `pnpm install --prod --frozen-lockfile` し、リリースの
# `gui/node_modules` はそこへの相対 symlink にする。cache の entry は作り終えてから rename で公開する
# （作りかけを指さない）。どのリリースからも指されなくなった entry は下の prune_releases が消す。
GUI_DEPS_KEY=""
GUI_DEPS_REUSED=false
GUI_DEPS_CREATED_BY=""
install_gui_prod_deps() {
  local gui="$1" key entry tmp f
  key="$(
    for f in package.json pnpm-lock.yaml pnpm-workspace.yaml; do
      printf '%s ' "$f"
      sha256sum <"$gui/$f" | cut -d' ' -f1
    done
    printf 'node %s\n' "$(node --version 2>/dev/null || echo none)"
    printf 'pnpm %s\n' "$(cd "$gui" && pnpm --version 2>/dev/null || echo none)"
  )"
  key="$(printf '%s' "$key" | sha256sum | cut -c1-32)"
  entry="$SD_PNPM_PROD_CACHE/$key"
  mkdir -p "$SD_PNPM_PROD_CACHE"
  if [ -d "$entry/node_modules" ] && [ -f "$entry/provenance.json" ]; then
    GUI_DEPS_REUSED=true
    GUI_DEPS_CREATED_BY="$(sd_json_get "$entry/provenance.json" created_by_sha12 2>/dev/null || true)"
    sd_log "gui: prod node_modules cache hit $key (created by ${GUI_DEPS_CREATED_BY:-?})"
  else
    # 前の release.sh が作りかけで死んだ残骸（`.lock-release` を持っているのは自分だけなので、あれば残骸）。
    rm -rf "$SD_PNPM_PROD_CACHE"/.tmp-* 2>/dev/null || true
    tmp="$SD_PNPM_PROD_CACHE/.tmp-$key-$$"
    mkdir -p "$tmp"
    for f in package.json pnpm-lock.yaml pnpm-workspace.yaml; do cp -p "$gui/$f" "$tmp/$f"; done
    sd_log "gui: pnpm install --prod --frozen-lockfile in $tmp (cache miss $key)"
    ( cd "$tmp" && pnpm install --prod --frozen-lockfile ) >&2 8>&- 9>&- \
      || sd_die "pnpm install --prod failed in the gui prod-deps cache ($tmp)"
    [ -d "$tmp/node_modules" ] || sd_die "pnpm install --prod did not create $tmp/node_modules"
    {
      printf '{"key": %s, "created_at": %s, "created_by_sha12": %s, ' \
        "$(sd_json_str "$key")" "$(sd_json_str "$(sd_ts)")" "$(sd_json_str "$SHA12")"
      printf '"node": %s, "pnpm": %s, "command": "pnpm install --prod --frozen-lockfile"}\n' \
        "$(sd_json_str "$(node --version 2>/dev/null || echo none)")" \
        "$(sd_json_str "$(cd "$tmp" && pnpm --version 2>/dev/null || echo none)")"
    } >"$tmp/provenance.json"
    rm -rf "$entry"
    mv -T "$tmp" "$entry"
    GUI_DEPS_CREATED_BY="$SHA12"
  fi
  # 相対 symlink（`releases/` ごと動かしても切れない）。`$gui` は `releases/<sha12>.partial/gui`。
  ln -s "../../$(basename "$SD_PNPM_PROD_CACHE")/$key/node_modules" "$gui/node_modules"
  [ -d "$gui/node_modules/" ] || sd_die "gui/node_modules symlink does not resolve ($gui/node_modules)"
  GUI_DEPS_KEY="$key"
}
GUI_DEPS_T0="$(sd_now)"
install_gui_prod_deps "$STAGE/gui"
GUI_DEPS_SECS="$(sd_secs_since "$GUI_DEPS_T0")"
sd_log "gui: prod node_modules ready in ${GUI_DEPS_SECS}s (key $GUI_DEPS_KEY, reused=$GUI_DEPS_REUSED)"

# ---- web/ の配布物（web ADR-W3 D3）: web の段が通ったときだけ。失敗は非 blocking（manifest / gate.json の `web` に残す） ----
bundle_web() {
  local tgz dir name
  [ "$WEB_OK" = true ] && [ -z "$WEB_SKIP_REASON" ] || return 0
  tgz="$(ls -1 "$BUILD/web/release/celeris-web-"*"-$SHA12.tar.gz" 2>/dev/null | head -n 1)"
  [ -n "$tgz" ] && [ -f "$tgz" ] || { WEB_OK=false; WEB_FAILED_STEP="web-bundle"; sd_log "web-bundle: no tarball under $BUILD/web/release for $SHA12 (non-blocking)"; return 0; }
  name="$(basename "$tgz" .tar.gz)"
  mkdir -p "$STAGE/web"
  cp -p "$tgz" "$STAGE/web/$name.tar.gz"
  dir="$STAGE/web/app"
  rm -rf "$dir"
  mkdir -p "$dir"
  if ! tar -xzf "$tgz" -C "$dir" --strip-components=1 >"$BUILD/.gate-web-bundle.log" 2>&1; then
    WEB_OK=false; WEB_FAILED_STEP="web-bundle"; sd_log "web-bundle: tar failed (non-blocking); see $BUILD/.gate-web-bundle.log"; return 0
  fi
  if ! ( cd "$dir" && web_pnpm install --prod --offline --frozen-lockfile ) >>"$BUILD/.gate-web-bundle.log" 2>&1 8>&- 9>&-; then
    WEB_OK=false; WEB_FAILED_STEP="web-bundle"; sd_log "web-bundle: offline prod install failed (non-blocking); see $BUILD/.gate-web-bundle.log"; return 0
  fi
  # pnpm の終了コードだけでは実行可能な app を保証できない。server/index.js は
  # import 時に listen するため、同じ依存を読む app.js を import して確認する。
  if [ ! -d "$dir/node_modules/" ]; then
    printf '%s\n' 'web-bundle: offline prod install did not create a resolvable node_modules directory' >>"$BUILD/.gate-web-bundle.log"
    WEB_OK=false; WEB_FAILED_STEP="web-bundle"; sd_log "web-bundle: node_modules missing (non-blocking); see $BUILD/.gate-web-bundle.log"; return 0
  fi
  if ! ( cd "$dir" && node --input-type=module -e 'import.meta.resolve("express"); await import("./server/app.js")' ) >>"$BUILD/.gate-web-bundle.log" 2>&1; then
    printf '%s\n' 'web-bundle: server dependencies could not be imported without listening' >>"$BUILD/.gate-web-bundle.log"
    WEB_OK=false; WEB_FAILED_STEP="web-bundle"; sd_log "web-bundle: server dependencies unresolved (non-blocking); see $BUILD/.gate-web-bundle.log"; return 0
  fi
  WEB_BUNDLE_JSON="{\"tarball\": $(sd_json_str "web/$name.tar.gz"), \"app\": \"web/app\"}"
  sd_log "web: bundle ready ($STAGE/web/$name.tar.gz, app at web/app)"
}
WEB_BUNDLE_T0="$(sd_now)"
bundle_web
WEB_BUNDLE_SECS="$(sd_secs_since "$WEB_BUNDLE_T0")"

SCHEMA_VERSION="$(sd_schema_version_of_tree "$BUILD")" \
  || sd_die "cannot parse SCHEMA_VERSION from crates/task-core/src/store/migrations.rs (or store/mod.rs, store.rs) at $SHA12"
# `celeris` の版は Cargo.toml から読む（バイナリを起こさない。`--version` は無い）。
CELERIS_VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*$/\1/p' "$BUILD/crates/celeris/Cargo.toml" | head -n 1)"
if [ -z "$CELERIS_VERSION" ]; then
  CELERIS_VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/ s/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*$/\1/p' "$BUILD/Cargo.toml" | head -n 1)"
fi
GUI_VERSION="$(sd_json_get "$STAGE/gui/package.json" version || true)"

{
  printf '{\n'
  printf '  "sha": %s,\n' "$(sd_json_str "$SHA_FULL")"
  printf '  "sha12": %s,\n' "$(sd_json_str "$SHA12")"
  printf '  "ref": %s,\n' "$(sd_json_str "$REF")"
  printf '  "built_at": %s,\n' "$(sd_json_str "$(sd_ts)")"
  printf '  "built_by": %s,\n' "$(sd_json_str "${USER:-unknown}@$(hostname)")"
  printf '  "profile": "release",\n'
  printf '  "schema_version": %s,\n' "$SCHEMA_VERSION"
  printf '  "celeris_version": %s,\n' "$(sd_json_str "$CELERIS_VERSION")"
  printf '  "gui_version": %s,\n' "$(sd_json_str "$GUI_VERSION")"
  printf '  "gui_prod_deps": {"cache_key": %s, "reused": %s, "created_by_sha12": %s},\n' \
    "$(sd_json_str "$GUI_DEPS_KEY")" "$GUI_DEPS_REUSED" "$(sd_json_str "$GUI_DEPS_CREATED_BY")"
  printf '  "web": %s,\n' "$(web_json)"
  printf '  "gate_ok": true\n'
  printf '}\n'
} >"$STAGE/manifest.json"

# ---- changes.json（ADR-0041 D4）-------------------------------------------
#
# 「いま動いている版（`current`）からこのリリースへ、何が変わるか」を**ビルド時に**書き留める。
# 昇格の直前に人が見るもので、GUI は `GET /releases` 越しにこれを読むだけ（判断はしない）。
# `base` はビルド時の `current` の sha12（無ければ `null` で、`commits` / `files` は空）。
# `sensitive` は `lib.sh` の `SD_SENSITIVE_PATTERNS` に**前方一致**した `files`（唯一の定義）。

write_changes_json() {
  local dest="$1"
  local base_sha12="" base_ref=""
  local commits_tsv files_txt sens_txt sha subject path
  base_sha12="$CUR"
  # git に渡すのは完全な sha の方が確実（`current/manifest.json` に入っている）。
  base_ref="$(base_ref_of_current)"

  commits_tsv="$(mktemp)"
  files_txt="$(mktemp)"
  sens_txt="$(mktemp)"
  printf 'sha:s subject:s\n' >"$commits_tsv"

  if [ -n "$base_ref" ] && git -C "$SD_REPO" rev-parse --verify --quiet "${base_ref}^{commit}" >/dev/null; then
    # 新しい順、最大 50 件（ADR-0041 D4）。subject の中のタブは潰す（TSV なので）。
    git -C "$SD_REPO" log --format='%H%x09%s' -n 50 "${base_ref}..${SHA_FULL}" 2>/dev/null \
      | while IFS=$'\t' read -r sha subject; do
        [ -n "$sha" ] || continue
        printf '%s\t%s\n' "$sha" "$(printf '%s' "$subject" | tr '\t' ' ')"
      done >>"$commits_tsv"
    git -C "$SD_REPO" diff --name-only "${base_ref}" "${SHA_FULL}" 2>/dev/null >"$files_txt" || true
  elif [ -n "$base_sha12" ]; then
    sd_log "changes.json: base $base_sha12 is not in $SD_REPO; commits/files will be empty"
  fi

  while IFS= read -r path; do
    [ -n "$path" ] || continue
    if sd_is_sensitive "$path"; then printf '%s\n' "$path"; fi
  done <"$files_txt" >"$sens_txt"

  {
    printf '{\n'
    if [ -n "$base_sha12" ]; then
      printf '  "base": %s,\n' "$(sd_json_str "$base_sha12")"
    else
      printf '  "base": null,\n'
    fi
    printf '  "at": %s,\n' "$(sd_json_str "$(sd_ts)")"
    printf '  "commits": %s,\n' "$(sd_tsv_to_json "$commits_tsv")"
    printf '  "files": %s,\n' "$(sd_lines_to_json_array "$files_txt")"
    printf '  "sensitive": %s\n' "$(sd_lines_to_json_array "$sens_txt")"
    printf '}\n'
  } >"$dest"

  sd_log "changes.json: base=${base_sha12:-<none>} commits=$(($(wc -l <"$commits_tsv") - 1)) files=$(wc -l <"$files_txt" | tr -d ' ') sensitive=$(wc -l <"$sens_txt" | tr -d ' ')"
  rm -f "$commits_tsv" "$files_txt" "$sens_txt"
}

write_changes_json "$STAGE/changes.json"

# ADR-0040 D6（Phase 48）: selfdeploy の道具一式をリリースに同梱する。
# `POST /releases/{sha12}/promote` は **リリースの中の** `scripts/promote.sh` を起こすので、
# 作業チェックアウトが無くても（別のブランチにいても）昇格できる。実行ビットは `cp -p` で保つ。
# `lib.sh` は `dirname "${BASH_SOURCE[0]}"` から自分の隣を読むだけなので、ここから source しても動く
# （場所は全部 `CELERIS_CONFIG_DIR` / `CELERIS_STATE_DIR` 基準。`SD_REPO` を使うのは `release.sh` だけ）。
mkdir -p "$STAGE/scripts"
for sh in "$BUILD"/scripts/selfdeploy/*.sh; do
  [ -f "$sh" ] || continue
  cp -p "$sh" "$STAGE/scripts/$(basename "$sh")"
done
[ -x "$STAGE/scripts/promote.sh" ] || sd_die "bundled scripts/promote.sh is missing or not executable"
[ -f "$STAGE/scripts/lib.sh" ] || sd_die "bundled scripts/lib.sh is missing"

BUNDLE_JSON="{\"secs\": $(sd_secs_since "$BUNDLE_T0"), \"gui_prod_deps_secs\": $GUI_DEPS_SECS, \"gui_prod_deps_reused\": $GUI_DEPS_REUSED, \"web_bundle_secs\": $WEB_BUNDLE_SECS, \"release_total_secs\": $(sd_secs_since "$RELEASE_T0")}"
write_gate_json "$STAGE/gate.json"

# ADR 2026-10-04-release-notes: リリースの説明（notes.json / notes.md）。gate.json の飛ばした段を拾うので
# gate.json の後に作る。**失敗は警告だけ**（説明が無くてもリリースは作る）。
# SD_RELEASE_NOTES_BIN: 呼ぶ celerisctl（既定は今作った $STAGE/bin/celerisctl。試験が差し替える）。
# SD_RELEASE_NOTES_API: task の題と配送記録を引く API（既定は $SD_PROD_API。届かなくても警告も出さず続ける）。
NOTES_BIN="${SD_RELEASE_NOTES_BIN:-$STAGE/bin/celerisctl}"
NOTES_SCHEMA_FROM=""
if [ -n "$CUR" ]; then
  NOTES_SCHEMA_FROM="$(sd_json_get "$(sd_release_dir "$CUR")/manifest.json" schema_version 2>/dev/null || true)"
fi
set -- release notes --repo "$SD_REPO" --sha "$SHA_FULL" --base "$(base_ref_of_current)" \
  --schema-to "$SCHEMA_VERSION" --gate-json "$STAGE/gate.json" \
  --api "${SD_RELEASE_NOTES_API:-$SD_PROD_API}" --token-file "$SD_API_TOKEN_FILE" --out-dir "$STAGE"
if [ -n "$NOTES_SCHEMA_FROM" ]; then set -- "$@" --schema-from "$NOTES_SCHEMA_FROM"; fi
if [ -x "$NOTES_BIN" ] && "$NOTES_BIN" "$@" >&2; then
  sd_log "release notes written: $STAGE/notes.json"
else
  sd_log "warning: could not write release notes with $NOTES_BIN (continuing without notes.json)"
fi

# gate のログも残す（失敗の再現に要る）。
mkdir -p "$STAGE/gate-logs"
for log in "$BUILD"/.gate-*.log; do
  [ -f "$log" ] || continue
  cp -p "$log" "$STAGE/gate-logs/$(basename "$log" | sed 's/^\.gate-//')"
done

rm -rf "$REL"
mv -T "$STAGE" "$REL"
sd_log "release ready: $REL"

# ---- ビルド用 worktree は消さない（Phase SD-1: 次のリリースが checkout し直して使い回す） ----

# ---- 掃除（current / previous / 検証済みの新しい 3 件を残す） --------------

prune_releases() {
  local keep_file dir sha
  keep_file="$(mktemp)"
  [ -n "$CUR" ] && printf '%s\n' "$CUR" >>"$keep_file"
  [ -n "$PREV" ] && printf '%s\n' "$PREV" >>"$keep_file"
  printf '%s\n' "$SHA12" >>"$keep_file"
  # 検証済み（verify.json.ok == true）の新しい 3 件
  for dir in "$SD_RELEASES"/*/; do
    [ -d "$dir" ] || continue
    sha="$(basename "$dir")"
    case "$sha" in .*) continue ;; esac
    if [ "$(sd_json_get "$dir/verify.json" ok 2>/dev/null || echo false)" = true ]; then
      printf '%s\t%s\n' "$(sd_json_get "$dir/manifest.json" built_at 2>/dev/null || echo 0000)" "$sha"
    fi
  done | sort -r | head -n 3 | cut -f2 >>"$keep_file"

  for dir in "$SD_RELEASES"/*/; do
    [ -d "$dir" ] || continue
    sha="$(basename "$dir")"
    case "$sha" in .*) continue ;; esac
    if grep -qxF "$sha" "$keep_file"; then continue; fi
    sd_log "pruning release $sha"
    # NFS: 実行中のバイナリ（drain 中の旧 celeris 等）は `.nfsXXXX` として残り rm が EBUSY で落ちる。リリースは
    # もうできているので、ここで release.sh を失敗させない（次の掃除で消える）。
    rm -rf "$dir" 2>/dev/null || sd_log "warning: could not fully remove $dir (a file is still in use, e.g. .nfs*); will retry next time"
  done
  rm -f "$keep_file"
  prune_gui_prod_deps
}

# Phase SD-1: どの（残った）リリースの `gui/node_modules` からも指されていない `.pnpm-prod-cache/<key>` を消す。
# いま作ったリリースが指す entry は必ず残る。symlink でない（Phase SD-1 より前の）node_modules は関係しない。
prune_gui_prod_deps() {
  local used entry key dir link
  [ -d "$SD_PNPM_PROD_CACHE" ] || return 0
  used="$(mktemp)"
  printf '%s\n' "$GUI_DEPS_KEY" >>"$used"
  for dir in "$SD_RELEASES"/*/; do
    link="${dir}gui/node_modules"
    [ -L "$link" ] || continue
    basename "$(dirname "$(readlink "$link")")" >>"$used"
  done
  for entry in "$SD_PNPM_PROD_CACHE"/*/; do
    [ -d "$entry" ] || continue
    key="$(basename "$entry")"
    if grep -qxF "$key" "$used"; then continue; fi
    sd_log "pruning gui prod-deps cache $key (no release points at it)"
    rm -rf "$entry" 2>/dev/null || sd_log "warning: could not fully remove $entry; will retry next time"
  done
  rm -f "$used"
}
PRUNE_T0="$(sd_now)"
# `SD_RELEASE_PRUNE=0` で掃除を飛ばす（計測・試験のためのリリースで、他人の検証済みリリースを押し出さないため。Phase SD-1）。
if [ "${SD_RELEASE_PRUNE:-1}" = 0 ]; then
  sd_log "prune: skipped (SD_RELEASE_PRUNE=0)"
else
  prune_releases
fi
sd_log "prune: done in $(sd_secs_since "$PRUNE_T0")s"

sd_log "done: sha12=$SHA12 schema_version=$SCHEMA_VERSION"
printf '%s\n' "$SHA12"
