---
title: 健全性v2 R5 affected-features — test-affected の crate の組が変わっても共有 crate を作り直さない
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 affected-features

完了日: 2026-11-10

WorkUnit `affected-features`。基準は `$CELERIS_WU_BASE`（段 design の統合の後）。ADR 2026-11-10-test-target-bundles D6 の案 F1 を入れた。

## 結論
- `scripts/dev/test-affected.sh` の cargo の呼び方だけを変えた。`cargo nextest run -p a -p b …` を `cargo nextest run --workspace -E 'package(=a) | package(=b) …'` に、`cargo test --doc -p a -p b …` を `cargo test --doc --workspace` にした。build は release gate（`test-parallel.sh`）と同じ `--workspace` の features で行い、nextest で走らせる試験は今と同じ対象 crate に filter で絞る。
- 対象 crate の決め方（変更 file → crate → 逆依存閉包、workspace 共通 file で全 crate）と web・control-ts・ts-verify の判定は変えていない。`--list` の出力は base の script と 16 個の基準で同じだった。
- 確かめる台本 `rebuild-check.sh` を足した。組 A（workspace の全 15 crate。task-core を含む）→ 組 B（celeris,celerisctl）→ 組 A の順に `--no-run` を走らせる。直す前の呼び方（`-p`）では 3 回目に celeris・celerisctl の 2 crate を作り直し、直した後は 0 だった。
- crates/*/Cargo.toml は変えていない（案 F2 は ADR D6-5 で採らない）。

## 変更前後の Compiling 行の数
同じ target dir（WU の scratch）・同じ順で、直す前（`-p` の組）を先に、直した後（`rebuild-check.sh`）を続けて走らせた。どちらも 1 回目と 2 回目の間に `crates/celeris/src/lib.rs` の mtime を進めた（内容は変えない）。

| 回 | 組 | 直す前（`-p`）秒 | 直す前 Compiling（workspace の crate） | 直した後（`--workspace -E`）秒 | 直した後 Compiling（workspace の crate） |
|---|---|---|---|---|---|
| 1 | A（15 crate） | 172.35 | 16（15） | 2.87 | 0（0） |
| 2 | B（celeris,celerisctl） | 91.26 | 14（13） | 49.47 | 2（2: celeris・celerisctl） |
| 3 | A | 38.43 | 2（2: celeris・celerisctl） | 6.70 | 0（0） |

- 直す前の 1 回目は、scratch の target に無かった全 crate を build した（外部の crate は 1 つだけ）。2 回目は組 B の版として task-core から celeris までの 13 crate と外部の 1 crate を build した。3 回目は組 A の版の celeris・celerisctl が 2 回目の mtime の変化より古いので作り直した。
- 直した後の 1 回目は、直す前の 1 回目（`-p` で全 15 crate）と同じ版を使い 0 だった。`-p` で全 member を選ぶ組と `--workspace` は同じ版になる。2 回目は mtime を進めた celeris と逆依存の celerisctl だけ、3 回目は 0。
- 直す前の 3 回目の 2 crate・38.43 秒が、組が変わるたびの作り直しの分にあたる。直した後は組 B でも `--workspace` の版を使うので、組 B の版の build（直す前の 2 回目の 13 crate）も無くなる。
- 計測中の load average（1 分値）: 直す前の開始 17.38・終了 28.95、直した後の開始 25.49・終了 56.02。秒は負荷を含む。前後比較の秒は measure-build の unit が同じ方法で測る。

## doc test（ADR D6-3）
- `cargo test --doc --workspace` は 14 crate の doc test を走らせ、3 件が通り 2 件が ignore（task-core 3 件、celeris-script・task-worker に ignore 各 1 件）。ADR が概数とした 31 件は fence の数で、試験として走るのは 5 件だった。
- nextest の build の後で、Compiling 0・23 秒（rustdoc の compile を含む。load average 40〜56 の間）。
- ADR D6-3 は件数と秒を measure.sh の `edit_verify` の段に足すとしたが、measure.sh は変えず、この進捗に書いた。measure.sh の `edit_verify` はまだ `-p` の組で build する。HEAD をどの呼び方で測るかは measure-build の unit が決める（提案 1）。

## 対象 crate の決め方が base と同じこと
- `git diff $CELERIS_WU_BASE -- scripts/dev/test-affected.sh` の変更は、先頭の説明の 5 行と、`if [ -n "$crates" ]` の中の filter の組み立て・nextest・doc test の行だけ。対象 crate を決める python と web・control-ts の判定の行は変えていない。
- base の script（`git show $CELERIS_WU_BASE:scripts/dev/test-affected.sh`）と変更後の script の `--list` を、`$CELERIS_WU_BASE` と過去 120 commit から 8 個おきの 15 commit の計 16 個の基準で比べた。16 個とも出力が同じ（6 crate の組と 14 crate の組）。

## rebuild-check.sh
- 呼び方: `bash agent-docs/progress/2026-11-10-health-v2-r5/rebuild-check.sh`。exit 0 = 3 回目に作り直した crate が 0、1 = 1 以上、2 = build の失敗か呼び方の食い違い。
- test-affected.sh の filter の行と nextest の行が test-affected.sh に在ることを確かめ、その 2 行を `eval` で実行する（`--no-run` だけ足す）。呼び方の写しを台本に持たない。test-affected.sh の呼び方を変えたら、この台本は exit 2 で止まる。
- 1 回目と 2 回目の間に `crates/celeris/src/lib.rs` の mtime を進める。Objective の順（A → B → A）だけでは、直す前も組ごとの版が target dir に残るので 3 回目は 0 になり、直す前と後を区別できない（baseline.md の A→B→A→B の計測で 3 回目・4 回目が 0）。mtime を進めると、直す前の 3 回目は celeris・celerisctl を作り直す（上の表）。
- log は `$TMPDIR` の使い捨ての dir に置き、作業ツリーには書かない。

## 証拠コマンド（提出時の HEAD で実行）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/rebuild-check.sh` → exit 0、59 秒、Compiling は 0・2・0、`3 回目に作り直した workspace の crate: 0（Compiling 行の合計 0）`
- 直す前: `rebuild-check.sh` の写しの `nextest_no_run` を `cargo nextest run -p <crate>… --no-run` に替え、呼び方の確認を外したもの（`$TMPDIR` に置き、commit していない）→ exit 1、302 秒、Compiling は 16・14・2
- `--list` の比較（上の節）→ 16 個の基準で差 0
- `bash scripts/dev/test-affected.sh` → exit 0、1028 秒。対象は celeris・celeris-campaign・celeris-script・celerisctl・llm-proxy・task-dispatch（test-affected.sh の変更で celeris-script が入る）。nextest は `3792 tests run: 3792 passed (31 slow), 11 skipped`（573 秒）、nextest の build の Compiling 0、doc test は 14 crate で 3 passed・2 ignored、`script bundle SKIP: 0 件`。test-ts-verify.sh・test-runner-tmp-lifecycle.sh・test-runner-tmp-repro.sh も走って通った（偽 cargo は `nextest run`・`test --doc` の後の引数を見ない）
- `cargo test --doc --workspace` → exit 0、23 秒、Compiling 0
- `cargo clippy --workspace -- -D warnings` → exit 0、66 秒
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`、`slices-check.py --write-set R5`、`sh scripts/dev/progress-index.sh --check`、`sh scripts/dev/check-doc-links.sh` → 結果は WU の報告に書く

## commit
- R5 affected-features: test-affected の build を --workspace の features にし、作り直しを確かめる台本を足す

## 未解決事項
- scratch の target の seed（reflink）がどの版を持つかは未確認（baseline.md と同じ）。直した後は `--workspace` の版だけを使うので、seed が `--workspace` の版ならそのまま使える。
- 対象外の crate の test binary も build・link する費用（ADR D6-4）は、この run では target に既に版があったので測っていない。measure-build の unit が base と HEAD を同じ run で測る。

## 提案
1. measure-build の unit は、HEAD の `edit_verify` を test-affected.sh と同じ呼び方（`--workspace -E`）で測り、doc test の件数と秒を足す。base は base の test-affected.sh の呼び方（`-p`）で測る。
2. stable の cargo が `resolver.feature-unification = "workspace"` を読むようになったら、ADR D6-6 のとおり filter を `-p` に戻し `CARGO_RESOLVER_FEATURE_UNIFICATION=workspace` に替える。そのとき rebuild-check.sh の呼び方の確認の 2 行も替える。
