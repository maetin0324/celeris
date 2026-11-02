---
title: S1 overhead — 判定遅延・RSS・release build と binary の増分の計測
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S1 overhead — 許容 overhead の計測

## 結論
境界 ADR (f) の閾値 6 項目はすべて閾値内だった。S2 へ進まない基準には当たらない。数値は `overhead.json` にあり、閾値の判定は WU の check（`overhead.json` を読む python）が行う。

| 項目 | 測定値 | 閾値 | 判定 |
|---|---|---|---|
| warm の判定遅延 p50 | 0.087 ms | ≤ 1 ms | 内 |
| warm の判定遅延 p99 | 0.117 ms | ≤ 5 ms | 内 |
| RSS 増分 isolate 1 個 | 24.0 MiB | ≤ 100 MiB | 内 |
| RSS 増分 isolate 2 個 | 31.1 MiB | ≤ 200 MiB | 内 |
| celeris の cold release build 増分 | +2.4 秒（132.3 → 134.6） | ≤ 60 秒 | 内 |
| celeris の release binary 増分 | +64.2 MiB（79.6 → 143.7） | ≤ 80 MiB | 内 |

参考値（閾値の判定に入れない）: celerisctl の release binary は 36.5 MiB → 93.2 MiB（+56.7 MiB）。celerisctl は celeris の lib に通常依存するため deno_core と V8 が入る（daemon-wire の境界 ADR 付記）。

binary 増分は閾値 80 MiB に対して余裕が 15.8 MiB しかない。V8 の静的 archive が大半を占める。

## 測り方
この unit はコードを変えていない。base は `git merge-base HEAD main`、head は提出時の HEAD。

1. 両方の木を `git archive <rev> | tar -x` で `$TMPDIR` に展開した（作業ツリーと git の worktree 一覧を変えないため）。
2. cold build は、渡された `CARGO_TARGET_DIR` の下に新しい subdir（`overhead-cold-base2`・`overhead-cold-head`）を作って `cargo build --release --locked -p celeris --bin celeris` を走らせた。秒は `date +%s%N` の前後差。sccache の cache を当てないため、`SCCACHE_BIN=/nonexistent` を付けた。host の rustc-wrapper 台本はこのとき sccache を通さず rustc を直接動かす（台本と cargo 設定は変えていない）。
3. 時間を測った build の後、同じ subdir で `-p celerisctl` と head の `-p celeris-script --bin script-bench` を release で build し、`stat -c %s` で大きさを読んだ。計測後に subdir を `rm -rf` で消した。
4. base は 2 回 build した。1 回目は秒の取り方を誤り、cargo の表示（`Finished ... in 2m 13s`）しか残らなかった。`overhead.json` の base は ms で測った 2 回目の 132.254 秒。binary の大きさは 2 回とも 83,430,066 byte で同じだった。
5. bundle は `control-ts` で `corepack pnpm@12.6.0 install --offline --frozen-lockfile`、`run typecheck`、`run bundle` で作った（`bundle_sha256` b2f49a878841541ee3492933277ed1d8cc07fa0e5165c4ec0ac604453daf0557）。
6. `script-bench --bundle-dir control-ts/dist --iters 1000 --out …` を release build で 3 回走らせた。判定 1 回は試験用の領域 `host_selftest` の呼び出し（async op 1 回を含む）で、isolate ごとに warm up 100 回を捨てる。`overhead.json` の値は 3 回の各欄の最大値。

| 回 | p50 ms | p99 ms | RSS 1 個 MiB | RSS 2 個 MiB |
|---|---|---|---|---|
| 1 | 0.0849 | 0.1131 | 23.81 | 31.13 |
| 2 | 0.0868 | 0.1157 | 24.00 | 31.13 |
| 3 | 0.0864 | 0.1172 | 23.44 | 30.38 |

## task-dispatch・task-api が deno_core を持たないこと
- `cargo tree --locked -e normal -p task-dispatch -i deno_core`・同じく `-p task-api`: どちらも `package ID specification `deno_core` did not match any packages`（依存の木に無い）。
- `cargo tree --locked -p task-dispatch | grep -cE 'deno_core|\bv8\b|serde_v8'`・同じく `task-api`（dev・build 依存を含む）: どちらも 0。
- 比較: `cargo tree --locked -e normal -p celeris -i deno_core` は `deno_core v0.413.0 (crates/script-runtime/vendor/deno_core) ← celeris-script ← celeris` を出す。

## 計測の条件と限界
- host は共用の home-dev（AMD Ryzen Threadripper PRO 3945WX、24 thread、125 GiB、kernel 6.8.12-9-pve）。build 中の 1 分 load average は 9〜14 で、他の作業と並んでいた。build の秒は各 1 回（base は ms 計測 1 回）なので、数秒の差は誤差の範囲にある。閾値 60 秒に対する判定は変わらない。
- V8 は host の `RUSTY_V8_ARCHIVE`（simdutf 版 v150.4.0 の prebuilt archive）を link する。V8 を source から build する場合の秒は測っていない。
- rustc / cargo 1.98.1、node v24.14.1。

## 証拠（提出時の HEAD で実行）
- 閾値の check（WU の check と同じ python）: exit 0。
- 範囲 check: 範囲外の path なし。
- `bash scripts/dev/test-affected.sh`: exit 0（「試験対象の変更なし（docs のみ等）」）。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。`sh scripts/dev/progress-index.sh --check`: ok。

## 未解決事項
- なし。

## 提案
- binary の増分（+64.2 MiB）は閾値の 80% を超えている。S2 以降で TS 側の依存や V8 の feature を増やす変更は、この計測を再実行して余裕を確かめる。
- celerisctl の +56.7 MiB は閾値の対象外だが、package 分割（daemon-wire の案 B）で消せる。close の提案と合わせて判断する。
