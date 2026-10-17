---
title: "MLSys P0-3: 試行に渡る文脈・KB・記憶・履歴・読める path の棚卸し"
tasks: [01M54HYGQPFNRX6PNPJJG475EB]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# P0-3 棚卸し: 試行の文脈・knowledge・記憶・履歴・filesystem 参照経路

WU `audit-paths`（読むだけ。コードは変えていない）。基準: GitHub main `08127c23d5d3`（この WU の base と同じ HEAD）。
読んだ箇所: `crates/task-dispatch/src/dispatcher/run_context.rs`（`run_extras` / `knowledge_context` / `skills_context` /
`session_summary`）、`dispatch_run.rs:766`（worker・planner・WU・子 task 共通の入口）、`review_spawn.rs:1031-1062`
（`pick_reviewer`）、`worker_finish.rs:1498`（`absorb_memory`）、`task-ops/src/knowledge.rs`（`resolve_root`・`search`・
`list_pages`）、`celerisctl/src/commands/knowledge.rs`（`root_of`）、`task-worker/src/db_guard.rs`、`tool_policy.rs`、
`campaign/src/{runner.rs build_seed・prepare_snapshot, isolation.rs, harness_exec.rs Sandbox, celeris_exec.rs body/check_template, preflight.rs, main.rs}`。

分類の記号:
- **構成** = 測定専用の daemon/config/state（別 `--config`・別 DB・別 KB root・別 org seed）と preflight/runbook で保証できる
- **修正** = コード修正が要る
- **人** = 人の決定が要る（A の claude HOME 等）
- **済** = 既に解消（commit と試験を示す）

## 1. 経路別: 何が文脈に入り、knowledge をどこから取り、何が読めるか

全ての worker 系 run（Execute の worker、planner（`TaskKind::Plan` と execution planner）、WU、木の子 task）は
`dispatch_run.rs:766` で同じ `run_extras` を通る。違いは planner に `execution_planner` 文脈（自 task の events・
work_units だけ。`planner_flow.rs:711-723`）が足されること、子 task（`task.tree` あり）は `available_genres`/組織図が
空になることだけ。reviewer だけ別経路（`pick_reviewer`）。

