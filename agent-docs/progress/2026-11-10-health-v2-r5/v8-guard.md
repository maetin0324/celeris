---
title: R5 v8-guard — 退避済みの seed 世代を指す v8 の link-search の検出
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
---
# R5 v8-guard — 退避済みの seed 世代を指す v8 の link-search の検出

完了日: 2026-11-11

結論: 計画 v5 の integrate-final で落ちた 1 本は、R5 の変更でなく scratch の target の環境（退避済みの seed 世代を指す v8 の link-search）が原因だった。
読むだけの検出台本 `v8-target-check.sh` を足し、この WU の target で段 final の check 1 が通ることを確かめた。
直す owner は `crates/task-worker/src/scratch` で、R5 の write set の外なので、直し方は「提案」に書いた。

## 実装前 brief
1. 根本原因: v8 の build script は `cargo:rustc-link-search=` に `<target>/<profile>/gn_out/obj` を絶対 path で書く。scratch は seed 世代の target を owner の target へ写すが、build script の output は書き換えないので、写した target も seed 世代の path を指す。seed の更新で古い世代が退避されると、features の違う cargo 呼び出しが v8 の lib を作り直す時に `librusty_v8.a` が見つからない。症状は段の統合の check（`cargo nextest run -p task-dispatch`）に出た。
2. 責務 owner: `crates/task-worker/src/scratch`（`reflink.rs` の `seed_target` が seed を写し、`seed.rs` の GC が current 以外の世代を消す）。R5 の write set の外。
3. 既存の再利用先: 段 final の check 1（pick-g4 と同じ awk と nextest）、計画が定めた check 2 の偽の target。
4. 選択肢: (a) scratch を直す → write set の外なので R5 ではしない。(b) check の前に `cargo clean -p v8` を必ず走らせる → 毎回 v8 を作り直し、原因を隠すので採らない。(c) 読むだけの台本で古い link-search を名指しして止める → 採用。check 1 の先頭に置けば、同じ原因で落ちた時に log が原因と直し方を示す。
5. 新しい状態・分岐・fallback・port: 無し（進捗の dir の検査台本 1 本だけ。product のコードと設定は変えない）。
6. 撤去対象: 無し。
7. 不変条件: 台本は target と seed を読むだけで、消さない・書かない。古い link-search を見つけたら exit 1、output が無い・全部の lib がある時は exit 0。check 2 の偽の target で確かめる。

## 落ちた check と log の行
- 計画 v5 の integrate-final の check は 14 本。1 本目（pick-g4 の check 1 の awk と `cargo nextest run -p task-dispatch -E 'test(/git_workspace::/)'`）だけが exit 101 で落ちた（17:08 開始、約 34 秒）。
- log（`/local/celeris/data/workspaces/01M6TYVJT53598CPKMGSQGA7XM/integration-checks/integrate-final/1794416889266-0.log`、35 行）の要点:
  - 26 行: `Compiling v8 v150.4.0`
  - 31 行: ``error: could not find native static library `rusty_v8`, perhaps an -L flag is missing?``
  - 33 行: ``error: could not compile `v8` (lib) due to 1 previous error``
  - 35 行: `` error: command `…/cargo '--color=auto' test --no-run --message-format json-render-diagnostics --package task-dispatch` exited with code 101 ``
- 統合の作業ツリーは `/local/celeris/data/workspaces/01M6TYVJT53598CPKMGSQGA7XM/repos/agent-platform`（log の `Compiling task-core` の path）。

## 同じ統合の test-affected の結果
- 同じ integrate-final の test-affected（log `1794416889266-7.log`、1002 行、17:32 終了）は exit 0。nextest `5952 tests run: 5952 passed (18 slow), 22 skipped`（491.3 秒）。親 task の計画 v6 の記録では所要 1375 秒。
- 同じ log の末尾で classify.py の verify-leaf（adjust-api・adjust-worker ともに 0 error）と cases.py（task-api 856 → 856、task-worker 1292 → 1292、removals 0 件）も OK。
- workspace 全体の build（test-affected）は通り、`-p task-dispatch` 単独の build だけが落ちた。v8 の lib の作り直しが features の組で決まるためと考える（features の組の違いで共有 crate が作り直される件は [affected-features.md](affected-features.md)）。

