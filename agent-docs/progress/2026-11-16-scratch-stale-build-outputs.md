---
title: scratch の target の引き継ぎと seed の写しの後に、元の場所を指す build 出力（v8 の gn_out の link-search）を消す
tasks: [01M7H4K519QZGY2875BHNFTMKX]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# scratch の target の引き継ぎと seed の写しの後に、元の場所を指す build 出力を消す

完了日: 2026-11-16

健全性v2 V0（01M6TYVJT97XH88TEEAVRD7JK6）の提案 1 の恒久の直し。決定は [ADR 2026-11-16-scratch-stale-build-outputs](../adr/2026-11-16-scratch-stale-build-outputs.md)。原因の記録は [V0 target-env](2026-11-15-health-v2-v0/target-env.md) と [G4 fix-v8-env](2026-11-10-health-v2-g4/fix-v8-env.md)。

## 結論
- 引き継ぎ（`allocate_locked` の rename の直後）と seed の写し（`seed_target` の rename の前）で、`<profile>/build/<pkg>-<hash>/output` の `rustc-link-search` を読む。`root-output` の下にも自分の `target/` の下にも無く、pool の `targets/` か `seeds/` の下を指す package の build の出力（`build/<pkg>-*`・`.fingerprint/<pkg>-*`・その hash の `deps/` の file）を消す。次の build で cargo は build script を再実行する（`cargo clean -p <pkg>` をその profile に当てたのと同じ効果）。
- 判定は path だけで行い、dir が在るかは見ない。範囲を直前の引き継ぎ元だけでなく `targets/`・`seeds/` 全体にしたのは、この run の target（引き継いだもの）の v8 の出力が、引き継ぎ元でなく前の seed の世代を指していたため（ADR の背景）。
- 消した package は `<owner>/target-origin.json` の `removed_build_outputs` に残す（`adopted`・`seed` では必ず書き、無ければ `[]`）。消したものがあれば `scratch: removed build outputs that point at another place in the pool` を info で log に出す。
- 引き継いだ target から消す処理が I/O で落ちたら、その target を `targets/.deleting-*`（GC の削除スレッドが消す）へ動かし、引き継がずに seed の写しか空の target へ進む。

## 変更（commit の件名）
- `ADR: scratch の引き継ぎ・seed の写しの後に元の場所を指す build 出力を消す` — 実装より先に ADR を置いた。
- `phase 1: scratch の引き継ぎ・seed の写しの後に元の場所を指す build 出力を消す` — `crates/task-worker/src/scratch/build_outputs.rs`（新規。`remove_stale_build_outputs`・`RemovedBuildOutput`）、`scratch.rs` の `allocate_locked`、`scratch/reflink.rs` の `seed_target`・`TargetOrigin`・`TargetOriginRecord`、試験 `crates/task-worker/src/scratch/tests/stale_build_outputs.rs`、`docs/architecture-map.md` の scratch の行、`docs/ops/local-disk-growth.md` の `target-origin.json` の説明。
- `phase 1: profile の dir の走査を workspace_targets::profile_dirs に一本化する` — repo-health の `dup.public-surface` が新しい `profile_dirs` と既存の `workspace_targets::profile_dirs` の重複を new で出したので、既存の方（`.cargo-lock` を持つ dir）を `build_outputs` と `prune_workspace_artifacts` の両方で使うようにした。この形では `scratch` から `workspace_targets` への依存が増え、repo-health の `deps.cycle` が worsened（task-worker の 6 module の循環）で落ちた。
- `phase 1: profile_dirs と CARGO_LOCK を scratch::build_outputs へ移し workspace_targets から使う` — 上の循環を直した。`workspace_targets` は前から `scratch` に依存するので、関数を `scratch` 側へ移して向きを揃えた。実装は 1 つ（`.cargo-lock` を持つ実 dir、`.deleting-*` と symlink は見ない）で、`workspace_targets`・`prune_workspace_artifacts`・新しい処理が使う。
- `phase 1: 進捗ファイルに検査の結果と本番への移行を書く` — この file。

