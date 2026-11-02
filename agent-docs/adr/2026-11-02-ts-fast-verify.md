---
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
---
# ADR 2026-11-02: TS だけの変更の高速検証経路（変更の 3 分類・script-host-runner・cache key・grader の保護・計測の定義）

- 状態: 採用（実装済み。実装との違いは末尾の付記 wire・付記 close）。実装は task 01M6EVP95S9VM3DBMZFRD8FS35（親 task 01M6DCMB75G5F0DSE58ZNZMBAK の S3）の
  unit runner・verify-sh・wire・bench・measure-warm・measure-cold。
- 関係: [TS 制御層の境界 ADR](2026-11-02-ts-control-layer-boundary.md)（(b) 接続・(d) 配備・(f) 数値目標・付記 S2-2〜S2-12）、
  [全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)、
  [deno_core の写し](2026-11-02-deno-core-vendored-serde-json-features.md)、
  [破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 根拠: [S0 の計画 §2 S3](../progress/2026-11-02-ts-control-layer-s0.md)、[S2 の進捗](../progress/2026-11-02-ts-control-layer-s2.md)、
  記録 [adr-fast](../progress/2026-11-02-ts-control-layer-s3/adr-fast.md)（設計の前に測った各段の秒）。

## 結論
- 変更を `ts-only`・`host-api`・`rust` の 3 つ（と、試験の対象が無い `none`）に分ける。`ts-only` は「変わった file がすべて
  `control-ts/src/` の業務 TS で、検証の道具・契約・golden・試験の土台に触れていない」ときだけに限る。file 名の一致だけでは決めず、
  TS の import グラフ・fixture を書く Rust・crate の逆依存で、何が影響を受けるかを決める。
- `ts-only` の検証は `scripts/dev/ts-verify.sh` が次の順に行い、cargo を 1 回も起動しない:
  型検査 → bundle（`$TMPDIR` に作り直す）→ 対象の単体試験 → 全領域の Rust golden との比較 → 安全シナリオ →
  事前 build 済みの本物の deno_core host（`script-host-runner`）での統合 → activate の代用（一時 state_dir への配備と load）。
- `script-host-runner` は検証だけに使う道具で、daemon の経路に入らない。cache key（host の source の hash・`Cargo.lock`・features・
  `rustc -vV`・`host-v1.schema.json` の hash）が変わったら runner を作り直し、golden も Rust の example で作り直して commit 済みの fixture と `cmp` する。
- 合否を決めるもの（分類規則・段の並び・golden の比較・安全シナリオの表）は `scripts/dev/` と Rust（`crates/script-runtime/`）が持つ。
  `control-ts/` の中の編集では合否を変えられない。そのような編集は `ts-only` にならず、従来の経路（cargo の試験）へ回る。
- `host-api`・`rust` の変更では従来どおり cargo の試験を走らせる。release gate（`scripts/selfdeploy/release.sh`）の全体試験は変えず、減らさない。

## 背景
S0 の測定では、lane 規則を 1 件足す編集（policy）の edit→release build が warm 中央値 221.0 秒、cold 283.5 秒かかった
（境界 ADR (f)）。S2 で lane 規則（`lane_rules`）と route の候補順（`route_priority`）は TS にも同じ実装ができ、
Rust が書いた fixture（19,683 件・2,026 件）と全件一致した。TS の規則だけを変える編集では、Rust の compile・link は要らない。
一方で、TS だけの変更でも検証を省いてはならない（親 task の S3 節:「TS であればテスト不要という扱いをしない」）。

この ADR を書く前に、この worktree で各段を 1 回ずつ測った（記録 adr-fast。計測の定義 (7) に沿う本番の計測ではない）:
`tsc --noEmit` 0.08 秒、bundle 0.07 秒、`vitest run`（3 file）0.5 秒、`node --test scripts/*.test.mjs` 0.85 秒、
`compare.mjs` 2 領域 0.28 秒、cache key 用の 516 file の hash 0.02 秒、`rustc -V` 0.02 秒。
本物の host の 1 呼び出し（`script-bench`、async op を 1 回含む `host_selftest`）は release build で p50 0.084 ms、debug build で p50 0.27 ms。

## 決定

### (1) 変更の分類（TS-only／Host API／Rust）
**入力**: 基準 `${CELERIS_WU_BASE:-$(git merge-base HEAD main)}` との `git diff --name-status`（rename は両側の path、削除は `D`）と未追跡 file。
`test-affected.sh` と同じ集め方にする。

**file ごとの区分**（上から最初に当たったものを使う）:
| 区分 | path | 理由 |
|---|---|---|
| 中立（分類に影響しない） | `agent-docs/**`、`docs/**`（`docs/api/**` を除く） | 進捗・ADR・文書。TS の変更と同じ commit に必ず入るので、これで `ts-only` を外さない |
| Host API | `docs/api/script-host/**`、`control-ts/src/host.d.ts`、`crates/script-runtime/src/ops.rs`・`dto.rs`・`host.js`・`bootstrap.js` | TS と Rust の契約（schema・型・native op・bootstrap） |
| Host API（TS の検証の道具・golden） | `control-ts/fixtures/**`（Rust golden）、`control-ts/scripts/**`、`control-ts/package.json`、`control-ts/pnpm-lock.yaml`、`control-ts/pnpm-workspace.yaml`、`control-ts/tsconfig.json`、`control-ts/vitest.config.mjs`、`control-ts/bundle.config.json`（manifest の `domains`・`capabilities`）、`control-ts/src/testing/**`（host の写しの stub）、`control-ts/src/**/*.test.ts` の削除、`control-ts/` 直下のその他 | 検証の道具・依存の版・試験の土台・golden を変える。これらを `ts-only` にすると、検証そのものを弱める編集が速い経路を通れる |
| TS-only の候補 | `control-ts/src/**/*.ts`（上の行に当たらないもの） | 業務 TS と、その単体試験の追加・変更 |
| Rust | 上のどれにも当たらない path（`crates/**`、`Cargo.lock`、ルート `Cargo.toml`、`scripts/**`、`web/**` など） | 従来の経路 |

**変更全体の分類** = 区分の強い方: `rust` > `host-api` > `ts-only` > `none`。中立だけなら `none`。

**依存グラフで決めること**（file 名の表は入口で、影響の範囲はグラフで決める）:
1. TS の import グラフ: `control-ts/node_modules` の esbuild（lockfile で版が固定され、lockfile は `ts-only` にならない）で
   `src/main.ts` と各 `src/**/*.test.ts` を入口に metafile を作り、各入口が読む file の集合を得る（`celeris:host` と `vitest` は外部）。
   この計算は `scripts/dev/` の helper（例 `scripts/dev/ts-verify-graph.mjs`）が行い、`control-ts/scripts/` の物を使わない。
   - 変わった file を読む試験だけを「対象の単体」に選ぶ。
   - 次のどれかなら全部の単体試験を走らせる（危険側の skip をしない）: グラフの計算が失敗した、変わった file がどの入口からも読まれない、
     `src/` に式で組み立てる `import(...)` がある、変わった file が試験から読まれない。
   - `control-ts/scripts/*.test.mjs` は `compare.mjs` が式で `src/domains/<domain>.ts` を読むなど、グラフで追えない読み方をするので、`ts-only` でも常に全部走らせる。
   - 今の試験 3 file はどれも `../main` を import するので、今は選択の結果が常に全部になる。規則は試験が増えたときのためにある。
2. fixture を書く Rust: `control-ts/fixtures/**` を書く・読む crate は、`crates/*/{src,examples,tests}` の中で path `control-ts/fixtures/` を参照する crate とする
   （今は `task-core`（example `dump-lane-fixtures`・`dump-route-priority-fixtures`、試験 `script_fixtures`）と `celeris`（`script_compare_*` の試験））。
   fixture の変更ではこれらの crate を cargo の試験の対象に足す（`test-affected.sh` は今 `control-ts/fixtures/` をどの crate にも写さない。wire unit で直す）。
3. crate の逆依存: `rust` の変更の対象 crate は `test-affected.sh` の既存の計算（manifest dir の最長一致 → workspace 内の逆依存の閉包）のまま。
   加えて、cache key (4) の入力が変わったら（`celeris-script`・golden の元の crate・`Cargo.lock`・ルート `Cargo.toml`・schema）、
   TS の段も runner を作り直して走らせる。

**`ts-only` の追加の禁止**（分類の段で検査し、当たれば失敗にする）: 追加した行に `@ts-nocheck`・`@ts-ignore`・`@ts-expect-error`、
`*.test.ts` の追加した行に `.only(`・`.skip(`・`.todo(`。型検査と単体試験を TS 側の編集で黙らせないため。

`ts-verify.sh --list` は分類、file ごとの区分と理由、走らせる段を出す（何も走らせない。cargo も起動しない）。

### (2) TS-only の段
`ts-verify.sh`（引数なし）は `ts-only` のとき次を順に走らせ、最初の失敗で止まる。cargo の起動は 0 回。
各段の秒と exit code は `--record <file>` の JSON に書く。

| 順 | 段（記録の名前） | 中身 |
|---|---|---|
| 0 | `classify` | (1) の分類と追加の禁止の検査。`ts-only` でなければ「従来の経路へ」と出して exit 0 で抜ける（cargo の試験は `test-affected.sh` が走らせる。(6)） |
| 1 | `runner-cache` | `script-runner-cache.sh path` で cache key を計算し、runner と golden を見つける（(4)）。無ければ `ensure` で作る（cargo を起動するので記録の `cargo_invocations` が 1 以上になる）。`--no-build` のときは exit 3 |
| 2 | `install` | `control-ts/node_modules/.modules.yaml` が無いときだけ `pnpm install --offline --frozen-lockfile` |
| 3 | `typecheck` | `control-ts/node_modules/.bin/tsc --noEmit -p control-ts/tsconfig.json`。`package.json` の scripts を通さない |
| 4 | `bundle` | `control-ts/scripts/build-bundle.mjs` の `buildBundle({ outDir: "$work/dist" })`（`$work` は `$TMPDIR` の下の使い捨て dir）。`control-ts/dist` は読まない・使わない |
| 5 | `unit` | (1) の選択で `vitest run --config control-ts/vitest.config.mjs <選んだ試験>` と `node --test control-ts/scripts/*.test.mjs` |
| 6 | `golden` | commit 済みの `control-ts/fixtures/<domain>/*.jsonl` と cache の golden を byte で `cmp` し、`node control-ts/scripts/compare.mjs <domain> --fixture <cache の golden>` を全領域に走らせる（Node で src を直に読む早期の検出。合否の最終判断ではない） |
| 7 | `safety` | `script-host-runner run --suite safety`（(3) の安全シナリオ） |
| 8 | `integration` | `script-host-runner run --suite golden`（本物の deno_core host で、全領域の golden 全件を正順と逆順の 2 回） |
| 9 | `activate` | `script-host-runner activate --bundle-dir "$work/dist" --state-dir "$work/state"`（daemon と同じ `deploy_bundle`・`load_deployed` で配備・load し、selftest と各領域 1 件を呼ぶ） |

- 領域の一覧は runner の `domains` 子 command（Rust の表）から取る。TS 側の一覧（`bundle.config.json`・`src/domains/index.ts`）からは取らない。
- 段 6 の `compare.mjs` は `control-ts/scripts/` に在る。`ts-only` ではこの file は基準から変わっていない（変われば `host-api`）。
  合否の最終判断は段 7・8 の runner がする。
- activate の代用が本物の activate と違う点: DB の `script_generations` の行と `script_domain_modes` の CAS（`compare` への切り替え）は行わない。
  これらは Rust のコードで、`ts-only` の変更では変わらない。cargo の経路の試験（`crates/celeris/tests/script_compare_*`）が受け持つ。

### (3) script-host-runner の CLI
置き場: `crates/script-runtime/src/bin/script-host-runner.rs`（package `celeris-script`）。既存の `manifest`・`selftest`・`ScriptRuntimeHandle` を使い、依存 crate を足さない。
検証専用の道具で、daemon・celerisctl から呼ばない。Rust↔TS の通常経路（同じ process の actor）は変えない。

```
script-host-runner run      --bundle-dir <dir> --fixtures <dir> [--suite golden|safety|all]（既定 all）
                            [--domain <name>]...（絞り込み。付けると出力の complete が false）
                            [--call-timeout-ms <n>]（既定 5000）
script-host-runner activate --bundle-dir <dir> --state-dir <dir>
script-host-runner domains                       採点する領域の表を JSON で出す
```

**採点の表（Rust の source が持つ）**: 領域ごとに、fixture の file 名（`lane_rules/all-features.jsonl`・`route_priority/cases.jsonl`）、
fixture の 1 行から入力を作る規則（`lane_rules` は `{"features": row.features}`、`route_priority` は `row.input`）、
比べる欄（`rule_id`・`proposed`・`condition`／`basis`・`candidates`）、出力の値の検査、最少行数（空の fixture・欠けた fixture を落とす）。

**判定**（境界 ADR 付記 S2-2・S2-4・S2-8 と同じ規則）:
- `match`: 出力が object で、欄の集合が比べる欄と完全に同じで、各欄の `serde_json` の直列化が fixture の同じ欄と byte 一致。
- `mismatch`: 比べる欄のどれかが違う。
- `undeclared_field`（宣言外の出力）: 比べる欄に無い欄がある。`invalid_output`: 欄が欠けた・値の検査に落ちた
  （`lane_rules`: `proposed` が `frontier|standard|cheap` 以外、`rule_id` が空か 128 byte 超、`condition` が 512 byte 超。
  `route_priority`: `basis` が 5 値以外、候補が 256 件超、候補が `{source, model}` の文字列だけでない）。
- `exception`: `ScriptError::Exception`。`timeout`: `Timeout`・`Terminated`・`HeapLimit`。`denied`: `Denied`（安全違反 `denied_capability`・`denied_scope`）。
- 安全違反 `undeclared_candidate`: `route_priority` の候補が入力 `table` のどの列にも無い（一致・不一致に関係なく数える）。
- `order_dependent`: 正順と逆順の 2 回で同じ行の出力が違う（境界 ADR 付記 S2-7 の隠れた入力）。

**安全シナリオ（`--suite safety`、表は Rust の source が持つ）**:
1. manifest の検査: `verify_bundle` に加え、`capabilities` ⊆ （manifest の各領域の `domain_capabilities` の和）。
   manifest の `domains` = 採点の表の領域 ∪ `{host_selftest}` でなければ失敗（採点されない領域・欠けた領域を通さない）。
2. 入力の拒否: 各領域に壊した入力（欠けた軸・未知の level・余計な欄・object でない入力・欄の型違い）を渡し、`exception` になることを確かめる。
   出力を返したら失敗（S2 の TS は不正な入力で throw する契約。Rust は不正な出力を使わないが、TS が黙って受けることを通さない）。
3. 純粋領域の op: `lane_rules`・`route_priority` は空の scope で呼び、`Denied` が 1 件でも出たら失敗。
4. 時間: 1 呼び出しが `--call-timeout-ms` を超えたら `timeout` で失敗。既定 5000 ms は負荷による偽の失敗を避けるための上限で、
   本番の deadline（50 ms）での速さは計測（`call_us` の欄）に出すだけで合否にしない。

**出力**（stdout に JSON 1 個。schema `celeris.script-host-runner/1`）:
```json
{
  "schema": "celeris.script-host-runner/1",
  "command": "run",
  "ok": true,
  "complete": true,
  "bundle_sha256": "<64 hex>",
  "host_api": "1.0.0",
  "schema_sha256": "<64 hex>",
  "domains": [
    { "domain": "lane_rules", "fixture": "lane_rules/all-features.jsonl", "rows": 19683, "passes": 2,
      "matched": 19683, "mismatch": 0, "undeclared_field": 0, "invalid_output": 0, "exception": 0,
      "timeout": 0, "denied": 0, "order_dependent": 0, "safety_violations": {},
      "elapsed_ms": 0, "call_us": { "p50": 0, "p99": 0, "max": 0 } }
  ],
  "safety": [ { "id": "lane_rules/missing_axis", "expect": "exception", "outcome": "exception", "ok": true } ],
  "failures": [ { "kind": "mismatch", "domain": "lane_rules", "line": 1, "pass": 1,
                  "expected": {}, "actual": {}, "error": null } ],
  "elapsed_ms": 0
}
```
`failures` は最初の 20 件まで（件数は `domains` の欄が全部数える）。`activate` の出力は `command: "activate"`、`ok`、`bundle_sha256`、
`deployed_dir`、`selftest`（`ok`）、`probed`（各領域 1 件の判定）を持つ。

**exit code**: 0 = すべて通った（`run` は `complete: true` のときだけ。絞り込みでは 0 でも ts-verify が失敗に数える）。
1 = 採点の失敗（`mismatch`・`undeclared_field`・`invalid_output`・`exception`・`timeout`・`denied`・`order_dependent`・安全違反・安全シナリオの失敗・行数不足）。
2 = 使い方・入出力の誤り（引数・file が無い）。3 = bundle の拒否（manifest の検証・領域の集合・capability・isolate の起動・配備の失敗）。

### (4) cache key・置き場所・golden の無効化
**cache key**（`scripts/dev/script-runner-cache.sh key`。cargo を起動しない）= 次の行を連ねた文字列の sha256:
```
celeris.script-runner-key/1
host_src   <crates/script-runtime/ と golden の元の crate（今は crates/task-core/）とルート Cargo.toml の、git ls-files -co --exclude-standard の各 file の (path, sha256) を path 順に並べた sha256>
cargo_lock <Cargo.lock の sha256>
features   profile=release bin=script-host-runner features=default locked=1
rustc      <rustc -vV の全行（rustc -V の行を含む）>
schema     <docs/api/script-host/host-v1.schema.json の sha256>
```
- 作業ツリーの内容を hash する（未 commit の編集も key に入る）。`crates/script-runtime/vendor/`（deno_core の写し）も含む。
- golden の元の crate の集合は「fixture を書く example の crate と、その workspace 内の通常依存の閉包」。今の `task-core` は workspace 内の依存を持たない。
  `ensure` は cargo を使ってよい段なので、`cargo metadata` でこの閉包を計算し、script の持つ一覧と違えば失敗する（一覧が古くなったまま key が変わらない事故を防ぐ）。
- release profile にする理由: 本番と同じ最適化で走らせるため、また呼び出しが速いため（golden 2 回で約 43,400 呼び出し。
  `script-bench` の p50 は release 0.084 ms・debug 0.27 ms）。既に deps が build 済みの target での release build は 25.1 秒だった（記録 adr-fast）。
- V8 の static library は `Cargo.lock` の `v8` の版で決まる。host の `RUSTY_V8_ARCHIVE`（`~/.cargo/config.toml` の `[env]`）は key に入れない。

**置き場所**: `${CELERIS_SCRIPT_RUNNER_CACHE:-${CELERIS_SCRATCH_DIR:-/local/celeris/data/scratch}/script-runner-cache}/<key>/`。
- 中身: `script-host-runner`（binary）、`golden/<domain>/<file>.jsonl`、`meta.json`（key の各入力の値、runner と golden の sha256、作った時刻、build の秒）。
- 置き場所が git の作業ツリーの中か `/tmp` の下なら exit 2 で拒否する（NFS の worktree に大きな file を書かない、`/tmp` に写しを置かない規則）。
- 同時の `ensure` は `<root>/<key>.lock` の `flock` で直列にし、`<root>/.tmp-<pid>/` に作ってから `rename` する。
- 使うたびに `meta.json` の時刻を更新し、`ensure` のときに最近使った 4 個より古い key の dir を消す（1 個は約 70 MB）。

**作り直しと golden の無効化**（`script-runner-cache.sh ensure`）:
1. key の dir が在り、`meta.json` の runner と golden の sha256 が file と一致すれば何もしない（`path` も同じ照合をする。照合に落ちたら cache が無いものとして扱う）。
2. 無ければ `cargo build --release --locked -p celeris-script --bin script-host-runner`（渡された `CARGO_TARGET_DIR` を使う）で作り、binary を写す。
3. golden を Rust の example で作り直す: `cargo run -q --locked -p task-core --example dump-lane-fixtures`・`… --example dump-route-priority-fixtures` の出力を `golden/` に書く。
4. 作り直した golden と commit 済みの `control-ts/fixtures/` を `cmp` する。違えば失敗（fixture を Rust に合わせて作り直すのは Rust の変更の一部で、`ts-only` の経路では行わない）。
5. key が変われば必ず 2〜4 をやり直す。古い runner・古い golden を別の key で使い回さない。

`ts-verify.sh` は毎回、cache の golden と commit 済みの fixture を byte で `cmp` し（段 6）、runner には cache の golden を `--fixtures` で渡す。
fixture を手で書き換えた変更は `host-api` に分類されるうえ、この `cmp` でも落ちる。

### (5) 選択規則と grader の保護
- 合否を決めるものの置き場:
  | 物 | 置き場 |
  |---|---|
  | 分類規則・段の並び・追加の禁止・TS の import グラフの計算 | `scripts/dev/ts-verify.sh`、`scripts/dev/ts-verify-graph.mjs`（helper） |
  | cache key・作り直し・golden の `cmp` | `scripts/dev/script-runner-cache.sh` |
  | 採点の表（領域・fixture・比べる欄・値の検査・最少行数）・安全シナリオ・exit code | `crates/script-runtime/src/bin/script-host-runner.rs`（とその module） |
  | golden の元 | `task-core` の example（`lane_fixture_jsonl` など Rust の参照実装） |
- `ts-verify.sh` は `control-ts/package.json` の scripts を呼ばない（`tsc`・`vitest` を直に呼ぶ）。領域の一覧は runner から取る。
  `control-ts/` の中で合否に効く物（`scripts/`・`fixtures/`・`bundle.config.json`・設定・lockfile・`src/testing/`）を変えると `host-api` になり、従来の経路へ回る。
- `control-ts/src/` の単体試験は TS 側が持つ補助で、合否の下限ではない。試験を消しても弱めても、golden と安全シナリオ（Rust）で落ちる。
- `test-affected.sh` は `ts-verify.sh` の判断と別に、変わった path を自分で見る: `control-ts/src/**`（業務 TS）以外の path があれば従来の経路も走らせる。
  grader の file（`scripts/dev/ts-verify*`・`scripts/dev/script-runner-cache.sh`・`scripts/dev/test-ts-verify.sh`・`scripts/dev/test-affected.sh`）が変われば、
  `bash scripts/dev/test-ts-verify.sh` と `celeris-script` の cargo の試験を走らせる。
- 試験（`scripts/dev/test-ts-verify.sh`、wire unit）で次を固定する。どれも `$TMPDIR` の写しで行い、cargo を失敗させる偽の cargo を PATH の先頭に置く:
  1. 無害な TS の編集 → `ts-only`、exit 0、記録の cargo 起動 0・`link_count` 0。
  2. 規則を 1 件壊した TS の編集 → exit 1（golden の不一致）。同時に `src/**/*.test.ts` の該当試験を消しても exit 1。
  3. 規則を壊し、`control-ts/scripts/compare.mjs` を常に成功するよう書き換える → `host-api` になり、runner でも exit 1。
  4. `bundle.config.json`・`src/domains/index.ts` から領域を消す → runner の領域の検査で失敗。
  5. `control-ts/fixtures/` を壊れた出力に合わせて書き換える → `host-api`、cache の golden との `cmp` で失敗。
  6. `package.json` の `typecheck`・`test` を `true` にする → `host-api`（ts-verify はこれを読まない）。
  7. `@ts-nocheck` の追加・`.only(` の追加 → 分類の段で失敗。
  8. Host API の file（`host.d.ts`・schema）の変更 → `host-api` で cargo 側を走らせる。
- 限界: branch 自身の `scripts/dev/` を書き換える変更は、この試験だけでは防げない（走るのはその branch の script）。
  その変更は `ts-only` にならず従来の経路を通り、差分は reviewer に渡り、release gate の全体試験は選択規則を使わない。
  自己改善の pilot（S6）の WorkUnit には daemon 側の範囲 check（`^control-ts/src/` だけを許す）を置くことを提案する（記録 adr-fast の提案）。

### (6) Host API・Rust の変更と全体試験
- `host-api`: `test-affected.sh` の既存の計算で `celeris-script` とその逆依存（`celeris`・`celerisctl`）の nextest と doc test、
  control-ts の型検査と全部の単体試験、fixture の変更なら (1)-2 の crate を走らせる。key が変わるので TS の段は runner を作り直して走らせる。
- `rust`: `test-affected.sh` の既存の経路のまま。key の入力が変わったときは TS の段も runner を作り直して走らせる。
- `none`: 何も走らせない（今の `test-affected.sh` と同じ）。
- release gate（`release.sh`）・その全体試験は変えない・減らさない。nightly の gate も減らさない（この repo に nightly の gate が在るかは未確認）。
  `ts-verify.sh` は全体試験の代わりにならない。release gate に bundle の作成と runner を足す案は S2 の提案 1 のとおり別 task。

### (7) 計測の定義
- edit→activate: 編集を書いた時点から、段 9（`activate`）が終わった時点までの壁時計。編集を戻す時間は含めない。
  段ごとの秒（`classify`・`runner-cache`・`install`・`typecheck`・`bundle`・`unit`・`golden`・`safety`・`integration`・`activate`）を別の欄に出す。
  型検査と bundle の秒は境界 ADR (f) のとおり別の欄にする。
- 編集課題 `policy-ts`: S0 の policy と同じ形で、`control-ts/src/domains/lane_rules.ts` の規則表に、どの入力にも当たらない規則を 1 件足す（golden は変わらない）。
- warm: `node_modules` が在り、runner の cache が当たる状態。最初に編集なしで 1 回（rep 0、`kind=prime`）回して温め、集計に入れない。
- cold: 試行ごとに `control-ts/node_modules` を消し、offline の store から `pnpm install --offline --frozen-lockfile` で入れる所から測る。
  runner の cache は当たる状態で測る（runner は Rust の入力でしか作り直さないため）。runner の作り直しの秒は `kind=prime` の行に `runner_build_secs` として別に記録し、報告で並べる。
- 試行数: warm・cold それぞれ 20 試行。同じ host・同じ編集課題で、他の計測や CPU を焼く負荷と同時に走らせない。
- 集計: p50 と p95 は最近傍順位（昇順に並べ、p50 は 10 番目、p95 は 19 番目）。
- 目標（境界 ADR (f)）: warm p50 ≤ 30 秒かつ p95 ≤ 60 秒、cold p95 ≤ 120 秒。全試行で exit 0、cargo の起動 0 回、`link_count` 0。
  cargo の起動は PATH に置く数える wrapper で、`link_count` は S0 と同じ linker の wrapper で数える。1 試行でも外れたら目標未達とする。
- 比べる相手: S0 の baseline（policy warm 中央値 221.0 秒、cold 283.5 秒）。段ごとに並べる。数値は直さず、未達は未達と書く。

## 境界 ADR との関係
次の点は境界 ADR と矛盾しない。違いのある点は close unit が境界 ADR に付記する（この unit の範囲は境界 ADR を含まない）。
- (b) の「入れないもの」: runner は別 process だが、検証だけの道具で daemon の経路に無い。Node は境界 ADR のとおり開発と検証の段だけで使う。矛盾しない。
- (f) の「cargo の compile・link 0 回」: (2) の段と (7) の計測で守る。runner の作り直しは Rust の入力が変わったときだけで、別の行に記録する。
- (f) の edit→activate の定義は「generation の load と `compare` での有効化」を含む。この ADR の activate は一時 state_dir への配備と load で代用し、
  DB の generation の行と mode の CAS は含めない（(2) の末尾）。**違い 1**: 付記で「TS だけの変更の計測では activate を配備と load で代用する」と明記する。
- 付記 S2-7 は「許可表が空の領域の manifest が `capabilities` を宣言していたら load で拒否する」とする。`crates/celeris/src/daemon/script*.rs` と
  `crates/script-runtime/src/manifest.rs` を grep した範囲では、この拒否は見つからなかった（実効の権限は `Grant` で manifest ∩ 許可表に絞られる）。
  manifest の `capabilities` は全領域の和で、領域ごとの宣言を持たない。runner は (3) の安全シナリオ 1 で「和 ⊆ 許可表の和」を検査する。
  **違い 2**: daemon 側に同じ検査が在るかは未確認。close unit が確かめて付記に書く。
- 付記 S2-12 (4)（dist が無いと試験が SKIP）: この経路は必ず `$TMPDIR` に bundle を作り直すので、古い dist の影響を受けない。
- runner の値の検査は compare の判定（`task-dispatch` の `classify_lane_rules`・`classify_route_priority`）と同じ上限の写しになる。
  `celeris-script` は `task-dispatch` に依存できない（境界 ADR (b) の V8 の link の制約の逆向き）。一致を固定する試験は `crates/celeris/tests` に置く必要があり、
  S3 の unit の範囲に無い（記録 adr-fast の提案）。

## 不採用
- 変わった file 名の表だけで単体試験を選ぶ: import の先の変更を見落とす。(1) のグラフと、追えないときの全部実行にした。
- runner を debug build にする: 呼び出しが約 3 倍遅く、本番の profile と違う。
- runner の build を毎回 cargo に任せる（cargo の no-op build で済ませる）: cargo の起動 0 回の目標に反し、fingerprint の確認だけで秒がかかる。
- golden を TS 側（`compare.mjs`）だけで比べる: `control-ts/` の中の物が合否を決めることになる。最終の判断は runner（Rust）にした。
- `control-ts/dist` を使う: git 管理外で古いことがある（S2 の stale-dist）。

## 後続
- runner unit: `script-host-runner`（(3)）と試験 `crates/script-runtime/tests/script_host_runner.rs`（一致・不一致・例外・宣言外の 4 場合）。
- verify-sh unit: `ts-verify.sh`（(1)(2)、`--list`・`--record`・`--no-build`）、`script-runner-cache.sh`（(4) の `key`・`path`・`ensure`）と 2 つの試験。
- wire unit: runner への接続、`test-ts-verify.sh`（(5) の 8 場合）、`test-affected.sh` の委譲と fixture の crate への写し。
- bench・measure unit: (7) の `policy-ts` と `--check-targets`。

## 付記 wire（2026-11-02、unit wire の実装との突き合わせ）
- **wire-1（unit 段から `test-affected.test.mjs` を外す）**: (2) の段 5 は `control-ts/scripts/*.test.mjs` を全部走らせるとしたが、
  `test-affected.test.mjs` は fixture の中で `scripts/dev/test-affected.sh` を走らせ、`cargo metadata` を起動する。
  ts-only の経路で cargo の起動 0 回（(2)(7)）と両立しないため、`ts-verify.sh` の段 5 はこの 1 file だけを除く。
  この試験の対象（`scripts/dev/test-affected.sh`）は ts-only では基準から変わらない。`scripts/dev/` か `control-ts/scripts/` の変更
  （rust・host-api）で `test-affected.sh` の control-ts の段（`pnpm` の `test`）が従来どおり走らせる。
  verify-sh の記録の未解決 (a)（目標を compile の起動 0 回に緩める）は採らない。
- **wire-2（`test-affected.sh` の委譲）**: `test-affected.sh` は ts-verify の分類と別に、変わった path を自分で見る
  （`git diff --name-status --no-renames` と未追跡 file。中立の `agent-docs/`・`docs/`（`docs/api/` を除く）以外がすべて
  `control-ts/src/**/*.ts` で、`src/testing/`・`src/host.d.ts`・試験の削除を含まない）。当たれば cargo を起動せず
  `bash scripts/dev/ts-verify.sh --record <file>` に委ね、記録の `class` が `ts-only`・`exit` が 0 であることを確かめる
  （ts-verify が「従来の経路へ」で何も走らせずに抜けた場合を通さない）。
  それ以外では、`control-ts/`・`docs/api/script-host/` の変更に `celeris-script`（と逆依存）を、`control-ts/fixtures/` の変更に
  fixture を参照する crate を cargo の試験の対象に足す。runner の cache key の入力か検証の道具の変更では `bash scripts/dev/test-ts-verify.sh` も走らせる。
- **wire-3（試験）**: `scripts/dev/test-ts-verify.sh` は (5) の 8 場合を 12 件の確認で固定する。規則を壊した編集は ts-only の経路では
  先に段 5（TS の単体試験）で落ちるので、同じ壊れた bundle を runner（`run --suite golden`）に直に渡し、Rust の採点だけでも
  `mismatch` で exit 1 になることを別に確かめる。`compare.mjs`・fixture・`package.json`・`bundle.config.json` の編集は host-api になることと、
  領域を消した bundle を runner が exit 3 で拒否することを確かめる。

## 付記 close（2026-11-02、unit close の実装との突き合わせ）
(1)〜(7) を `scripts/dev/ts-verify.sh`・`scripts/dev/script-runner-cache.sh`・`scripts/dev/test-ts-verify.sh`・`scripts/dev/test-affected.sh`・
`scripts/dev/edit-activate-baseline.sh`・`crates/script-runtime/src/bin/script-host-runner.rs` と突き合わせた。下に挙げない決定は本文のとおり実装されている。
本文と違う点は、この付記を正とする。実際に走らせた結果は [S3 の進捗](../progress/2026-11-02-ts-control-layer-s3.md)。

- **close-1（(1)-1・(5) の helper）**: `scripts/dev/ts-verify-graph.mjs` は無い。import グラフの計算は `ts-verify.sh` が `$TMPDIR` の使い捨て dir に書く
  heredoc（`graph.mjs`）で行い、規則は (1)-1 のとおり。合否を決める物が `scripts/dev/ts-verify.sh` にあることは変わらない。
- **close-2（(1)-1 の cold の単体試験）**: 単体試験の選択は段 `classify` で行う。`node_modules` が無い cold では import グラフを計算できないので、常に全部を走らせる
  （「計算が失敗したら全部」の規則どおり）。
- **close-3（(3) の runner の細部）**: 判定に `host_error`（`Busy`・`GenerationGone`・`InputTooLarge`。失敗に数える）と `too_few_rows` を足した。
  `route_priority` の候補は `{source, model}` の順に組み直してから比べる（`classify_route_priority` が DTO で比べるのと同じく、欄の順を見ない）。
  安全シナリオの行は manifest 2 件と壊した入力 15 件（`lane_rules` 7・`route_priority` 8）の計 17 件。シナリオ 3（空の scope）と 4（時間）は別の行を持たず、
  各呼び出しの判定 `denied`・`timeout` で数える。exit 3 でも stdout に `{ok:false, exit, error}` を出す。
- **close-4（(4)）**: `script-runner-cache.sh` に key の元を出す子 command `inputs` を足した。1 個の dir は 74 MB（`du -sh`）、`ensure` の `build_secs` は 60.3 秒だった。
  (4) の末尾の「手で書き換えた fixture は `cmp` でも落ちる」は段 6 の話ではない。fixture の変更は `host-api` なので `ts-verify.sh` は段 0 で exit 0 で抜け、段 6 に届かない。
  検出するのは `test-affected.sh` が対象に足す `task-core` の試験 `script_fixtures`（exit 100 を確かめた）と、key が変わったときの `ensure` の `cmp` である。
- **close-5（(5) の試験の 8 場合）**: `test-ts-verify.sh`（12 件）の確認は本文より狭い。足りない部分は unit close が写しで 1 回だけ確かめた（試験には入っていない）。
  | 場合 | 試験が確かめること | 本文との違いと、写しで確かめた結果 |
  |---|---|---|
  | 2 | 規則を壊した編集の exit 1 と、同じ bundle の runner の mismatch | 「試験を消しても exit 1」は無い。試験 file を消すと `host-api` になり、`ts-verify.sh` は exit 0 で「従来の経路へ」と出す。このとき `compare.test.mjs`・`compare.mjs`・runner はどれも exit 1（1,458 件の mismatch）。試験の中身だけを無害に書き換えると `ts-only` のままで、段 unit の `compare.test.mjs` により exit 1 |
  | 3 | `compare.mjs` の書き換えで `host-api` | runner の exit 1 は場合 B が確かめる（同じ壊し方） |
  | 4 | `bundle.config.json` から消すと runner が exit 3 | `src/domains/index.ts` から消す編集は試験に無い。これは `ts-only` で、段 unit で exit 1。同じ bundle の runner は領域の検査ではなく `exception` 2,026 件の exit 1（manifest の領域の集合は `bundle.config.json` から作るため exit 3 にならない） |
  | 5 | `host-api` と `cmp` の違い | 試験は `cmp` を直に呼ぶ。`ts-verify.sh` 自身は close-4 のとおり exit 0 で抜ける |
- **close-6（(6) の `host-api`）**: 「key が変わるので TS の段も runner を作り直して走らせる」は、key の入力（`docs/api/script-host/host-v1.schema.json`・`crates/script-runtime/`・`crates/task-core/`・
  `Cargo.lock`・ルート `Cargo.toml`）か検証の道具（`scripts/dev/` の 4 file）が変わったときだけ成り立つ。そのとき走るのは `test-ts-verify.sh` で、作業ツリーの写しに無害な編集を足して 10 段を通す。
  `control-ts/src/host.d.ts`・`control-ts/scripts/**`・`control-ts/fixtures/**`・`control-ts/package.json` などの変更では key が変わらず、`test-ts-verify.sh` は走らない。
  走るのは `celeris-script` と逆依存の cargo の試験と control-ts の型検査・単体試験（fixture では `script_fixtures` の crate も）である（進捗の未解決に残す）。
- **close-7（(6) の nightly）**: nightly の gate はこの repo で見つからなかった（`git grep -il nightly -- scripts config` の当たりは `edit-activate-baseline.sh` の「nightly を使わない」という注記だけで、`.github/` は無い）。全体試験の gate は `scripts/selfdeploy/release.sh` だけで、
  `git diff` で `scripts/selfdeploy/` に差分が無いことを確かめた。
- **close-8（(7) の prime と記録の形）**: warm の prime は「編集なしで 1 回」ではなく、`step=runner-ensure` と、編集を当てて 1 回走らせる `step=ts-verify` の 2 行にした。編集なしでは ts-verify が分類 `none` で抜け、何も温まらないため。
  cold の prime は `step=runner-ensure` の 1 行。cache が当たれば `runner_build_secs` は 0.2 秒ほどで、作り直しの秒ではない（作り直しは wire の 60.6 秒）。
  記録は S0 の 1 行 1 段と違い、1 試行 1 行（`kind=trial`）で段の秒を `stages` に持つ。cargo と link の数が試行全体の値で、段ごとに分けられないため。
  `--check-targets` は試行数の下限を持たない。20 試行であることは `modes.<mode>.trials` で確かめる。
- **close-9（「境界 ADR との関係」の違い 1・2）**: 違い 1（activate の代用）と違い 2（daemon の検査）は境界 ADR の [付記 S3](2026-11-02-ts-control-layer-boundary.md) に書いた。
  違い 2 の答え: daemon・`manifest.rs` に S2-7 の拒否は無い。領域の値の検査の写しの一致を固定する試験も無い。手で比べると `task-dispatch::script_policy` の上限（`RULE_ID_MAX` 128・`CONDITION_MAX` 512・`ROUTE_PRIORITY_CANDIDATES_MAX` 256）と本文 (3) の値は同じである。
