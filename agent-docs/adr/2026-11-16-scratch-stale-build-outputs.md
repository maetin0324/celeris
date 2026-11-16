# scratch の target の引き継ぎと seed の写しの後に、元の場所を指す build 出力を消す

- 日付: 2026-11-16
- 状態: Accepted（実装済み）
- 関連: [ADR-0075](0075-tiered-build-cache.md) D3（adopt）、[ADR-0129](0129-host-sccache-reflink-targets.md) (4)（seed の reflink）、[ADR 2026-11-11 scratch-lock-tick-stall](2026-11-11-scratch-lock-tick-stall.md)
- task: 01M7H4K519QZGY2875BHNFTMKX（健全性v2 V0〈01M6TYVJT97XH88TEEAVRD7JK6〉の提案 1）

## 背景

健全性v2 V0 の段 verify の統合が `could not find native static library rusty_v8`（exit 101）で落ちた（[V0 target-env](../progress/2026-11-15-health-v2-v0/target-env.md)）。G4 の [fix-v8-env](../progress/2026-11-10-health-v2-g4/fix-v8-env.md) も同じ欠陥で、そちらは seed の世代が消えた場合だった。

cargo は target を別の場所へ動かした後、build script の出力 `<profile>/build/<pkg>-<hash>/output` を読むとき、`root-output` に記録した前の OUT_DIR の文字列だけを今の OUT_DIR に置き換える。v8 crate の build script は static lib を OUT_DIR の外（`<target>/debug/gn_out/obj`）に置き、`cargo:rustc-link-search=<target>/debug/gn_out/obj` を出すので、この値は動かす前の path のまま残る。build script の入力は変わらないので cargo は build script を再実行しない。v8 の lib を再コンパイルすると、残った path に dir が無ければ link が落ちる。

target の位置が変わる経路は 2 つある。

- 引き継ぎ（`scratch.rs` の `allocate_locked`）: 引き継ぎ元の owner の `target/` を `std::fs::rename` で動かす。元の path には何も残らない。
- seed の写し（`scratch/reflink.rs` の `seed_target`）: `seeds/<repo-key>/gen-*/target` を owner へ reflink で写す。元の世代は seed の更新で `.deleting-*` へ退避され、いずれ消える。

2026-11-16 にこの run の target（`origin: adopted`。引き継ぎ元は WorkUnit の target で、その target は seed から写したもの）を読むだけで確かめた。v8 の `output` の `rustc-link-search` は、引き継ぎ元の path でなく、seed の世代 `seeds/agent-platform-…/gen-de38efdf4699-…/target/debug/gn_out/obj` を指していた。引き継ぎが 2 回以上続くと、残った値は直前の引き継ぎ元でなく、もっと前の場所を指す。`target-origin.json` は 1 段しか記録しないので、直前の引き継ぎ元の path だけで判定すると、この値を見落とす。同じ `output` には v8 の archive の置き場 `/local/celeris/data/scratch/rusty-v8/…` も出るが、これは pool の root の下で `targets/`・`seeds/` の外にあり、消えない。

## 実装前 brief

