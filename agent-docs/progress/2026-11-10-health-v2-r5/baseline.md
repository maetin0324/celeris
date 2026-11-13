---
title: 健全性v2 R5 baseline — base の test build/link・edit→verify・実行時間・flaky と、crate の組による作り直しの原因
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 baseline

完了日: 2026-11-10

WorkUnit `baseline`。基準は `git merge-base HEAD main`（計測時の HEAD と同じ commit。`baseline.json` の `measured_src_equals_merge_base` が true で、crates/・Cargo.*・.config/nextest.toml・test-affected.sh が基準と同じだったことを示す）。repo のコードは変えていない。

## 結論
- base で task-api・celeris・celerisctl の build/link・edit→verify・2 回の試験の実行を測り、`baseline.json` に書いた（`measure.sh --validate` は `ok: 3 crates, 0 skipped item(s)`）。2 回の実行とも失敗した試験は 0 件で、flaky は見つからなかった。
- 作り直しの原因: Cargo（resolver 2）は `-p` で選んだ crate の組の中だけで features を統合する。この workspace では crate ごとに共有 crate へ足す features が違う（試験用の dev-dependencies と通常の依存の両方）。組が変わると共有 crate の features が変わり、その crate と逆依存の crate 全部が別の版（別の metadata hash）として build される。
- 版は target dir に並んで残る。A → B → A → B の順の build では、戻った 3 回目・4 回目の Compiling 行が 0 だった。つまり作り直しは「その組の版を初めて使うとき」と「その版を最後に build した後に source が変わったとき（編集・main 同期・rebase）」に、組ごとに別々に起きる。
- 直す案は write set の中にある。`scripts/dev/test-affected.sh` が build と実行を `--workspace` の features で行い、走らせる試験は nextest の filter で今と同じ crate に絞る（案 F1）。これで test-affected の features は release gate（`--workspace`）と同じになる。Cargo の `resolver.feature-unification = "workspace"` は cargo 1.98.1 では `-Zfeature-unification` が無いと無視されたので使えない。

## 計測の方法（measure.sh）
- 台本: `agent-docs/progress/2026-11-10-health-v2-r5/measure.sh`（B0 の `test-targets-measure.sh` が元）。
  - `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh run <label> <out.json> <crate,...>`
  - `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate <out.json>`（必要な欄があれば 0。測れなかった項目は `skipped` に crate・item・理由があれば通す）
  - `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh features <out.json> <組A> <組B>`（下の原因調査に使った）
  - `MEASURE_DEADLINE`（epoch 秒）を渡すと、それを過ぎた段は走らせず `skipped` に理由を書く。走らせる段も残り秒数を timeout にする。
- crate ごとの段:
  1. `build`: `cargo nextest run -p <crate> --no-run` の秒・Compiling 行の数と名前・test binary の数・この build で書かれた（link した）test binary の数（binary の mtime が開始より後のもの）。
  2. `edit_verify`: `src/lib.rs`（celerisctl は lib が無いので `src/main.rs`）の末尾に 1 行足し、`CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list` の秒と、対象 crate 全部の `cargo nextest run -p … --no-run` の秒を測る。settle は B0 と同じく編集の前の build だが、B0 の R3（14 crate 全部で settle）と違い、編集で決まる対象 crate と同じ組で行う。B0 が「交絡」とした、組の違いと前の編集の mtime による作り直しを除くため。
  3. `run1`・`run2`: 先に `--no-run` で build を済ませ、`cargo nextest run -p <crate> --no-fail-fast` を 2 回走らせた秒・件数・失敗した試験名。
  4. 各段の前後と全体の前後に `/proc/loadavg` を記録する。
- 生の出力: `baseline-raw/`（各段の log・nextest の binary 一覧・`steps.jsonl`）。

## 環境
- host: 24 CPU、Linux 6.8.12-9-pve、cargo 1.98.1、cargo-nextest 0.9.146。CARGO_TARGET_DIR は celeris が渡した WU の scratch（開始時 1.3G。依存の一部は未 build）。
- load average（1 分・5 分・15 分）: 計測の前 23.05 22.44 29.15、後 27.64 50.71 48.54。計測中の 1 分値は 23〜84。自分の build を含み、他の process の負荷と分けていない。
- 3 crate を task-api → celeris → celerisctl の順に 1 回の run（1427 秒）で測った。並走の cargo は走らせていない。

## 結果（秒）
| crate | build --no-run | Compiling | test binary | link | 対象 crate | settle（Compiling） | --list | 再 build（Compiling） | edit→verify | 再 build で link | run1 | run2 | 試験 | 失敗 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| task-api | 133.25 | 76 | 114 | 114 | 7 | 216.92（15） | 0.27 | 155.51（5） | 155.78 | 328 / 333 | 59.56 | 60.64 | 876（skip 2） | 0 / 0 |
| celeris | 124.64 | 14 | 57 | 57 | 2 | 42.18（2） | 0.34 | 36.76（2） | 37.11 | 76 / 76 | 184.30 | 69.98 | 802（skip 2） | 0 / 0 |
| celerisctl | 55.60 | 26 | 19 | 19 | 1 | 5.54（1） | 0.12 | 5.41（1） | 5.53 | 1 / 19 | 4.78 | 4.45 | 205 | 0 / 0 |