| 項目（参照点） | worker（A 以外）/ WU | planner | 子 task（木の節点） | reviewer | A（harness 直、bwrap） |
|---|---|---|---|---|---|
| KB 索引の前置き `knowledge_context`（`run_context.rs:626`） | 出る。根 = `config.knowledge.root`、mount = 担当 profile の `knowledge` ∪ `[knowledge] default_mounts`（既定 `kb:user`,`kb:environment`）∪ 案件の `projects/<slug>` | 同左 | 同左 | **出ない**（RunContext に knowledge を入れない） | 無し（Celeris の前置きを通らない） |
| KB 内の `dir:` / `repo:` / `memory` mount | profile/default_mounts に書けば出る（`memory` は `memory_dir/<node>/notes.md`） | 同左 | 同左 | 無し | 無し |
| worker 内の `celerisctl knowledge search/get` | 根 = `--root` > `CELERIS_KNOWLEDGE_ROOT` > `CELERIS_CONFIG` の `[knowledge] root` > **`~/.config/celeris/config.toml`**（`knowledge.rs:157-170`）。scope 指定は任意で**根全体を検索できる**（mount で絞らない）。`_inbox/` は検索対象外（`list_pages:391`）だが `get` は path で読める | 同左 | 同左 | 同左（道具として使えば） | `celerisctl` は sandbox に無い（bind されない） |
| skills `skills_context` | `config.knowledge.root/skills/<name>`（担当 profile の `skills_mounts`、Work 用） | 同左 | 同左 | 同 root、対象 task 担当の `skills_mounts`（Review 用） | 無し |
| 記憶 `memory`（`run_context.rs:73`） | `[memory]` があり担当がいれば `MemoryDir::load(node, project)`＝`notes.md`（案件横断）＋`projects/<project_id>.md` | 同左 | 同左 | 無し | 無し |
| 記憶の書き込み `absorb_memory`（`worker_finish.rs:1498`） | result.json の `memory` を担当×案件に**追記** → 次の試行（他 arm・他反復、同じ担当・同じ campaign 案件）の前置きに出る | 同左 | 同左 | 無し | 無し |
| 会話履歴 `conversation`（`run_context.rs:79`） | 担当がいれば `message_list(node, project_id, CONVERSATION_HISTORY)` を**対話でない run にも**載せる | 同左 | 同左 | 無し | 無し |
| 永続の認可 `standing_rules` | 担当宛て＋全員向け（DB） | 同左 | 同左 | 無し | 無し |
| 組織図 / 分野一覧 | 委譲できる run だけ | 出る | 出ない | 部署 node と profile | 無し |
| comments / children / milestone | 自 task のものだけ（他試行は混ざらない） | 同左 | 同左 | 自 task | 無し |
| CoS session / recent_work / active_projects / clusters | 対話・CoS だけ（試行 task は対象外） | — | — | — | — |
| Claude Code の session 永続 | 既定 `--no-session-persistence`。WU・atomic の continuation session は自 task の中だけ | 同左 | 同左 | 同左 | 毎試行新規 |
| Claude Code 自身の自動記憶・CLAUDE.md（`CLAUDE_CONFIG_DIR` の中） | アカウントの `CLAUDE_CONFIG_DIR` を使う。そこにある `CLAUDE.md`・`projects/<slug>/memory`・過去の transcript が Claude Code 自身により読み込まれ得る（Celeris は抑止していない） | 同左 | 同左 | 同左 | **同じ問題**: `Sandbox.config_dir`（Celeris 管理 claude_oauth アカウントの dir）を **rw bind** |
| 読める filesystem | 同じ uid で host 全体（`db_guard` は DB dir と `$HOME` の固定設定 dir を読み取り専用にするだけ。`tool_policy` は subagent 禁止だけで Read を制限しない）→ 通常 KB root・`~/.local/celeris/memory`・他試行の `trials/**`・`--grader-root`・campaign out dir が**読める** | 同左 | 同左 | 同左 | bwrap: `/usr` `/etc` ro・`/tmp` tmpfs（`HOME=/tmp/home`）・workspace と account dir rw・実行ファイルのみ（`harness_exec.rs:191`） |
| 試行の初期資料 | workspace の `materials/`・`kb/`（runner が seed から作る。prompt には kb/ の存在を書かない） | 同左（作業場所を継ぐ） | 同左 | 作業場所 | 同左 |
| knowledge の書き込み `celerisctl knowledge record` | 解決した根の `_inbox/` に書く（daemon 共有。試行間で残る） | 同左 | 同左 | — | 不可 |
| langmem 整理 / cron の KB 整理（ADR-0131） | `[knowledge.langmem]`・cron job が有効なら試行完了後に KB を書き換え得る | — | — | — | — |

## 2. 参照点ごとの分類