1. 根本原因: target の位置を変える 2 つの経路（引き継ぎの rename、seed の写し）が、cargo の付け替えの対象外の build script の出力（OUT_DIR の外を指す `rustc-link-search`）を残す。症状は、後で元の場所が消えてから、その package を再コンパイルしたときの link の失敗として出る。
2. 責務 owner: target の位置を変える `task-worker::scratch` の `allocate_locked`（引き継ぎ）と `scratch::reflink::seed_target`（seed の写し）。消す処理は新しい `scratch::build_outputs`（両方が呼ぶ 1 つの関数）に置く。
3. 既存の再利用先: profile の dir の見つけ方（`workspace_targets::profile_dirs`。`.cargo-lock` を持つ `<target>/<profile>`・`<target>/<triple>/<profile>`。`scratch::build_outputs` へ移し、`workspace_targets`・`prune_workspace_artifacts`〈独自の走査は `.fingerprint` を持つ dir〉・新しい処理の 3 つが使う。`workspace_targets` は前から `scratch` に依存するので、移せば `scratch` から `workspace_targets` への逆向きの依存を足さずに済む）、`<name>-<hash>` の分け方（`unit_hash`）、`remove_any`、記録の `target-origin.json`（`write_target_origin`）、GC の削除待ち（`targets/.deleting-*` と削除スレッド）。試験は `scratch/tests.rs` の seed の偽の写し（`SeedCopyOps` の差し替え）と adopt の候補の作り方。
4. 選択肢:
   - (a) 引き継ぎ・写しの直後に、元の場所を指す package の build の出力を消す。採用。target の側だけで閉じ、元の場所が消える前でも後でも効く。
   - (b) seed の古い世代や引き継ぎ元を、それを指す target が残る間は消さない。引き継ぎの rename では元の path が必ず無くなるので直せない。採らない。
   - (c) 残った path を新しい path へ書き換える。`gn_out` の中身が新しい target に在るかは package ごとに違い、在ると決めつけられない。cargo の記録を書き換えることにもなる。採らない。
   - 判定の範囲: 直前の引き継ぎ元・seed の世代の path だけを見る案は、上の背景の連鎖（引き継いだ target が seed の世代を指す）を見落とす。pool の root 全体を見る案は、v8 の archive の置き場（`rusty-v8/`）のような消えない共有の dir まで消す。`targets/`・`seeds/` の下（GC が消す場所）を見る案を採る。
5. 新しい状態・分岐・fallback・port: `target-origin.json` の欄 `removed_build_outputs`（引き継ぎ・写しのときだけ書く）と、`TargetOrigin::Adopted`・`TargetOrigin::Seed` の `removed`。引き継ぎの後に消す処理が I/O で落ちたら、その target を `targets/.deleting-*` へ動かして引き継ぎをやめる（下の D4）。新しい設定の欄・flag・port は無し。
6. 撤去対象: `prune_workspace_artifacts` の中の profile の走査と `workspace_targets` の `profile_dirs`・`CARGO_LOCK`（`scratch::build_outputs` の 1 つにまとめる。cargo は build する profile の dir に必ず `.cargo-lock` を置く）。消す仕組みは無い。手で `cargo clean -p v8` する回避は、この変更の後の新しい引き継ぎ・写しでは要らなくなる（既存の target の扱いは進捗ファイルの「本番への移行」）。
7. 不変条件: 引き継ぎ・写しの後の target に、`targets/`・`seeds/` の下の自分以外の場所を指す `rustc-link-search` が残らない。`root-output` の下を指す package と、pool の外・`targets/`・`seeds/` の外を指す package の出力は残る。判定は dir が在るかに依らない。試験は `scratch/tests/stale_build_outputs.rs`（既存の試験は build script の出力を作らないので、この欠陥を検出できない）。

## 決定

### D1 いつ消すか
- 引き継ぎ: `allocate_locked` が引き継ぎ元の `target/` を rename した直後、pool の `.lock` と owner の `.alloc.lock` を持ったまま消す。この時点で target を使う run はまだ無い。
- seed の写し: `seed_target` が一時 dir（`.seed-copy-*`）へ写し、`prune_workspace_artifacts` を当てた後、`target/` へ rename する前に消す。
- 既にあった target（`TargetOrigin::Existing`）には当てない。WorkUnit が task の target を共有して build している最中に消しうるため。

