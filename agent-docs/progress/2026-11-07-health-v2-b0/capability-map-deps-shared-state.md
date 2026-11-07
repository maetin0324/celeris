---
title: 健全性v2 B0 capability-map / deps-shared-state — crate 依存・module の use の向き・共有状態の書き手
tasks: [01M6V1TGEG5QT5P6PM0Q0D0QMK]
status: done
updated: 2026-11-07
---

# 健全性v2 B0 deps-shared-state

## 結論
- 複数の書き手を持つ共有状態がある。SQLite の表 14 個（うち crate をまたぐもの 3 個）、event kind 20 種（crate をまたぐもの 17 種）、daemon と worker（または run の agent）が書く file 5 個。
- crate をまたいで書かれる表は `chat_client_requests`（task-api chat と task-core chat）、`cos_inbox_items`（task-api cos と task-core chat）、`llm_proxy_requests`（llm-proxy log と task-core store）の 3 個。ほかの表は task-core の中だけで書かれる。
- 書き手の多い event kind は `work_unit_transitioned`（celeris・task-core・task-dispatch・task-ops）、`decision_requested`（同じ 4 crate）、`execution_planned`（celeris・task-dispatch・task-ops）など。
- crate の normal 依存に循環は無い。module 単位では強連結成分が 3 個ある: task-core の 10 module（`store`↔`chat`、`store`↔`script_compare` など）、task-api の `middleware`↔`state`、task-api の 9 module（`cos`↔`handlers` を含む）。
- 現行の書き手が見つからない event kind が 4 種ある（`model_role_assignment_changed`・`project_plan_proposed`・`project_plan_decided`・`routing_shadow_recorded`）。どれも読み取り（task-api query の名前付け、task-ops project_plan の畳み込み）にだけ現れる。旧 event の読み取りのために残っている variant とみられる（意図は未確認）。

## 成果物
- `agent-docs/progress/2026-11-07-health-v2-b0/module-deps.json`（schema `celeris.health-v2-b0.module-deps/1`）。`summary` に件数、`method` に数え方を書いた。
- 生成 script: `agent-docs/progress/2026-11-07-health-v2-b0/module-deps-measure.py`。人が source を読んで選んだ file の書き手は `module-deps-files.json`。
- 再生成: repo の root で `python3 agent-docs/progress/2026-11-07-health-v2-b0/module-deps-measure.py <base_sha> > agent-docs/progress/2026-11-07-health-v2-b0/module-deps.json`（約 4 秒。読むのは `cargo metadata --no-deps` と source だけで、build・試験は走らせない）。

## 基準
- 着手時の `git merge-base HEAD main`（件名「repair: main 取り込みの衝突解消の import を整形する」）。SHA は module-deps.json の `base_sha`。

## 件数（module-deps.json の summary）
| 項目 | 数 |
|---|---|
| crate_deps（workspace member 間） | 49（normal 35、dev 14） |
| module_uses（6 crate の module 間の辺） | 1157 |
| module の循環（強連結成分） | 3 |
| crate の循環（normal 依存） | 0 |
| migration で作られた表 | 135（現存 120、DROP・RENAME で消えたもの 15） |
| 表で書き手が複数 / crate をまたぐ | 14 / 3 |
| migration の trigger だけが書く表 | 2（`chat_search`、`task_roadmap_specs`） |
| event kind | 108 |
| event kind で書き手が複数 / crate をまたぐ | 20 / 17 |
| 現行の書き手が見つからない event kind | 4 |
| daemon と worker が書く file / うち複数書き手 | 6 / 5 |

## file の書き手（source を読んで確かめた）
- `<data>/runs/<run_id>/result.json`: task-worker subprocess と task-dispatch dispatcher（WorkUnit の合成終端）。
- `<artifacts_dir>/result.json`: run の agent、task-worker（result_location・langmem・paperqa・local_deep_research）、task-dispatch dispatcher（CoS の再試行の前に消す）。
- `<artifacts_dir>/delegate.json`: run の agent と task-worker delegate_file（run の前に消す）。
- `<artifacts_dir>/followups.json`: run の agent、celerisctl add、task-dispatch dispatcher（取り込んだ後に消す）。
- scratch pool（lease・.lock・target）: task-worker scratch、task-dispatch scratch_gc・target_sweep、celerisctl scratch、run の中の cargo。
- `<artifacts_dir>/checkpoint.json`: 書くのは run の agent だけ。task-dispatch checkpoint と task-worker claude_code は読むだけで、複数書き手ではない。

## 数え方の限界
- 表の書き手は SQL 文字列の `INSERT [OR x] INTO` / `REPLACE INTO` / `UPDATE … SET` / `DELETE FROM` の直後の表名で数えた。`format!` で表名を組む SQL は拾えない。見つけた 2 表（`run_write_sets`・`work_unit_write_sets`、task-core store write_sets）は `dynamic_sql_writers` に手で足した。ほかに同じ形の書き手があるかは未確認。
- event の書き手は `Event::<Variant>` を構築する箇所で数えた（match・`matches!`・`if let` の pattern を除く）。構築した event を実際に append するかは追っていない。`use Event::*` などで variant 名だけを書く構築は拾えない。
- 試験の code（`tests/`・`*_tests.rs`・`tests.rs`・`test_support.rs`・`*_fixture.rs`・`#[cfg(test)] mod` の後ろ）と行頭の `//` コメントは除いた。
- module_uses は `use` 文だけを数えた。`crate::x::f()` のような use を経ない完全修飾の参照は数えていない。`crate::Type` のように crate root の再 export を指すものは `lib` に寄せた。
- task-core の module 循環 10 個には `store` と各機能 module（`chat`・`script_compare`・`completion_outcome` など）の相互参照が含まれる。型を機能 module に置き、永続化を store に置く配置の結果で、どれが不具合かはこの WU では判断していない。

## 証拠コマンドと結果（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/module-deps-measure.py <base_sha>`: exit 0。script 内の検査で全 writer の path の実在を確かめた。
- 受け入れ条件 0 の検査（crate_deps が空でない、migration の全表が shared_state にある、writer の path が全部ある）と条件 1（multi_writer が 1 件以上）は報告本文に結果を書く。
- crates/・web/・control-ts/ は変更していない。

## 未解決事項
- 書き手の見つからない event kind 4 種を enum から消せるか（旧 event の読み取りに要るか）は未確認。
- `format!` の表名で書く SQL の網羅は未確認。

## 提案
- crate をまたいで同じ表を書く 3 か所（`chat_client_requests`・`cos_inbox_items`・`llm_proxy_requests`）は、書き手を 1 module に寄せるかを G1 の設計で検討する。