## 試験（守る不変条件と、既存の試験で検出できない理由）
`crates/task-worker/src/scratch/tests/stale_build_outputs.rs`。一時の dir に cargo の配置を真似た偽の build 出力（`build/<pkg>-<hash>/{output,root-output,out/}`、`.fingerprint/`、`deps/`、`.cargo-lock`）を置く。本物の cargo の build と v8 の compile は走らせない。既存の adopt・seed の試験は build script の出力を作らないので、この欠陥を検出できない。分け方は `target-link-check.sh --self-test` の 3 例と同じ:
1. `aws-lc-sys`: `root-output` の下を指す → 残る。
2. `v8-sys`: `root-output` の外で、pool の root の下の archive の置き場（`rusty-v8/…`、`targets/`・`seeds/` の外）と pool の外の dir を指す（どちらも在る）→ 残る。名前が `v8-` で始まっても `v8` と一緒に消えない。
3. `v8`: `root-output` の外で元の場所の `debug/gn_out/obj` を指す → 消える。

- `adopt_removes_packages_that_point_at_the_adopted_from_target`: 引き継ぎ。元の path は rename で無くなる。3 例に加え、前の seed の世代を指す package（連鎖）も消える。`Allocation.origin` と `target-origin.json` の `removed_build_outputs` に `v8` と連鎖の package が `removed_entries: 8` で残る。
- `seed_copy_removes_packages_that_point_at_the_seed_generation`: seed の写し。元の場所（seed の世代の `gn_out/obj`）が在っても消える。seed の側は変わらない。記録に `v8` が残る。
- `stale_build_output_rule_details`: `cargo::` の書式と `dependency=`、`root-output` が無いときの `out`、component ごとの比較（`out` と `outx`）、自分の target の下、profile ごとの判定（`x86_64-unknown-linux-gnu/debug`）、消すものが無いときの `[]`。
- `adopt_discards_the_target_when_stale_outputs_cannot_be_removed`: ADR D4。`deps` を file にして消す処理を落とすと、target は `targets/.deleting-*` へ動き、`origin` は `empty`、lease の `adopted_from` は無い。

新しい処理を外すと落ちることの確認（提出時の HEAD の前、作業ツリーで一時的に書き換えて戻した）: `allocate_locked` の `remove_stale_build_outputs(pool, &target, &target)` を `Ok(Vec::new())` に、`seed_target` の `remove_stale_build_outputs(pool, &tmp, &target)?` を `Vec::new()` に替えて `cargo nextest run -p task-worker --no-fail-fast -E 'test(/stale_build_output|removes_packages_that_point|adopt_discards_the_target/)'` → `4 tests run: 1 passed, 3 failed`（adopt・seed・D4 の 3 件が `v8 must be removed` などで落ちる。関数を直接呼ぶ `stale_build_output_rule_details` だけ通る）。戻した後は 4 件とも通る。fixture を `.cargo-lock` に替えた後にもう一度行い、同じ結果だった。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
EVIDENCE_PLACEHOLDER

## 本番への移行
config・DB・API の変更は無い。新しい release に昇格した後の新しい引き継ぎ・seed の写しから効く。

### 既存の scratch の target（引き継ぎ済み・写し済みのもの）
- この変更は、既にある target（`TargetOrigin::Existing`）には当てない（WorkUnit が task の target を共有して build している最中に消しうるため。ADR D1）。この release より前に引き継いだ・写した target の `target-origin.json` には `removed_build_outputs` が無い。
- その target を別の owner がもう一度引き継ぐときには、新しい処理が当たる。判定は `targets/`・`seeds/` 全体なので、前の世代や前の引き継ぎ元を指す出力も消える。
- その target を今の owner が使い続ける間は、元の場所が消えた後に v8 を再コンパイルすると、従来どおり link が落ちうる。落ちたら（または事前に確かめるなら）、人か planner が次を実行する。消すのはその target の v8 の build の出力だけで、`targets/` の他の中身は消さない。

  ```sh
  T=/local/celeris/data/scratch/targets/<owner>/target    # WorkUnit は targets/task-<id>/wu-<id>/target
  sh agent-docs/progress/2026-11-15-health-v2-v0/target-link-check.sh "$T"
  # exit 1（MISSING あり）なら、その target を使う run が走っていないことを確かめてから:
  CARGO_TARGET_DIR="$T" cargo clean -p v8
  ```