### D2 どれを「元の場所を指す」とみなすか
`<target>` の下の profile の dir（`scratch::profile_dirs`。`.cargo-lock` を持つ `debug`、`<triple>/<profile>` など）ごとに、`build/<pkg>-<hash>/output` の各行のうち `cargo:rustc-link-search=` と `cargo::rustc-link-search=` の値を読む。値の前置き（`native=`・`dependency=`・`crate=`・`framework=`・`all=`）は外す。次の 3 つをすべて満たす値を「元の場所を指す」とみなす。
1. 同じ dir の `root-output` の path（`root-output` が無ければ `build/<pkg>-<hash>/out`）とその下に無い。cargo が付け替えるので残す。
2. 引き継ぎ・写しの後の自分の `target/` とその下に無い。
3. pool の `targets/` か `seeds/` の下にある（設定の path と、その実体の path〈canonicalize〉の両方で比べる）。引き継ぎ元・seed の世代はどちらかの下にあり、GC がいずれ消す。

dir が今在るかは見ない（引き継ぎ元は rename の後に必ず無く、seed の世代は在ってもいずれ消える）。path は component ごとに比べる（`a/b` は `a/bc` の下ではない）。読めない `output` は落とさず読み飛ばす（`root-output` だけが読めないときは `out` を使う）。

### D3 何を消すか
D2 に当たる値が 1 つでもあった package（`build/<pkg>-<hash>` の `<pkg>`）について、その profile の中で次を消す。次の build で cargo は build script を再実行し、package とその依存元を作り直す（`cargo clean -p <pkg>` をその profile に当てたのと同じ効果）。
- `build/<pkg>-*`（build script の build と実行の両方）
- `.fingerprint/<pkg>-*`
- `deps/` の中で、上の 2 つの `<hash>` を持つ file（`lib<crate>-<hash>.rlib`・`.rmeta`・`<crate>-<hash>.d` など）

`<pkg>-<hash>` は最後の `-` で分け、package の名前は完全に一致するものだけを消す（`v8` を消しても `v8-sys` は残る）。別の profile の同じ package は、その profile で D2 に当たらなければ残す。

### D4 失敗したとき
- seed の写し: 消す処理の I/O の失敗は写しの失敗として扱う（一時 dir を消し、空の target から始める。`reason` は `seed copy failed: …`）。既存の seed の失敗の扱いと同じ。
- 引き継ぎ: 消す処理が I/O で落ちたら、引き継いだ target を `targets/.deleting-<owner>-<nanos>` へ rename し（GC の削除スレッドが消す）、引き継がなかったものとして seed の写し・空の target へ進む。消しかけの target は使わない。この rename も落ちたら、`allocate` は error を返す（呼び出し側は既存のとおり warn を出して target の path を使う）。

### D5 記録
消した package は `target-origin.json`（`<owner>/target-origin.json`。schema `celeris.scratch-target-origin/1` のまま欄を足す）の `removed_build_outputs` に残す。event は使わない（target は host のローカルの build cache で、task の状態ではない。引き継ぎ・写しの記録は既に `target-origin.json` にある）。

```json
"removed_build_outputs": [
  {"profile": "debug", "package": "v8",
   "link_search": ["/local/celeris/data/scratch/seeds/…/gen-…/target/debug/gn_out/obj"],
   "removed_entries": 10}
]
```

- 引き継ぎ（`origin: adopted`）と seed の写し（`origin: seed`）では必ず書く。消したものが無ければ `[]`。件数は配列の長さ（package の数）と、各要素の `removed_entries`（消した dir と file の数）。
- 空の target（`origin: empty`）には書かない。この変更の前に作られた記録にも無い。欄が無い `adopted`・`seed` の記録は、この確かめをしていない target を表す。
- 消したものがあれば、`scratch: removed build outputs that point at another place in the pool` を info で log に出す。

## 範囲外
- `rustc-link-search` 以外の build script の出力（`cargo:rustc-link-arg`・`cargo:rustc-flags` の `-L`・links の metadata〈例: aws-lc-rs の `cargo:include=`〉）は見ない。2026-11-16 の時点で link の失敗を起こしたのは `rustc-link-search` だけ。
- 既にある target（この変更の前に引き継いだ・写したもの）は直さない（D1）。扱いは task の進捗ファイルの「本番への移行」に書く。
