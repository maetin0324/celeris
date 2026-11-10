---
title: 健全性v2 R5 inventory — Cargo の test target の分類と束ねの案
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 inventory — Cargo の test target の分類と束ねの案

完了日: 2026-11-10

WorkUnit `inventory`。workspace の全 test target（cargo metadata の kind=test）を 1 件ずつ分類し、担当 leaf と束ね先を決めた。基準は `git merge-base HEAD main`（生成時。`inventory.json` の `base_sha`）。repo のコードは変えていない。

## 変更
- 件名「R5 inventory: test target の分類台本 classify.py と inventory.json」の commit — [classify.py](classify.py)・[inventory.json](inventory.json)・この file を追加した。

## 呼び方
- 生成: `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --write agent-docs/progress/2026-11-10-health-v2-r5/inventory.json`（`--base REV` を省くと `git merge-base HEAD main`。base の tree と、その時点の R2〜R4 の branch を読む。cargo metadata の kind=test の集合と ls-tree の集合が違えば止まる）。
- 検査: `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json`。base_sha の tree の workspace member の `tests/*.rs` が過不足なく 1 回ずつ載ること、欄の型、base だけで決まる欄（支援 module・fixture・process global・外からの参照・file 名の数字・capability）が base_sha の再走査と一致すること、下の leaf の規則と束の規則を確かめる。
- leaf の確認: `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf <leaf> agent-docs/progress/2026-11-10-health-v2-r5/inventory.json`。その leaf の target が `tests/<bundle>/<target>.rs` にあり元の path に無いこと、`tests/<bundle>/main.rs` が `mod <target>;` を宣言すること、keep と r6 の target が元の path のままであることを作業ツリーで見る。他の leaf の target はどちらの状態でもよい。

## 結果（352 target）

| crate | target | 束ねる leaf（target → 束） | adjust | keep | r6 |
|---|---|---|---|---|---|
| task-api | 113 | bundle-api-a 21 → 4、bundle-api-b 49 → 9 | adjust-api 5 → 2 | 18 | 20 |
| celeris | 55 | bundle-celeris 16 → 4 | — | 28 | 11 |
| celerisctl | 18 | bundle-ctl 9 → 2 | — | 5 | 4 |
| task-worker | 21 | — | adjust-worker 9 → 4 | 12 | 0 |
| ほかの 11 crate | 145 | — | — | 145 | 0 |

- 段 bundle の 4 leaf で 95 target が 19 束になる（task-api の binary は 113 から 56 へ、celeris は 55 から 43 へ、celerisctl は 18 から 11 へ）。段 final の adjust を足すと task-api は 53、task-worker は 21 から 16 になる。
- process global の操作を持つ target は 59 件（current_exe 33・mutable_static 19・fixed_port 14・signal 9・process_exit 5・env_mutation 3・fixed_path 3。重複あり）。すべて keep。
- 外からの名前参照: scripts/ の `--test`・`-E` と `.config/nextest.toml` の `binary_id` で呼ばれる suite が 16 件（keep）。`docs/architecture-map.md` が path を挙げる target が 63 件（keep の理由が無い 35 件が r6）。追跡 .md の Markdown リンクで参照される target は 0 件だった。
- file 名に数字を持つ target は 44 件（B0 R5 の write set の `tests/[a-z_/]+\.rs` を外れ、動かせない。keep）。
- 別の target の file を `#[path]` で取り込む、または取り込まれる target は 5 件（`local_machine_settings` と `local_settings_admin`、`llm_routes_admin` と `llm_accounts_admin`・`llm_sources`）。keep にした。
- R2〜R4 と競合する target は 12 件。task-api の 5 件は adjust-api、残りの 7 件は keep の理由（file 名の数字・named suite・current_exe・束ねる leaf の無い crate）か r6（`cos_triage`）を先に持つ。task-worker は R2 の write set と interface が全 target を覆うので、keep でない 9 件を adjust-worker にした。

## 規則（inventory.json の `rules` と同じ）
- leaf は次の順で最初に当たったもの。理由は `leaf_reasons` にすべて残す。
  1. keep: process global の操作、名前で呼ばれる suite、file 名の数字、他の target と source を共有、R5 に束ねる leaf の無い crate、adjust leaf の無い crate での R2〜R4 との競合、束ねる相手の無い 1 件だけの組（`singleton_bundle_group`）。
  2. r6: architecture-map の path か、この進捗 directory の外の追跡 .md のリンクで参照される。
  3. adjust-api・adjust-worker: R2〜R4 と競合する task-api・task-worker の target。task-worker は競合が無くても adjust-worker（段 bundle に task-worker の leaf は無い）。
  4. bundle-api-a（task-api で名前が `cos_`・`chat_`・`general_cos`・`project_` で始まる）、bundle-api-b（他の task-api）、bundle-celeris、bundle-ctl。
