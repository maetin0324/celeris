---
title: K2 の採点を評価器 uid で通す（grade wrapper の引数・sudo 下の toolchain・env_failure・case.json の hash 反映）
tasks: [01M678HHSHGT3Y5J4H7RRYFXD8]
status: done
updated: 2026-10-30
---
# K2 の採点を評価器 uid で通す

完了日: 2026-10-30

ADR: [2026-10-30-ops-r2-isolation-principal](../adr/2026-10-30-ops-r2-isolation-principal.md) 付記「K2 の採点を評価器 uid で通す」（H1〜H6）

## 変更

- H1 `scripts/campaign/ops-r2-grade.py`: 引数は `--tgz`・`--sha256`・`--case` だけ（重複・他の引数は exit 64）。`celeris-campaign grade --case EVAL_ROOT/grade-cases/final/<case>/<case>.json --artifact <work>/artifact --work <work>/grade` を wrapper が組み、stdout を `verdict.json` に受ける。grade の exit 1（不合格）も判定 JSON があれば公開。grade-case が無ければ exit 66。旧い `-- <引数>`・`{work}`/`{verdict}` 置換・`--grader-root`/`--grade-case-root` 付与は消した。
- H2 同 wrapper: 子 process の環境を固定（`PATH=<home>/.cargo/bin:/usr/local/bin:/usr/bin:/bin`・`HOME`・`CARGO_HOME`・`RUSTUP_HOME`・`CARGO_NET_OFFLINE=true`・`LANG`。home は評価器 uid の passwd）。呼び出し側の環境は継がない。
- H3 `crates/campaign/src/grader.rs` `output_is_env_failure`: 行末が `: not found`（dash の `sh: 1: cargo: not found`）を env_failure に。試験 `crates/campaign/tests/grade_env_marker.rs`。
- H4 `grade_out_with` に `work_root`、`celeris-campaign grade-out --work <dir>`。
- H5 `docs/ops/ops-r2-isolation.md` §4.3・§4.4・§4.5: 新しい呼び方、toolchain と crate cache の準備、`tar --dereference --hard-dereference` による tgz の作り方。
- H6 `crates/campaign/tests/support_cases/mod.rs`: `cases/` を `CARGO_TARGET_TMPDIR` に写し、K2 の `private_ref`・`grade_case_ref` を repo の旧 fixture（漏洩済みの試験専用。付記 G1・G4）の実 hash に替えた試験用 cases root。`fixtures/grader-private` と組で K2 を読む試験 15 本と `k2_scope.rs`・`real_asset_grade.rs`、celeris crate の `cap_probe_real_daemon.rs`・`campaign_lock_e2e.rs`・`ops_r1_pipeline.rs` をこれに替えた（helper は `CARGO_MANIFEST_DIR/../campaign` を root にし、celeris 側は `#[path]` で読む）。

commit（件名）: 「ADR 付記: K2 の採点を評価器 uid で通す（H1〜H6）」「campaign: dash の '<name>: not found' を env_failure に、grade-out に --work（H3・H4）」「ops-r2 grade wrapper: celeris-campaign grade の argv と toolchain の環境を wrapper が固定で組む（H1・H2）」「campaign tests: K2 の参照 hash を旧 fixture に合わせた試験用 cases root を読む（H6）」「campaign tests: k2_scope・real_asset_grade も試験用 cases root の K2 pin を読む（H6）」「campaign tests: celeris crate の試験も試験用 cases root を読む（H6）」「docs: ops-r2 §4.3〜4.5 を wrapper の新しい呼び方・toolchain の準備・tgz の作り方に合わせる（H5）」「cargo fmt」。基準は `git merge-base HEAD main`。

## 証拠

- `python3 scripts/campaign/test_ops_r2_grade.py` → 15 件 OK（引数で grade-case・private・作業 dir を差し替えられない、子の環境が固定、不合格の公開、grade-case 欠落）。
- `cargo test -p celeris-campaign --test grade_env_marker` → 2 passed。
- ダミー値（private `ab`×32、grade-case `cd`×32）を `ops-r2-set-private-sha.py` で K2 の `case.json` に入れて `cargo nextest run -p celeris-campaign --no-fail-fast`: H6 前は 126 件が `private file hash mismatch`、H6 の 1 段目の後は 912 件中 910 passed・2 failed（`k2_scope`・`real_asset_grade` の pin 照合）、2 件を直した後に `-E 'binary(k2_scope)|binary(real_asset_grade)|binary(cap_probe)|binary(cases_load)'` → 49 passed。確認後に `case.json` は git で戻した。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0（`cargo fmt --all` を適用して commit した後）。
- ダミー値の下で `bash scripts/dev/test-affected.sh`（1 回目）→ exit 100。celeris crate の 11 件（`cap_probe_real_daemon` 5・`campaign_lock_e2e` 4・`ops_r1_pipeline` 2）が `private file hash mismatch`。celeris-campaign の失敗は 0。
- celeris crate の 3 本を試験用 cases root に替えた後、ダミー値で `cargo nextest run -p celeris -E 'binary(cap_probe_real_daemon)|binary(campaign_lock_e2e)|binary(ops_r1_pipeline)'` → 12 passed。
- `bash scripts/dev/test-affected.sh`（提出時 HEAD に K2 の `case.json` のダミー値だけを足した作業ツリーで実行。対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）→ exit 0、2896 tests run: 2896 passed、9 skipped。実行後に `case.json` は git で戻した（HEAD の `case.json` は旧値のまま。値の反映は operator が行う）。

## 未解決事項

- wrapper は grade に `--initial-tree` を渡さない。採点で compile error が出ると、出どころを決められないので env_failure になる（model_failure に数えない側の誤り）。K2 の初期 tree を EVAL_ROOT に置いて渡すかは後で決める。
- sudo の env_reset 下での実起動は未確認（評価器 uid が要る。下の手順 4 で operator が確かめる）。

## 本番への移行（operator が順に行う）

1. この branch が main に入った release の `celeris-campaign` と wrapper を入れ替える（§4.3 の `sudo install ... celeris-campaign` と `sudo install ... scripts/campaign/ops-r2-grade.py /var/lib/celeris-eval-bin/grade`）。確認: `sha256sum /var/lib/celeris-eval-bin/grade scripts/campaign/ops-r2-grade.py` が一致（`ops-r2-k2-preflight.md` §0）。
2. grade-case が `/var/lib/celeris-eval/grade-cases/final/k2-bf-xattr-restart-campaign/k2-bf-xattr-restart-campaign.json` にあることを評価器の shell で確かめる（無ければ §4.5 の `gen-grade-case` で作る）。
3. hash を反映して commit する: `python3 -I scripts/campaign/ops-r2-set-private-sha.py --case k2-bf-xattr-restart-campaign --sha256 <承認票の private の値> --grade-case-sha256 <承認票の grade-case の値>` → `bash scripts/dev/test-affected.sh` が通ることを見てから commit（値は task・進捗・KB に書かない）。
4. wrapper 経由の採点を確かめる: 試行の出力を §4.3 の `tar --dereference --hard-dereference` で tgz にして drop box に置き、`sudo -u celeris-eval /var/lib/celeris-eval-bin/grade --tgz <tgz> --sha256 <hex> --case k2-bf-xattr-restart-campaign` が exit 0 で outbox の path を 1 行出すこと。toolchain の確認は未修正の公開初期 tree の tgz で行い、判定 JSON に env_failure が無いこと。

## 提案

なし。