| # | 参照点 | 汚染源 | 分類 | 保証の仕方 / 修正内容 |
|---|---|---|---|---|
| R1 | `knowledge_context` の根 `config.knowledge.root` | 通常 KB | **構成** | 測定 daemon の config で `knowledge.root` を測定専用 dir にする。本番 KB を指さないことを preflight で検査 |
| R2 | `default_mounts`（既定 `kb:user`,`kb:environment`）・profile `knowledge`・案件 `projects/<slug>` | 通常 KB / 試行横断 | **修正**（構成だけでは足りない） | 測定 root を空にすれば汚染は防げるが、試行用 KB（`<ws>/kb`、試行ごとに path が違う）を B/C の前置きと `knowledge search` に**接続できない**（静的 config に試行ごとの path を書けない）。→ 試行 task（label `campaign-<hex>`。`celeris_exec::is_campaign_label` と同じ判定）では mount を `dir:<workspace>/kb` だけにし、default/profile/project mount を外す |
| R3 | worker の `celerisctl knowledge` の根解決 | 通常 KB（`~/.config/celeris/config.toml` に落ちる） | **修正**＋構成 | 試行 run の env に `CELERIS_KNOWLEDGE_ROOT=<workspace>/kb` を入れる（R2 と同じ判定）。構成側: 測定 daemon を `CELERIS_CONFIG=<測定 config>` 付きで起動（worker は env を継ぐ）。現状 celeris は `CELERIS_CONFIG`/`CELERIS_KNOWLEDGE_ROOT` を worker に渡さない（`task-worker`/`task-dispatch` に設定箇所なし） |
| R4 | `skills_context`（root/skills） | 通常 KB の skills | **構成** | 測定 root に置く skills を A/B/C 共通の固定集合（または空）にし、org seed の `skills_mounts` を同じにする。A は skills を受けないので、比較の「利用可能ツール」を揃えるなら測定 org の `skills_mounts` を空にする（preflight で org を検査） |
| R5 | `memory`（読み）・`absorb_memory`（書き） | 他 arm/他反復の成果（同じ担当×同じ campaign 案件に追記される） | **構成**（推奨）／修正（任意） | 測定 config に `[memory]` を書かない → `memory_dir = None`（`config/dispatch.rs:167`）で読み書きとも止まる。preflight で `[memory]` 無しを検査。保険として試行 task では読み書きを止めるコード修正も可（fix-context で R2 と同じ判定に乗せる） |
| R6 | `conversation`（担当×案件の messages） | chat 履歴 | **構成** | 測定 DB は空から作り、campaign 案件で対話しない。preflight で campaign 案件の messages 0 件を検査。保険として試行 task では空にするコード修正も可 |
| R7 | `standing_rules` | 通常運用の認可 | **構成** | 測定 DB は空 → 0 件。preflight で検査 |
| R8 | worker の filesystem 読み取り（host 全体） | 通常 KB・memory・他試行 `trials/**`・`--grader-root` | **構成**（path を離す）＋**人**（B/C に bwrap 相当の隔離を足すか） | 汎用権限基盤は足さない方針なので、(a) 測定 daemon の KB root・memory・state を本番と別 dir、(b) campaign out dir と `--grader-root` を B/C worker から推測しにくい場所に置くだけでは「読めない」保証にならない。保証するには B/C worker の process を bwrap で包む（A と同等）コード修正が要り、範囲が大きい → 人の決定 D2 |
| R9 | `--grader-root` 既定 `crates/campaign/fixtures/grader-private`（repo の中） | grader 私有 | **修正**（preflight） | `snapshot::copy_tree` は withheld を見ない。repo-src / materials-src / kb-src / B/C 案件の repo の中に grader root があると試行 workspace に複製・worktree に出る。preflight で「grader root がこれらの下に無い」ことを検査する |
| R10 | A の `CLAUDE_CONFIG_DIR`（アカウント dir を rw bind） | その dir の `CLAUDE.md`・自動記憶・過去 transcript（B/C・本番の run が同じアカウントを使えば他試行・通常運用の履歴） | **人** | 選択肢: 測定専用 claude_oauth アカウント dir（transcript/memory 無し、測定でだけ使う）／試行ごとに資格情報だけ写した使い捨て dir（OAuth の refresh で元の資格情報が失効し得る）。決定 D1 |
| R11 | B/C の Claude Code が使うアカウント `CLAUDE_CONFIG_DIR` | 同上（Claude Code の自動記憶は git root 単位で前置きされる。同じ trial path を作り直すと前回の記憶が残る） | **人**＋構成 | 測定 daemon のアカウントを測定専用にし、試行前に `projects/` 下が空であることを preflight で検査。自動記憶を設定で切れるか（Claude Code の設定名）は未確認 → 次葉で一次情報を確かめる |
| R12 | `knowledge record` の `_inbox/`・langmem・cron KB 整理 | 試行の書き込みが後続試行へ | **構成** | 測定 config で `[knowledge.langmem]` 無効・KB 整理 cron 無し。R2/R3 の修正後は record も `<ws>/kb/_inbox` に落ちる（試行内で閉じる。`finish_trial` 相当で破棄） |
| R13 | reviewer の文脈 | — | **済**（仕様上） | knowledge・memory・conversation を載せない（`review_spawn.rs:1031-1062`）。ただし filesystem（R8）と skills（R4）、アカウント（R11）は共通 |
| R14 | A の filesystem 隔離 | host 全体 | **済** | commit `05cd9749`（ADR `2026-10-17-mlsys-pilot2-fixes` D2）。試験: `crates/campaign/tests/a_arm_hardening.rs` の `sandbox_exposes_only_the_workspace_account_and_the_binary`・`real_bwrap_hides_everything_outside_the_workspace`（bwrap がある host でだけ実行）、preflight の `sandbox_usable`。付記: ADR は HOME を「workspace 内」と書くが実装は `HOME=/tmp/home`（tmpfs）。害は無いが記述の食い違い |
| R15 | 試行 workspace の初期資料（repo+materials+kb を A/B/C 同一に） | — | **済**（mock） | commit `c765a1a2`（build_seed、`kb/` を workspace 内へ）。試験: `crates/campaign/tests/trial_inputs.rs` `every_arm_sees_the_same_trial_kb_and_nothing_else`、`isolation.rs` の `check_same_inputs`/`verify_isolation`。実 harness が `kb/` を使うかは未検証（`2026-10-16-mlsys-trial-inputs.md`） |
| R16 | task template が担当・tier・workspace を上書きしない | — | **済** | `celeris_exec::check_template`（commit `05cd9749`）、試験 `tests/celeris_exec.rs`・`tests/preflight.rs` |

