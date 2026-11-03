---
title: P1-E(1) lane_rules の本番 compare の preflight・人の手順・集計 script
tasks: [01M6HGFR1TMY7PTGQ553FQGM7H]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# P1-E(1) lane_rules の本番 compare の preflight・人の手順・集計 script

task 01M6HGFR1TMY7PTGQ553FQGM7H（親 01M6HG7T6ZKTX239KGH0QY4HFQ の p1e-shadow-preflight）。設計は [境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 P1-E。基準は `git merge-base HEAD main`。親の分割の記録（`agent-docs/progress/2026-11-03-ts-control-ops.md`）は親 branch にあり、この基準の main には無い。

## 結論
- 本番はまだ compare に上げられない。人の手順が 3 段先に要る: S5 以後の release の promote（今の release `3259e04ea63e` は schema 102 で script の表が無い）、`[script]` の追加と再起動、bundle の配備と登録。preflight は exit 3（fail 0 件、human 6 件。raw は [p1e-preflight-prod.json](p1e-preflight-prod.json)）。
- 測定環境との分離は確かめた。測定の config 7 件はすべて DB・API の listen が本番と別で `[script]` が無い。動いている測定 daemon（K2 p7）は本番と別の binary・DB・port である。止まった測定の config 4 件だけが `selfdeploy.releases_dir` で本番の releases を指す（下の未解決 1）。
- 件数の見込み: 2026-10-20 以後の本番の実績に記録の重複の抑えを当てると、`lane_rules` は 48.45 件/日で **1000 件まで約 20.6 日**、`cos_digest` は 54.4 件/日で 200 件まで約 3.7 日である。synthetic で補わない。
- S5 の手順の誤りを 1 つ見つけて改めた: `[script]` を足す段が無かった。今の本番 binary は `[script]` を未知の欄として拒むので、promote の後に足す（付記 P1-E）。
- worker は本番の DB・config を読んだだけで、何も書いていない。release・promote・mode・guard・K2/K4/IEEE Access の環境は変えていない。

## 変更（file・関数）
- `scripts/dev/script-compare.py`（新規。読み取り専用）
  - `cmd_preflight`: `preflight_release`（`gate.json` の 4 段と `script-bundle/`、release の CLI）、`preflight_db`（schema 107 以上、表、全領域 `rust`、全体の停止なし、未処理の受信箱）、`preflight_isolation`（測定の config と動いている daemon の DB・listen・`[script]`・本番への参照、測定の binary の置き場）、`estimate_volume`（`routing_decided` に S2-4 の重複の抑えを当てた見込み）。
  - `cmd_report`: `read_snapshot`（全部を 1 つの read transaction で読む）、`aggregate`・`finish_group`（match/mismatch/ts_error/invalid_output/unavailable、一致率、latency の p50・p95・最大、`script_guard::readiness` と同じ ready、`script_guard_state` との照合）、`rollback_summary`（mode の変更、rust への戻しを主体ごとに、generation の無効化・停止、受信箱）、`summarize_samples`（RSS・tick の待ち）、`summarize_tick_log`（`slow tick` の行）。
  - `cmd_sample`: `/proc/<pid>/status` の RSS と `GET /api/v1/daemon` の `script_tick_wait` を 1 行の JSON にする（GET だけ。token は出力しない）。
  - 差分を隠さない: `normalized` が空でない記録、`match` なのに `rust` と `ts` が違う記録、未知の outcome は problem で exit 1。記録の無い計数も exit 1（synthetic の件数を本番の件数と取り違えない）。
- `scripts/dev/script-compare-rehearsal.sh`（新規）: 下の人の手順と同じコマンドを一時 DB の daemon で通す。
- `crates/celeris/tests/script_compare_report.rs`（新規、7 試験）: 本物の store と dispatcher が書いた一時 DB で集計を確かめる。
- `scripts/dev/tests/script-compare.sh`（新規）: 実 migration の schema の一時 DB、偽の `/proc`・本番・測定で、store が書かない破れと preflight の 19 通りの判定を確かめる。
- `scripts/dev/test-affected.sh`: `script-compare.py` か上の試験が変われば `tests/script-compare.sh` を走らせる。
- 境界 ADR に付記 P1-E。crates/ の本体、config 欄、API、権限、(f) の数値は変えていない。

## 試験と検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 1008 passed・0 skipped（celeris・celeris-script・celerisctl）、script bundle の SKIP 0 件（bundle を読む試験は実 bundle で走った）、`tests/script-compare.sh` ok、`test-ts-verify` 18 passed。
- `cargo nextest run -p celeris --test script_compare_report` → exit 0、7 passed。
- `bash scripts/dev/tests/script-compare.sh` → exit 0（33 項目。preflight の判定 19 通り、report の破れ 6 通り、sample 3 通り）。
- `bash scripts/dev/script-compare-rehearsal.sh --out agent-docs/progress/2026-11-03-ts-control-ops/p1e-rehearsal.jsonl` → exit 0（10 段）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。`bash scripts/dev/check-doc-links.sh` → ok。`sh scripts/dev/progress-index.sh --check` → ok。
- 本番の読み取り: preflight → exit 3（human 6・fail 0）、report → exit 3（表が無い）。前後で本番 config の sha256 は `3f2aff24…` のまま、schema は 102、`current` は `releases/3259e04ea63e` のまま。
- 試験が落ちることの確かめ（変異）: 計数の照合を外すと `script_compare_report_refuses_counts_without_records` が落ちる。streak の判定を外すと `script_compare_report_pure_gate_counts_real_records` が落ちる。match の差分の検査を外すと `tests/script-compare.sh` が「差分を隠した match は exit 1: exit 0」で落ちる。どれも元に戻して通る。
- TS-only の経路（`scripts/dev/ts-verify.sh`）は変えていない。edit→verify→activate の秒はこの task では測っていない（変える変更が無いため）。

## 計測（raw は同じ dir）
- [p1e-preflight-prod.json](p1e-preflight-prod.json): 本番の読み取りの結果。exit 3。本番 daemon の RSS は 1,190,984 kB（compare の前の基準）、`events` の最大 id 781877、schema 102。
- [p1e-rehearsal.jsonl](p1e-rehearsal.jsonl): 一時 daemon での手順の通し。10 段、全部期待どおり。

| 段（p1e-rehearsal.jsonl の step） | 結果 |
|---|---|
| `no_script_section` | `[script]` が無い daemon の登録は 503 |
| `start_with_script_section` | `[script]` を足して再起動、3 領域とも `rust` |
| `register_before_deploy` | 配備の前の登録は `script_bundle_not_deployed` で拒否 |
| `deploy_and_register` | 配備と登録で `loaded` |
| `mode_compare` | `lane_rules` だけ `compare`、他の 2 領域は `rust` |
| `dispatch_records` | 本物の dispatch で記録 3 件 |
| `report_not_ready` | match 3、mismatch 0、latency p50 1,522 µs・最大 3,450 µs、streak 3/1000 で ready でなく exit 4 |
| `canary_refused` | operator の token は 403、人の token でも件数不足は 422 |
| `rollback_to_rust` | `rust` に戻すと、次の dispatch で記録が増えない |
| `restart_stays_rust` | 再起動後も `rust`、集計に人の戻し 1 件 |

これは fake harness・debug build の一時 daemon の値で、本番の latency・RSS・tick ではない。件数の見込みは過去の実績からの推定で、件数には足さない。

## 本番への移行（人の手順）
worker は実行していない。どの段も人が決めて実行する。mode・generation の操作の画面は無いので、それらはコマンドで行う（下の提案 1）。記号: `<sha12>` は release、`<bundle>` は bundle の sha256。

### 0. 実行前の確認
- `measure/ops-r2-k2-p7` の測定 daemon が動いている。分離は確かめたが、再起動が K2 の operator session に与える影響は未確認なので、測定の担当の都合を確かめてから進める。
- 確認: `python3 -I scripts/dev/script-compare.py preflight --config ~/.config/celeris/config.toml --state-dir /local/celeris/state --measure-root /local/celeris/data/measure --measure-root /local/celeris/measure-accounts --measure-bin /local/celeris/data/measure-bin/releases --repo . --estimate-since 2026-10-20T00:00:00Z`。fail が 1 件でもあれば（exit 1）止まる。

### 1. release・verify・promote（`[script]` はまだ足さない）
- この task を含む main で `bash scripts/selfdeploy/release.sh main`、`verify.sh <sha12>`。promote は画面の「リリース」（`/releases`）か `promote.sh <sha12>`。
- 確認: 同じ preflight で `release_gate_has_script_steps`・`release_has_ops_cli`・`db_schema_version`・`db_script_tables`・`db_modes_all_rust` が pass。この時点で TS 層は起きない。

### 2. `[script]` を足して再起動
- `cp ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-p1e` の後、`[script]` と `state_dir = "/local/celeris/state"` の 2 行を足す。`active_bundle_sha256` は書かない（読み込みエラー）。
- `systemctl --user restart celeris@<sha12>`（動いている run の扱いは通常の再起動と同じ）。
- 確認: `celerisctl script --config ~/.config/celeris/config.toml domains` が 3 領域とも `rust`。preflight の `config_script_section` が pass。

### 3. bundle の配備と登録
- release と同じ commit の checkout で `cargo run --release -p celeris-script --bin script-deploy -- --dist /local/celeris/state/releases/<sha12>/script-bundle --state-dir /local/celeris/state`。stdout の 1 行が `<bundle>` で、`gate.json` の `script_bundle.bundle_sha256` と同じことを確かめる。
- `celerisctl script --config ~/.config/celeris/config.toml generations register <bundle>`。配備より先だと 409 `script_bundle_not_deployed`（配備の後に登録し直せる）。
- 確認: `... generations list` で `loaded`。

### 4. 標本取りを始める
- 別の端末で `python3 -I scripts/dev/script-compare.py sample --pid "$(systemctl --user show -p MainPID --value celeris@<sha12>)" --api http://127.0.0.1:7710 --token-file ~/.config/celeris/api.token --count 4320 --interval-secs 600 >> ~/p1e-samples.jsonl`（10 分ごとに 30 日分）。daemon を再起動したら pid を変えて始め直す。

### 5. `lane_rules` を compare にする
- `celerisctl script --config ~/.config/celeris/config.toml domains` で `version` を読み、`... mode lane_rules compare --expected-version <version> --generation <bundle> --mode-reason "<人の判断の記録>"`。compare は判定の結果を変えない（Rust の結果を使い、TS を待たない）。
- 確認: `domains` で `lane_rules` だけ `compare`。次の dispatch の後の集計（6.）に記録が出る。

### 6. 集計（読み取り専用。何度でも）
- `journalctl --user -u celeris@<sha12> --since "<compare にした時刻>" > ~/p1e-daemon.log`
- `python3 -I scripts/dev/script-compare.py report --db /local/celeris/data/db/celeris.sqlite3 --samples ~/p1e-samples.jsonl --tick-log ~/p1e-daemon.log --require-ready lane_rules --format table`。json は `--format json`。
- exit: 0 = ready。4 = 矛盾は無いが件数不足。1 = problem あり（`PROBLEM` の行を読み、7. で戻す）。3 = 表が無い。
- compare の `script_tick_wait` は 0 が正しい（compare は TS を待たない。S2-6）。tick への影響は `slow tick` の行を compare の前の log と比べて読む。
- CoS の件数（200 件）は `cos_digest` も compare にしないと増えない。上げるなら 5. と同じコマンドで領域を替え、`--require-ready lane_rules,cos_digest` にする。上げるかは人が決める。

### 7. 戻す
- `celerisctl script --config ~/.config/celeris/config.toml mode lane_rules rust --expected-version <version> --mode-reason "<理由>"`。確認: `domains` が `rust`。集計の `rollback.to_rust_by_actor` に出て、新しい記録が増えない。再起動しても `rust` のまま。
- TS 層ごと止めるとき: `config.toml.bak-p1e` に戻して `systemctl --user restart celeris@<sha12>`。
- release を戻すとき: **先に `[script]` を消す**（今の release の binary は `[script]` を拒む）。schema が 102 より進むので `rollback.sh` は `--restore-db` が要り、昇格の後の仕事を失う。人が選ぶ。
- 自動の戻しと受信箱: compare では不一致が出ても mode は変わらず、計数の streak が 0 に戻る。安全違反は generation を `invalid` にして `rust` に戻し、受信箱に `script_control` が出る（画面の「受信箱」→「daemon の画面」）。

### 8. 次へ進む条件
- `report` が exit 0 になっても canary には自動で上がらない。canary・ts は別の人の承認で、人の token（`CELERIS_API_PRINCIPAL=human:web`）だけが通る。この手順の範囲外である。

## 未解決
1. 止まった測定の config 4 件（`main`・`main-2`・`ops-r2-k2-p1`・`ops-r2-k2-p2`）は `selfdeploy.releases_dir` が本番の `/local/celeris/state/releases` を指す。動いていないので今の測定には影響しないが、起動し直すと本番の releases を読む。preflight は human と出す。測定の config は worker が変えない。
2. `measure/main-3`・`main-4` などには深さ 3 までに `daemon.toml` が無く、config の検査の対象外である。動いている daemon は process から見るので検査に入る（今は K2 p7 だけ）。K4 の環境は今 daemon が動いておらず、config の置き場は未確認。
3. release の `script-bundle/` からの `script-deploy` は未確認（release を作っていないので、通しでは `build-bundle.mjs` の出力を使った）。`script-deploy` は release に入っていないので、人の手順に cargo の build が要る。
4. `lane_rules` の 1000 件は今の負荷で約 20.6 日の見込み。負荷が変われば変わる。`p1e-shadow-measure` はその間待つ。
5. 本番での latency・RSS・tick は未計測（compare の後に 6. で取る）。debug build の値は S6・P1-C と同じく本番の値でない。
6. P1-D の意図した差分の outcome が main に入ると、集計はそれを未知の outcome として exit 1 で止める（fail-closed）。入った後で `OUTCOMES` と定義を直す。
7. 集計は `events` を全走査する。本番で 4 秒だった（2026-11-03、78 万行）。読むだけで書き込みを止めない。

## 次の gate
- 人の判断: 上の 1.〜5. を実行するか、いつ実行するか（親の「本番切替に要る人の判断」1・3）。
- その後に `p1e-shadow-measure` が 6. の集計を読み取り専用で行う。人の承認が無ければ question で止まる。

## 提案
1. mode・generation の操作の画面を足す。人向けの手順を画面の操作で書けるようになる。今は受信箱と daemon の画面から mode を変えられない。
2. `script-deploy` を release に入れる。手順 3. の cargo の build が要らなくなる。
3. 本番の `script_compare_recorded` が増えたら、`events` の部分 index を足すかを実測で決める。

## commit（件名）
- p1e: lane_rules の本番 compare の preflight・集計・標本取りの読み取り専用 script と一時 DB の試験、手順の通し