- edit→verify は `--list` の秒と再 build の秒の和（B0 と同じ定義）。再 build の Compiling は task-api の編集で task-api・celeris・celeris-campaign・task-worker・celerisctl、celeris の編集で celeris・celerisctl、celerisctl の編集で celerisctl だけだった。
- task-api の編集は対象の 7 crate の test binary 333 個のうち 328 個を link し直した。task-api の 114 個、celeris の 57 個が束ねの主な対象になる。
- celeris の run1（184.30 秒）と run2（69.98 秒）の差: run1 の間に 1 分値が 62 から 84 に上がった。試験の件数と結果は同じだった。差の原因は未確認（負荷か、初回の実行による page cache の差か）。
- 3 crate とも 2 回の実行で失敗・timeout は 0 件だった。skip 2 件は nextest の skip（task-api・celeris に各 2 件）。
- B0（`2026-11-07-health-v2-b0/test-targets.md`）との比較: B0 の task-api の再 build R1〜R3 は 105〜113 秒、celeris は 33〜85 秒。今回は同じ組で settle した後の値で、task-api 155.78 秒・celeris 37.11 秒。負荷（今回 41〜69、B0 は 2〜36）と settle の方法が違うので、差を変更の効果と読まない。前後比較は measure-build の unit で、base と HEAD を同じ run・同じ方法で測る。

## 作り直しの原因（crate の組と features）
### Compiling 行の比較
- 同じ run の中で、source を変えずに組だけを変えた所で、共有 crate が作り直された（`baseline.json` の `compiled`・`settle_compiled`）。
  - `-p task-api` の build の直後に、task-api の編集の対象の 7 crate の組（以下 B）で settle した → 15 crate を Compiling（task-core・jev-classifier・deno_core・celeris-script・celeris-credentiald・task-ops・routing-eval・task-worker・task-api・celeris-mcp・task-dispatch・celeris-campaign・llm-proxy・celeris・celerisctl）。task-core・deno_core は source が変わっていない。
  - B の直後の `-p celeris` → 14 crate（clap・clap_builder と workspace の 12 crate）。
  - celeris の編集の組（celeris・celerisctl、以下 A）の直後の `-p celerisctl` → 26 crate（tokio・tokio-util・tower・hyper・axum・reqwest・deno_core などと workspace の crate）。
- `measure.sh features` で A → B → A → B の順に `--no-run` を走らせた（`features.json`、生の出力 `features-raw/`）:

| 回 | 組 | 秒 | Compiling |
|---|---|---|---|
| 1 | A | 5.97 | celerisctl（直前の編集を戻した mtime の分） |
| 2 | B | 40.22 | celeris・celerisctl（同じく mtime の分） |
| 3 | A | 1.65 | 0 |
| 4 | B | 2.95 | 0 |

  戻った組は作り直さない。features の違う版は target dir に並んで残り、版ごとに source の変化を追う。

### features の差（`cargo tree -e features`）
- A と B の差（`features.json` の `feature_diff`）: B だけにある features は 4 package。
  - task-worker: `attack-test-hooks`・`h3-e2e-insecure-cert`・`live-cross-test-support`・`same-uid-harness`（task-api の dev-dependencies）
  - celeris-credentiald: `same-uid-harness`（task-api と task-worker の dev-dependencies）
  - syn: `visit`（task-api の dev-dependencies）
  - time: `macros`（llm-proxy の通常の依存）
- 調べる前の候補（task-api の dev-dependencies が task-worker・celeris-credentiald に足す features）は原因の一部だった。1 crate だけの組では、通常の依存が足す features の差も加わる。`--workspace` と比べた差（`features-raw/feature-diffs.txt`）:

| 組 | `--workspace` より features が少ない package の数 | 主な差 |
|---|---|---|
| B（task-api の編集の対象 7 crate） | 0 | — |
| A（celeris・celerisctl） | 3 | task-worker の試験用 4 つ、celeris-credentiald `same-uid-harness`、time `macros` |
| `-p celeris` | 5 | A の 3 つと clap・clap_builder `env` |
| `-p celerisctl` | 4 | A の 3 つと tokio `test-util`（celeris の dev-dependencies） |
| `-p task-api` | 27 | tokio `full`・`signal`・`parking_lot`・`io-std`、futures-util、rusqlite `hooks`・`limits`、reqwest `blocking`・`stream` など |
| `-p task-worker` | 28 | `-p task-api` の差と task-worker 自身の `h3-e2e-insecure-cert`、tokio `test-util` |

- tokio のように features が変わる crate が依存の根に近いと、その上の crate 全部（hyper・axum・reqwest・deno_core・workspace の crate）が別の版になる。`-p celerisctl` の 26 crate はこの例。

