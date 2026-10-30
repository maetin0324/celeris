---
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
---
# ADR 2026-10-30: ops-r1 の condition は manifest の `conditions` 欄で表し、記録は ops-r1-assemble で runner の out に組み立てる

- 状態: 採用（設計。実装は同じ task の pipe-manifest・pipe-raw・pipe-assemble・pipe-e2e の葉）
- 関係: [2026-10-29-ops-r1-trace-attribution-export](2026-10-29-ops-r1-trace-attribution-export.md)（B の trace と消費）、
  [2026-10-29-ops-r1-cos-intake-record](2026-10-29-ops-r1-cos-intake-record.md)（B の受付の記録）、[2026-10-27-trial-observed-versions](2026-10-27-trial-observed-versions.md)
  （契約 revision `main-4-ops-r1`）、[2026-10-19-experiment-sets](2026-10-19-experiment-sets.md)、ops-r1 preflight の不足
  `g-ops-r1-pipeline-manifest`。仕様の本文は `docs/ops/ops-r1-pipeline.md`

## 1. 文脈

grade-out・check・export・final-accounting は `--manifest` と runner の out dir（`trials.jsonl` など）を入力にする。ops-r1
（measurement_id `mlsys-main-4-ops-r1-k4p1`）の 2 condition、A=人が直接起動する Claude Code（`…-A-direct-claude-code`）と
B=測定 daemon の案件 CoS thread で受け付ける通常運用（`…-B-celeris-ops`）は runner を通らないので、out dir も manifest も無い。
既存の manifest の B は `POST /tasks` に routing を固定して投入する arm で、CoS 受付の B を表せない。human-op の `--arm` は A0/B0/B1/C1 だけ。
測定対象の daemon は変えられない。旧系列（A0/B0/B1/C1、`mlsys-main-4-v4`）の manifest・sha256・台帳は 1 byte も変えない。

## 2. 決定

- D1: ConfigId は A・B を流用し、manifest に省略時は直列化しない欄 `conditions`（arm → condition_id・`intake`・`trial_ids`）を足す。
  `intake` は `direct_claude_code`（A）か `cos_thread`（B）。`conditions` と `contract.revision = main-4-ops-r1` は互いに必須。
  B が `cos_thread` のとき final の routing 必須を免除し、routing を書いたら拒否する。
- D2: `conditions` のある manifest は task API に投入する道具（`run`・`dry-run`・`preflight`・`cap-probe`・`config-check`・`env-check`）が拒否する。
  B を `POST /tasks` 投入として扱う道を型の外の運用ではなく検査で塞ぐ。
- D3: human-op は `--condition <condition_id>`（`--arm` と排他、`--manifest` 必須）を足す。記録は experiment_set `ops-r1`、arm A/B、
  省略時は直列化しない `condition_id`。`--arm` に値（例 `OA`・`OB`）を足す案は採らない: condition_id は契約の識別（条件を変えたら
  新しい condition_id を発行する、評価契約 §3）で、固定の label では次の condition を表すたびに enum を増やすことになり、manifest との
  対応も検査できない。
- D4: 新しい入口 `celeris-campaign ops-r1-assemble` が、A の raw（`ops-r1-a-raw/1`、transcript から作る）と B の intake・trace・attribution・
  成果物 dir から、runner と同じ形の out dir と decision-log/1 形の `records.jsonl` を作る。`run-meta.executor` は `ops-r1-assemble`。
  時刻・host・実行 path を出さず、同じ入力からの 2 回の出力は byte 一致にする。lock.json は `run` と同じ `expected_lock` で作る。
- D5: grade-out・check・export・human-op report・final-accounting・check-out-consistency.py・stage-grader-inputs は既存の subcommand を
  使い、ops-r1 用の入口を別に作らない。checker の (A, B) 組の B0 検査（B の run が全部 Sonnet）も ops-r1 にそのまま効かせる。
  そのため B の observed run に model・harness が要り、`ops-r1-attribution` に DB の写しの `runs.model`・`runs.adapter` を写す欄を足す
  （schema `ops-r1-attribution/2`）。CoS の model（opus）は assemble が `chat_runs.resolved_config_json` で固定票と照合する。
- D6: 旧系列の不変は「足す欄はすべて省略時に直列化しない」「追加の検査は `conditions` か ops-r1 契約があるときだけ」で守り、
  旧 manifest の sha256 と旧 human-op 記録の record_id が変わらない試験で固定する。

## 3. 却下した案

- ops-r1 専用の manifest 型と、grade・check・export・集計の ops-r1 版の入口（`ops-r1-pipeline` 一式）: 成功契約・分母・欠測規則の
  実装が 2 系統になり、check-out-consistency.py が照合する前提（同じ checker・export）が崩れる。
- ConfigId に新しい値（例 `Bo`）を足す: TrialRecord・ledger・grades・human-op・final-accounting の全 match と直列化に波及し、
  B0 の検査や experiment set の検査を ops-r1 だけ外す分岐が増える。condition の違いは受付経路で、ConfigId（どの道具で走らせるか）の違いではない。
- B の消費を `celerisctl campaign export` で取る: CoS が起票した task は campaign 識別を持たず結べない（ADR 2026-10-29-ops-r1-trace-attribution-export）。
- daemon に試行の識別を持たせる: 測定対象の daemon を変えることになり、固定した release 94f86928dc13 で測れなくなる。

## 4. 結果

- daemon（crates/campaign 以外の crate）の変更は要らない（verdict: feasible）。
- 未確認の点（set 宣言の照合が routing の無い B を通すか、自由文の reason を checker が受けるか、check-out-consistency.py が executor を見るか）は
  pipe-assemble が試験で確かめる。外す必要があれば executor 名ではなく `conditions.B.intake` を読んで外す。

## 付記 2026-10-30（pipe-manifest の実装との突き合わせ）

- D1 の置き場所: `conditions` は manifest の最上位ではなく `contract.conditions`（`ContractRevision` の任意欄、空なら直列化しない）に置いた。
  最上位に欄を足すと、既存の試験が構造体リテラルで組む `CampaignManifest` 約 45 か所が compile できなくなり、旧試験を無修正で通せない。
  `contract` の下なら直す既存試験は `crates/campaign/tests/celeris_exec.rs` の `ops_r1()` の 1 行（`conditions: Default::default()`）だけで、
  「condition は ops-r1 契約の manifest だけが持つ」も型で表せる。読み出しは `CampaignManifest::conditions()`（`crates/campaign/src/ops_r1_conditions.rs`）。
- D1 の「互いに必須」のうち「`contract.revision = main-4-ops-r1` なら `conditions` 必須」は入れなかった。既存の
  `celeris_exec.rs` の試験（trial-observed-versions）が conditions の無い ops-r1 契約で A/B/C の `run` を通しており、必須にすると旧試験が落ちる。
  検査は「`conditions` があれば revision は main-4-ops-r1」の向きだけ。
- B0 の宣言検査（`check_b0_all_sonnet`）は、ops-r1（conditions あり）で `fixed.role_models` が空のときだけ外した（設計書 §3.1 の「role_models を書かない」を通すため）。
  `role_models` を書けば従来どおり検査する。observed の Sonnet 照合（checker の `check_b0_observed`）は変えていない。
- `HumanOpRecord.condition_id` の追加で、既存試験の構造体リテラル 2 か所（`crates/campaign/tests/human_ops.rs` の `base`、
  `crates/celeris/tests/actor_trace/support.rs` の `human_record_base`）に `condition_id: None` を 1 行ずつ足した。assert は変えていない。
