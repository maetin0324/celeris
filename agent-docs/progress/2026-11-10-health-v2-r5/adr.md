---
title: 健全性v2 R5 adr — test target の束ね方・削除の証拠・作り直しの直し方の ADR
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 adr — test target の束ね方・削除の証拠・作り直しの直し方の ADR

完了日: 2026-11-10

WorkUnit `adr`（段 design）。段 survey の成果（`inventory.json`・`baseline.json`・`contracts.json`・`cases.py`・`mutate.sh`）から [ADR 2026-11-10-test-target-bundles](../../adr/2026-11-10-test-target-bundles.md) を書いた。repo のコード（`crates/`・`scripts/`）と `inventory.json` は変えていない。基準は `$CELERIS_WU_BASE`。

## 変更
- 件名「R5 adr: test target の束ね方・削除の証拠・test-affected の作り直しの直し方を ADR に書く」の commit — ADR とこの file を追加した。

## ADR の決定の要点
- D1 束ねの規則: 同じ crate・leaf・capability group・fixture の target を `tests/<bundle>/main.rs` の下に `git mv` で集める。束の名前は数字を含めない。nextest は試験ごとに別 process、`cargo test` は 1 process なので、汚染しうる target は束ねない。
- D2 上限: 1 束 8 target まで、本体の行の合計 8000・1 file 2000（size 規則の module の単位は `src/` なので `tests/` では finding は出ない。計画の最大は 6 file 3678 行）。
- D3 汚染: env・CWD・process::exit・可変 static・signal・umask・current_exe・固定 port・固定 path は keep。時刻を読むだけは汚染でない。束ねた後に `cargo test --test <bundle>`（1 process）も通す。
- D4 keep・r6: inventory.json の `rules.leaf_order` をそのまま採った。
- D5 削除: contracts.json の候補だけ。契約の case が残る・`mutate.sh` が KILLED・不変条件でない、の 3 つを `removals/<crate>.json` に記録し、`cases.py` が exit 0。試験の数が減る変更は決定 `test-removal` の後。replay の契約の試験（legacy_routing_events・legacy_event_startup）は消さない。
- D6 作り直し: `test-affected.sh` の `cargo nextest run -p …` を `--workspace -E 'package(a) | package(b)'` に、`cargo test --doc -p …` を `--doc --workspace` にする（案 F1）。対象 crate の規則は変えない。F2・F3 は採らない。stable の cargo が `feature-unification` を読めるようになったら F3 へ替える。
- D7 測り方: `measure.sh run` を base と HEAD で同じ run・同じ順に走らせ、`baseline.json` と同じ欄で比べる。

## 束ねの表と inventory.json
- ADR の表は `inventory.json` の `bundle` と `leaf` を写した（段 bundle 19 束 95 target、段 final の案 6 組 15 target、動かさない keep・r6 の内訳）。`inventory.json` の案は見直した上で変えていない（名前・上限 8・鍵はそのまま）。
- `#[path = "../common/mod.rs"] mod common;` を束の `main.rs` に置けば入れ子の `pub mod cos_ops;` が `tests/common/` から解決されることを、`$TMPDIR` の小さな crate を rustc 1.98.1（edition 2024）で build して確かめた（repo には置いていない）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --check agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0（352 targets、25 bundles、0 error）
- `sh scripts/dev/check-doc-links.sh` → exit 0、`sh scripts/dev/check-adr-numbers.sh` → exit 0（415 files）
- ほかの検査の結果は下の「検査」に書く。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、new=0・worsened=0、doc-only）
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。repo-health verdict=pass）
- `cargo clippy --workspace -- -D warnings` → exit 0（59 秒）
- `cargo fmt --all -- --check` → exit 0
- `python3 scripts/dev/check-human-text.py --root .` → exit 0（0 findings）、`sh scripts/dev/progress-index.sh --check` → exit 0
- `{ git diff --name-only "$CELERIS_WU_BASE"; git ls-files --others --exclude-standard; } | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 …` → `write-set R5: ok`
- `sh "$CELERIS_WU_SCOPE_PATHS"` → ADR とこの file の 2 path だけ
- 受け入れ条件 1（束ねの表と inventory.json の担当 leaf の一致）: ADR の表の行を読んで `inventory.json` の `crate`・`leaf`・`bundle` ごとの target の集合と比べる一時の Python（repo には置かない）で、25 束すべて一致・不一致 0 件

## 未解決事項
- doc test の件数と秒は未計測（ADR D6-3 の概数 31 件は doc comment の fence の数からの推定）。`affected-features` が measure.sh に足して記録する。
- `group:none` の束（`unmapped_*`）の鍵は、contracts.json の extra capability が B0 capability-map に入った後に sync-main が付け直す。

## 提案
- B0 capability-map に contracts.json の extra_capabilities を足す task（R5 の write set の外）を、段 final の前に起票する。入れば `unmapped_*` の 5 束 22 target に capability の名前が付く。