状態の区別: R13〜R16 は「実装済み・mock 検証済み」（R14 の実 bwrap 試験は bwrap のある host でのみ実経路）。R1〜R12 は未対応
（R1/R4〜R7/R12 は構成で足りるが、その構成の preflight 検査はまだ無い）。実 LLM の経路では何も検証していない。

## 3. 結論

- 試行に通常 KB が混ざる経路は 3 本: 前置きの索引（R1/R2）、worker の `celerisctl knowledge`（R3、config 既定 path に落ちる）、
  filesystem の直接読み（R8）。記憶（R5）と会話（R6）は担当×案件で全 run に載るので、測定 config/DB を分ければ止まる。
- **構成だけでは足りないのは R2/R3**: 試行用 KB（`<ws>/kb`）を B/C の文脈と検索の先に「接続」する手段が今の config に無い。
  空の測定 KB にすれば汚染は止まるが、B/C は前置きに KB 節が無く `kb/` を file として見るだけになる（A と同じ条件にはなる）。
  受け入れ条件 0 の「試行用 KB の識別文字列は出る」を満たすには R2/R3 のコード修正が要る。
- R8（B/C の filesystem）と R10/R11（Claude のアカウント dir）は、汎用権限基盤を足さない方針の下では人の決定が要る。

## 4. 次葉への具体的修正リスト

### fix-context（dispatcher）
1. `run_context.rs`: 試行 task の判定関数を 1 つ置く（task の labels に `campaign-<16+hex>` があり、作業場所が local の dir）。
   判定は `celeris_exec::is_campaign_label` と同じ規則（campaign crate には依存しないので task-core か task-dispatch に同等の小関数）。
2. `knowledge_context`: 試行 task なら mount を `[KnowledgeMount::dir(<task workspace>/kb)]` だけにし、profile・`default_mounts`・
   `projects/<slug>`・`memory` mount を使わない。