### 原因のまとめ
1. Cargo（resolver 2）は選んだ組の中だけで features を統合する。
2. workspace の crate は共有 crate に違う features を足す（試験用の dev-dependencies と、一部の crate だけの通常の依存）。
3. test-affected.sh は変更ごとに違う組を `-p` で渡す。組ごとに共有 crate の features が違い、別の版として build される。版は残るが、編集・main 同期・rebase で source が変わると、使う組ごとに作り直しが要る。task の scratch の target に無い版は初回に build される。版の数だけ target dir も大きくなる。

## release gate（`--workspace`）との features の一致
- release gate の試験（`scripts/dev/test-parallel.sh` = `cargo nextest run --workspace`）は全 crate の features の和集合で build する。
- test-affected.sh の組が `--workspace` と同じ features になるのは、上の表の B のように差が 0 の組だけ。A（celeris の編集）では、celeris の試験は task-worker の試験用 features を切った task-worker と link される。`-p task-worker` だけの組（そうなる編集があれば）では task-worker 自身の `h3-e2e-insecure-cert` が切れる。feature で切り替わる code の試験は、test-affected と release gate とで違う build を見ることがある。

## 直す案
| 案 | 置き場所 | write set | 内容 | 評価 |
|---|---|---|---|---|
| F1 | `scripts/dev/test-affected.sh` | 中 | `cargo nextest run --workspace -E 'package(a) \| package(b) …'` で build と実行を `--workspace` の features に揃え、走らせる試験は今と同じ crate に絞る | 推す。走らせる試験の組は変えない（対象規則を弱めない）。features は release gate と同じになり、版は 1 つになる。費用: 版が古いときは対象外の crate の test binary も build・link する（B0 の 14 crate 全部の再 build は 139〜144 秒）。source が新しい版では、作り直しは変更した crate と逆依存だけで、今の対象と同じ |
| F1 の doc test | 同上 | 中 | `cargo test --doc -p …` も組ごとの版を作る。`--workspace` にすると全 crate の doc test を走らせることになる | 未決。doc test の件数と秒は未計測。ADR（unit `adr`）で決める |
| F2 | `crates/*/Cargo.toml` | 中（既にある依存の features を揃える所まで） | 各 crate の既にある依存の宣言に、他の crate が足す features を足して揃える | 推さない。1 crate の組で差が出る 27〜28 package の多くは、その crate が直接依存しない crate（icu_*・zerovec など）。揃えるには依存を足す必要があり、Cargo.lock が変わる（write set の外）。依存が増えるたびに崩れる |
| F3 | Cargo の `resolver.feature-unification = "workspace"` | 外（`.cargo/config.toml` は worktree-target-dir.sh が作る。環境変数なら test-affected.sh で渡せる） | 選んだ組でも features を workspace 全体で統合する | 今は使えない。`CARGO_RESOLVER_FEATURE_UNIFICATION=workspace cargo tree -p celerisctl …` は「ignoring `resolver.feature-unification` without `-Zfeature-unification`」と出し、差は消えなかった（`features-raw/tree-celerisctl-fu-workspace.txt`）。stable の cargo で使えるようになれば F1 より小さい変更で済む |

- 案 F1 を `affected-features` の unit で入れる前に、ADR で次を決める: doc test の扱い、版が古いときの build の費用（対象外の crate の link）を受け入れるか。
- 未確認: task の scratch の target の seed（reflink）がどの組の版を持っているか。seed が `--workspace` の版なら、F1 は seed の版をそのまま使える。

## 証拠コマンド（提出時の HEAD で実行）
- `MEASURE_DEADLINE=$(( $(date +%s) + 1850 )) bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh run base agent-docs/progress/2026-11-10-health-v2-r5/baseline.json task-api,celeris,celerisctl` → exit 0、1427 秒、`ok: 3 crates, 0 skipped item(s)`
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh features agent-docs/progress/2026-11-10-health-v2-r5/features.json celeris,celerisctl celeris,celeris-campaign,celerisctl,llm-proxy,task-api,task-dispatch,task-worker` → exit 0、Compiling は 1・2・0・0
- `cargo tree --workspace|-p <crate> -e features,normal,build,dev -f '{p} {f}' --prefix none` → `features-raw/tree-*.txt`・`feature-diffs.txt`
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/baseline.json` → exit 0
- `bash scripts/dev/test-affected.sh` / `cargo clippy --workspace -- -D warnings` / `cargo fmt --all -- --check` → 結果は WU の報告に書く

## commit
- R5 baseline: 計測台本 measure.sh と base の計測、crate の組による作り直しの原因を記録する

## 未解決事項
- doc test の件数・秒（F1 で doc test を `--workspace` にするかの材料）は未計測。
- celeris の run1 と run2 の差（184 秒と 70 秒）の原因は未確認。measure-run の unit で同じ方法で測り直す。

## 提案
- F1 を `affected-features` の unit で入れ、`measure.sh` の `edit_verify` と、組を変えた後の build（A → B → A）で効果を確かめる。
- cargo の `feature-unification` が stable になったら F3 に替える案を、ADR に撤去の条件として書く。