- 受け入れ条件 1 と 2 が同じ target で重なるとき（例: `browser_e2e` は current_exe と R2 の競合を持つ）は keep・r6 を先にした。keep・r6 の target は動かさないので、R2〜R4 の変更と衝突しない。
- process global の検出は、target の file と `mod`・`#[path]` で取り込む全 file（tests/ の外の `crates/campaign/tests/preflight_setup/mod.rs` を含む）の、注釈行を除いた本文の正規表現。fixed_port と fixed_path は bind・connect・listen・fs 操作・`Path::new`・`const` の行だけを数える（`HOST = "127.0.0.1:7710"` を Host header や config の表示に使うだけの task-api の `common` は数えない）。`listen = "127.0.0.1:17998"` の TOML は daemon が bind しうるので数える。
- 競合: R2〜R4 の branch（`celeris/<id>`・`celeris-wu/<id>/*`）の `main...branch` の変更 file に target かその支援 module が入る、slice の ADR（その branch で変わった `agent-docs/adr/*.md`）が target を名指す、または（R2 だけ）R2 が変える interface（`EventSink`・`WorkerAdapter`・`BrowserPorts`・`run_with_executable_record`・`run_with_candidates`・with_* 5 つ）を target が名指す。write set の一致は記録するが、それだけでは競合にしない（R3 の write set は全 crate の `tests/` を覆う）。名前に `-superseded-` を含む branch は退避された旧 WU branch なので、変更 file を `superseded_branch_changes` に記録し、競合に数えない。
- 束: 同じ crate・leaf・`capability_key`・`fixture_key` の target だけを束ね、1 束は 8 件まで（件数を均す。18 件は 6+6+6）。`capability_key` は B0 の capability group（cap-*）。capability-map の tests に path があればその capability、無ければ file 名の token と capability の id・owner module の token の一致が最大の capability（先頭の token を 2 倍に数える。`capability_source` に `capability-map:` か `name_token:` を書く）。一致が無ければ `group:none`（束の名前は `unmapped_*`）。束の名前は crate の中で一意で、既存の target 名・tests/ の directory 名と重ならない。

## 検査（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 target、base_sha の tree も 352、25 束、0 error）。
- `classify.py --verify-leaf keep …` と `--verify-leaf r6 …` → exit 0（keep 208 件・r6 35 件の計 243 件が元の path にある）。作業ツリーで bundle-ctl の 9 件を `tests/<bundle>/<target>.rs` へ動かし `main.rs` に `mod` を書くと `--verify-leaf bundle-ctl` は exit 0、さらに keep の `worker_run.rs` を動かすと exit 1 になることを確かめ、作業ツリーは `git checkout` で戻した。未知の leaf 名は exit 2。`--verify-leaf bundle-api-a …` → exit 1（まだ動かしていないので、21 件それぞれが「元の path にある」「束の file が無い」「`mod` が無い」で落ちる。束ねの leaf が終えると 0 になる形）。
- `git diff --name-only $CELERIS_WU_BASE` と未追跡の file → この directory の 3 file だけ。`python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 …` → exit 0。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし」。Rust の変更なし）。`bash scripts/dev/check-health.sh` → exit 0（doc-only）。`cargo clippy --workspace -- -D warnings` → exit 0（95 秒）。`cargo fmt --all -- --check` → exit 0。

## 未解決事項
- 静的な走査の限界: 支援 crate（task_core など）の関数の中で行う process global の操作、実行時に組み立てる port や path は検出しない。束ねの leaf は束ねた後に nextest で束の試験を走らせて確かめる。
- 競合は生成時の branch の状態の写しで、R2〜R4 が進むと変わる。段 final の sync-main は main に入った R2〜R4 の差分で `--write` をやり直して比べる。
- R3 の `port-fix-files.json`（8 method を動かす試行の file 集合）が挙げる試験のうち、`celeris` の `daily_curation`・`notify`・`variant_propagation`、`celerisctl` の `org_migrate_v2`、`task-api` の `approvals`、`task-ops` の `browser_policy_autoattach` は、計画 v2（6 method）の mcp-port の差分に無いので競合に数えていない。このうち `variant_propagation` は bundle-celeris、`approvals` は bundle-api-b に入る。R3 の再実装がこれらに触れたら sync-main で直す。
- `capability_key` を capability group（5 組）に上げたのは、B0 の capability-map の tests に載る target が 58 件だけで、capability 単位では束の大半が 1 件になり link の回数が減らないため。束の粒度は段 design の ADR で決め直してよい。

## 提案
- `celeris-campaign` の 111 target は R5 の scope（task-api・celeris）の外なので keep にした。link の回数を減らす効果が最も大きいので、別の task で同じ台本を使って束ねる。
- `local_machine_settings` は `local_settings_admin.rs` を `#[path]` で取り込み、同じ試験を 2 つの binary で走らせている。`llm_routes_admin` と `llm_accounts_admin`・`llm_sources` も同じ形。dedupe-api で、取り込みを束の中の 1 回に直す候補にする。