3. `run_extras`: 試行 task なら `memory = None`・`conversation = []`・`standing_rules = []`（R5〜R7 の保険）。
4. worker env: 試行 task の run に `CELERIS_KNOWLEDGE_ROOT=<workspace>/kb` を入れる（`work_unit_env` 等の既存の env 注入口）。
   `<ws>/kb` は `.git` を持たないので `require_kb` が落ちる点に注意（`knowledge::exists` は `.git` を見る）— seed の kb を
   `celerisctl knowledge init` 相当で初期化するか、`dir:` mount と同じ `list_pages` 経路で読む方を選ぶ。
5. `skills_context`: 試行 task では skills を渡さない（A と揃える）か、測定 org を空にする（どちらかを fix-context で決め ADR に書く）。
6. 試験: 実 `run_extras` を通し、通常 KB root・memory_dir・messages に置いた canary が前置き（`RunContext`）に出ず、`<ws>/kb` の canary
   が出ること。worker・planner（Plan task）・木の子 task・reviewer の 4 種で。

### preflight-kb（campaign）
1. `--grader-root` が `--repo-src`/`--materials-src`/`--kb-src` と B/C 案件の repo の下に無いことを検査（R9）。
2. 測定 daemon の構成検査（API で読める範囲）: `knowledge.root` が本番 root と違う、`[memory]` 無し、langmem 無効、campaign 案件の
   messages・standing_rules 0 件、org の `knowledge`/`skills_mounts` が空または A/B/C 共通の固定集合（R1/R4〜R7/R12）。
   API に無い項目は runbook の人の check にする（汎用権限基盤は足さない）。
3. A のアカウント dir（`find_a_account` の結果）に `projects/` の transcript・`CLAUDE.md` が無いことを検査（R10/R11。D1 の答えに従う）。
4. runbook: 測定 daemon は `CELERIS_CONFIG=<測定 config>` を export して起動、作成・リセット・再開の手順（DB/KB/memory/out dir を
   捨てて作り直す・再開時は out dir と ledger だけ再利用）と、API/クラスタ接続が保たれることの確認コマンド。

### canary-e2e
- 実 task-api router（正式 DTO）で campaign label 付き task を作り、通常 KB・別 rep の `trials/**`・grader root に置いた識別文字列が
  worker/planner/子 task/reviewer の RunContext・`knowledge search` の結果に出ず、試行 KB の識別文字列は出ることを stub harness で示す。

## 5. 人の決定（result.json の decisions）

- D1 `a-claude-config-dir`（R10/R11）: A（と B/C）の Claude アカウント dir を測定専用にするか。
- D2 `bc-filesystem-confinement`（R8）: B/C worker の filesystem 読み取りを bwrap で縛るか、path 分離＋canary 検査（読めはするが
  文脈には出ないことの証明）で受け入れるか。

## 証拠コマンド

- `git log --oneline -1` → `08127c23`（main と同じ）
- `grep -rn "CELERIS_KNOWLEDGE_ROOT\|CELERIS_CONFIG" crates/task-worker/src crates/task-dispatch/src` → 該当 0 件（worker に根を渡していない）
- `grep -rn "knowledge_context(\|skills_context(\|run_extras(" crates/task-dispatch/src` → `dispatch_run.rs:766`（全 worker 系）、
  `review_spawn.rs:1062`（reviewer は skills だけ）
- コード変更なし（この WU の差分はこの文書だけ）。cargo の検査はコード差分が無いので走らせていない（統合工程で走る）。

## 未解決事項

- Claude Code の自動記憶を設定/env で切る正式な名前は一次情報で未確認（R11）。
- 実 harness が `kb/` を読むか（実経路）は未検証のまま。

## 提案

- 試行 task の判定（campaign label）を dispatcher に 1 か所だけ置き、knowledge/memory/conversation/env をまとめて切り替える。
  汎用の権限・来歴基盤にはしない。