## 原因の根拠
- この WU の target の v8 の output（`$CARGO_TARGET_DIR/debug/build/v8-<hash>/output`）の 29 行は `cargo:rustc-link-search=/local/celeris/data/scratch/seeds/<repo key>/<世代 A>/target/debug/gn_out/obj`（`<repo key>` は `agent-platform-` に続く repo の key、`<世代 A>` は下の今の世代） だった。owner の target でなく seed 世代の path を絶対 path で指す。この WU の target は 17:41 にその世代から写された（owner の `target-origin.json` の `seed`）。
- `/local/celeris/data/scratch/seeds/<repo key>/` に残る世代は 1 つ（以下 世代 A。16:55 作成、`current` の指す先、seed の commit は 2026-11-11 時点の main）だけ。この task の他の WU の `target-origin.json` が写し元に挙げる 3 つの古い世代（2026-11-10 17:33〜2026-11-11 14:55 に写したもの）は残っていない。
- `crates/task-worker/src/scratch/seed.rs` の module の説明は「cargo が試験 binary や dep-info に焼く絶対 path が確定後も存在するよう、build 後に rename しない」と書き、seed の自分の path の安定は守っている。一方で GC は「current 以外の世代」を消すので、古い世代から写した owner の target が参照する path は消える。
- この WU の target では、同じ nextest が v8 の lib を作り直し（`Compiling v8 v150.4.0`）、今の世代の `gn_out/obj` の `librusty_v8.a` で link できた。指す世代が残っていれば通り、退避されていれば落ちる、という説明と合う。
- 未確認: 統合の target が写された世代の名前。統合の target は落ちた後（17:35:48）に今の世代から作り直され、落ちた時点の v8 の output は残っていない。

## 変更
- `agent-docs/progress/2026-11-10-health-v2-r5/v8-target-check.sh`（POSIX sh、dash で確認）を足した。引数は取らない（渡すと exit 2）。`$CARGO_TARGET_DIR`（未設定なら `target`）の `*/build/v8-*/output` の `cargo:rustc-link-search=` と `cargo::rustc-link-search=` を読み、`native=` などの種別の前置きを外す。1 つの output のどの dir にも `librusty_v8.a` が無ければ、output の path・指す dir・seed 世代が残っているか・写し元の seed・直し方 2 行（`cargo clean -p v8`、seed 世代の退避）を 1 行ずつ出して exit 1。output が無い・lib がある時は exit 0。読むだけで、消さない・書かない。
- pick-g4 の check 1 の先頭にこの台本を置くのは親 task の計画 v6 の変更（この WU は計画を変えていない）。
- この WU の target では古い path が出なかったので、`cargo clean -p v8` はしていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- check 2（偽の target）: exit 0。`native=<d>/ok` は `ok（… 1 件）` で exit 0、`<d>/gone` は `NG stale v8 link-search` と output・dir・直し方を出して exit 1。
- 追加の確認（dash）: `cargo::` 形式・`release` の profile・seed 世代の path は `seed 世代: …（退避済み）` と `cargo clean -p v8 --release` を出して exit 1。output の無い target は exit 0。引数を渡すと exit 2。偽の target の file は実行の前後で変わらない（`find -newer` で 0 件）。
- check 1（この WU の target）: `sh …/v8-target-check.sh` は exit 0（v8 の build output 1 件）。awk は exit 0。`cargo nextest run -p task-dispatch -E 'test(/git_workspace::/)'` は exit 0、`23 tests run: 23 passed, 1638 skipped`。build を含め 188 秒、load average 71 → 65（17:43〜17:47 UTC）。実行後も台本は exit 0。
- check 3（`slices-check.py --write-set R5`）: exit 0、`write-set R5: ok`。
- check 4（`check-doc-links.sh` と `progress-index.sh --check`）: exit 0。最初は seed 世代名と build dir 名の hex を commit hash と見なされて落ちたので、本文ではそれらを `<repo key>`・`<世代 A>`・`<hash>` と書いた。
- check 5（範囲）: 変更 path は `v8-guard.md` と `v8-target-check.sh` の 2 つで、範囲外は 0 件。
- commit: 「R5 v8-guard: 退避済みの seed 世代を指す v8 の link-search を検出する台本と進捗を足す」「R5 v8-guard: 進捗の世代名を hash に見えない書き方にし、検査の結果を書く」。

## 未解決事項
- scratch の owner の修正（下の提案）。R5 では直さない。
- 統合の target が写された世代の名前は未確認（上の「原因の根拠」）。

## 提案
scratch（`crates/task-worker/src/scratch`）の owner への提案。どちらか、または両方。
1. 写すときに書き換える: `reflink.rs` の `seed_target` が seed を owner の target へ写した後、`<profile>/build/*/output`（と `root-output`）に現れる seed の target の prefix を owner の target の path に書き換える。v8 以外にも絶対 path の link-search を出す build script（`OUT_DIR` の外を指すもの）があれば同じ扱いになる。書き換えた file は reflink の共有から外れるが、小さい text なので容量は増えない。
2. 生きている target が参照する世代を退避しない: `seed.rs` の GC は、lease のある owner の `target-origin.json` の `seed` が指す世代を消さない（owner が無くなってから消す）。
- 1 は原因（写しが seed の path を参照すること）を消す。2 は参照を残したまま世代を残すので、seed の容量が owner の寿命だけ増える。1 を推す。
- どちらを採っても、`v8-target-check.sh` の判定（古い link-search を exit 1 で名指し）を scratch の試験に移せば、R5 の進捗の台本は撤去できる。撤去は scratch を直す task が行う。