- 記録の無い（この release より前の）引き継ぎ済み・写し済みの target を一覧にするには（読むだけ）:

  ```sh
  for f in /local/celeris/data/scratch/targets/*/target-origin.json /local/celeris/data/scratch/targets/*/*/target-origin.json; do
    [ -f "$f" ] || continue
    jq -r --arg f "$f" 'select((.origin == "adopted" or .origin == "seed") and (has("removed_build_outputs") | not)) | "\(.origin) \($f)"' "$f"
  done
  ```

  task の target は task が終わると GC が回収するので、手で消す必要は無い。

### 次の release の後に確かめる手順
1. 新しい release に昇格した後、新しく引き継いだか seed から写した target を持つ task（または WorkUnit）を 1 つ選ぶ。`cat /local/celeris/data/scratch/targets/<owner>/target-origin.json` で `origin` が `adopted` か `seed` で、`removed_build_outputs` の欄があることを確かめる。seed から写した target なら、seed の v8 は seed の世代を指すので、`v8` の要素が 1 つ以上あるはず（`profile: debug`、`link_search` が `seeds/…/gen-…/target/debug/gn_out/obj`）。
2. daemon の log で消したことを確かめる: `journalctl --user -u celeris@<sha12>.service --since '-1 h' --no-pager | grep 'removed build outputs that point at another place in the pool'`。
3. その task の target に `sh agent-docs/progress/2026-11-15-health-v2-v0/target-link-check.sh /local/celeris/data/scratch/targets/<owner>/target` を当てる。build の前でも後でも exit 0（`missing=0`）になることを確かめる。build の後は v8 の link-search が自分の target の `debug/gn_out/obj` を指し、`outside-present` に数えられる（自分の target の中なので問題ない）。
4. 同じ target に、元の場所（`seeds/` や別の owner の target）を指す値が残っていないことを確かめる（読むだけ）: `T=…/target; for f in "$T"/*/build/*/output; do grep -H 'rustc-link-search' "$f"; done | grep -v "$T" | grep -E '/scratch/(targets|seeds)/'`。何も出なければよい（`root-output` の下の値は cargo が付け替えるので、build の後は出ない）。

## 未解決
- seed から写した target はすべて、v8 の build script の再実行と v8 crate の再コンパイルが要る（seed の v8 は必ず seed の世代を指すため）。V0 では v8 を含む build が 1 分 36 秒だった。正しさのための費用として受け入れたが、seed の効果は v8 の分だけ減る。
- `rustc-link-search` 以外の build script の出力（`cargo:rustc-link-arg`・`cargo:rustc-flags` の `-L`・links の metadata。例: この run の target の aws-lc-rs の出力の `cargo:include=`・`cargo:root=` が seed の世代の aws-lc-sys の OUT_DIR を指す）は見ていない（ADR の範囲外）。link の失敗を起こした例はまだ無い。
- 本番の scratch では未確認（release の後に上の手順で確かめる）。

## 提案
1. KB の `environment/celeris/scratch-seed-v8-stale-link-search.md` を、この変更の release の後に次のように直す（この task では KB を直接書き換えない）。
   - 「恒久の直しは seed 側 … 未対応」を「恒久の直しは ADR 2026-11-16-scratch-stale-build-outputs（task 01M7H4K519QZGY2875BHNFTMKX）。引き継ぎ・seed の写しの直後に、`targets/`・`seeds/` の下の自分以外を指す `rustc-link-search` を持つ package の build の出力を自動で消す。消した package は `target-origin.json` の `removed_build_outputs` に残る」に替える。
   - 「直し方（その target だけ）: `cargo clean -p v8`」に、「`removed_build_outputs` の無い `adopted`・`seed` の target（この release より前に作ったもの）か、引き継ぎ・写しの後に v8 を作り直していない target で、元の場所が消えたときだけ要る。確かめるのは `target-link-check.sh`」と条件を足す。
   - 症状の節に「引き継ぎ（rename）でも起きる（健全性v2 V0、2026-11-15）」を足す。
2. 未解決の 1 の費用を減らすなら、引き継ぎ・写しで `gn_out` も一緒に動くことを使い、v8 のような「OUT_DIR の外で、自分の target の中の同じ相対 path に中身が在る」値を新しい target の path に書き換える案がある。cargo の記録を書き換えることになり、package ごとに中身が在るかを確かめる必要があるので、この task では採らなかった（ADR の選択肢 (c)）。seed の写しの後の build 時間を測ってから別 task で決める。
