---
title: core-fix — s5-core 統合後の GenerationNotLoaded（celeris の script_compare 試験 2 件）を直す
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# core-fix — s5-core 統合後の GenerationNotLoaded を直す

## 結論
原因は試験の前提で、本体（daemon・store・runtime）は付記 S5 のとおりに動いていた。2 試験は「rust に戻した後、同じ generation をもう一度 compare で指す」形で、store-gen 以前は `loaded` のまま指し直せた。store-gen の後は、戻す CAS で generation が `draining` になり、`draining` を行き先にする CAS は付記 S5-4 の規則どおり `GenerationNotLoaded` で拒まれる。試験を付記 S5-3 の known-good の戻り道（`retired` にしてから同じ hash を登録し直して `loaded` に戻す）に合わせた。store の規則と試験の検査内容は弱めていない。

## 再現と特定
- 統合後の branch で `cargo nextest run -p celeris -E 'test(script_compare_daemon_injects_records_and_restores_rust) | test(script_compare_cos_digest_daemon_restart_without_bundle_restores_rust)'` → 2 件とも `crates/celeris/tests/support/script_compare.rs:487`（`set_mode` の expect）で `GenerationNotLoaded` で落ちた。
- 落ちた呼び出しは、どちらの試験も 2 回目の `env.set_mode(ScriptMode::Compare, Some(generation.id))`（「配備先に無い bundle を指す daemon も同じ」の直前）。
- そのときの generation の状態の遷移:
  1. 1 回目の daemon の起動で `loaded` の行ができる。
  2. 1 回目の `set_mode(Compare)` の CAS で `active` になる（`crates/task-core/src/store/script_domains.rs` の CAS の中の状態の移動）。
  3. `[script]` の無い daemon の起動時の復元（`crates/celeris/src/daemon/script_layer.rs`）が actor `system:script-startup` で mode を `rust` に戻す。どの領域からも指されなくなったので、同じ CAS で `draining` になる（付記 S5-2「切替」）。
  4. 2 回目の `set_mode(Compare)` は `draining` を指すので store が `GenerationNotLoaded` で拒む（付記 S5-4 の 1 行目「generation が `loaded`・`active`」）。
- 修正後の試験は、3 の後の状態が `draining` であることを assert してから戻り道を通すので、この遷移も検査している。
- daemon の起動・停止で `draining` を `retired` にする処理（in-flight 0 での drain の終わり）は daemon-gen unit の範囲で、まだ無い。この unit では足さない。

## 直し方（commit `phase s5: core-fix: draining の generation を retired→再登録してから compare に戻す`）
- `crates/celeris/tests/support/script_compare.rs`: `Env::rearm_generation(id)` を足した。今の状態が `draining` であることを assert し、`script_generation_transition` で `retired` にし、`script_generation_record` で同じ hash を登録し直して `loaded` に戻ったこと（同じ行の id）を assert する。task-api の `rearm_generation`・task-dispatch の `cos_digest_script.rs` の `set_mode`（store-gen の追従）と同じ形。
- `crates/celeris/tests/script_compare_lane_rules.rs`・`crates/celeris/tests/script_compare_cos_digest.rs`: 2 回目の `set_mode(Compare)` の前で `rearm_generation` を呼ぶ。その後の検査（読めない bundle を指す daemon が `rust` に戻し、記録を増やさない）は変えていない。

## 証拠（基準は `$CELERIS_WU_BASE`、提出時の HEAD で実行）
- `cargo nextest run --no-fail-fast -p celeris -E 'binary(/^script_compare_/)'` → 25 passed、0 skipped（control-ts/dist は無く、試験が source から bundle を作った）。
- `sh scripts/dev/tests/script-bundle-fresh.sh` → `script-bundle-fresh: all ok`（(a)〜(d)、(t1)〜(t5) すべて ok）。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celerisctl、nextest 884 passed・0 skipped。control-ts の source から bundle を作り、script bundle の SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決
- daemon の再起動で `draining` の generation が `draining` のまま残る（isolate は process と一緒に消える）。付記 S5-2 の起動時の復元と drain の終わりとして `retired` にするかは daemon-gen unit で決めて実装する。

## 提案
- daemon-gen unit の objective に「起動時に、mode の行から指されない `draining` の generation を in-flight 0 の drain の終わりとして `retired` にする（`script_generation_retired`、`cancelled_in_flight` 0）」を入れる。入れたら `rearm_generation` の assert（`draining`）は `retired` を受ける形に直す。
