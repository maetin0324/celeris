---
title: R5 adr-tables — ADR の状態・D3・束の表・付記と adjust-worker の進捗を実装に合わせる
tasks: [01M6TYVJT53598CPKMGSQGA7XM, 01M76Z46J2331HPP9XX3Q5D9P5]
status: done
updated: 2026-11-12
completed: 2026-11-12
---
# R5 adr-tables — ADR の状態・D3・束の表・付記と adjust-worker の進捗を実装に合わせる

完了日: 2026-11-12

結論: ADR [2026-11-10-test-target-bundles.md](../../adr/2026-11-10-test-target-bundles.md) を tree の束と `inventory.json` に合わせた。
`adr-check.py` は ADR に exit 0「一致（4 crate、束 17 個、束の target 85 件）」（直す前は exit 1、食い違い 5 件。[adr-check.md](adr-check.md)）。
[adjust-worker.md](adjust-worker.md) の status を blocked から done にし、未解決の test-affected を後の run の合格で更新した。
検査器（`adr-check.py`・`classify.py`・`cases.py`）・`inventory.json`・baseline・test-affected の規則・コードは変えていない。

基準は `$CELERIS_WU_BASE`（WU の base）と `git merge-base HEAD main`。

## 実装前 brief
1. 根本原因: ADR の束の表・D3・状態の行は設計の時（unit adr）のままで、実装の leaf（bundle-api-b・adjust-api・adjust-worker・worker-singleton）が束を変えても追従していなかった。
   leaf の範囲 check が ADR を write set の外にしたので、各 leaf は ADR を直す文を自分の進捗の「提案」に置いた。
2. 責務 owner: ADR 本文（この unit の objective）と adjust-worker の進捗。
3. 既存の再利用先: `adr-check.py`（表と tree の照合）、各 leaf の進捗の「提案」「ADR の付記の案」の文、`inventory.json` の `summary`・`leaf_reasons`、`classify.py` の `PG_PATTERNS`。
4. 選択肢: (a) ADR の表を tree と `inventory.json` から書き直し、付記に leaf ごとの違いを書く → 採用。(b) ADR を設計の版のまま残し付記だけを足す → 表が tree と食い違い `adr-check.py` が exit 1 のままなので採らない。
5. 新しい状態・分岐・fallback・port: 無し（文書だけ）。
6. 撤去対象: ADR の設計の時の誤り（状態『採用（設計）』、D3 の `PROCESS_GLOBAL` の名前と `OnceLock`・`Lazy` を mutable_static とした記述、実在しない task-worker の `unmapped_suite`、`unmapped_suite_a` の `docs`、件数）。
7. 不変条件: ADR の束の表が tree の 4 crate の `tests/<束>/main.rs` と 1 対 1 で件数・target が一致し、keep の行の target が元の path に在る（`adr-check.py` が確かめる）。

## 変更
件名「phase 1: adr-tables: ADR の状態・D3・束の表・付記と adjust-worker の進捗を実装に合わせる」の commit:
- ADR の状態の行: 『採用（設計）。この ADR ではコードを変えない』を『採用・R5 で実装済み（r6 の 55 target は R6）。最初の版を書いた unit adr はコードを変えていない』にした。段 final の unit に `pick-g4`・`worker-singleton` を足し、照合の台本 `adr-check.py` を書いた。
- D2: 最大の束を `wc -l` で数え直した。task-api `unmapped_suite_a` の 7 file 3718 行（`main.rs` の 10 行を除く。設計の時は 8 file 4627 行）、最大の file は task-worker `browser_eval_surface_suite` の `browser_live_cross.rs` の 1140 行（次は task-api `operations.rs` の 1133 行）。
- D3:
  - 検出の正規表現の名前を `classify.py` の `PG_PATTERNS`（行の文脈は `PG_LINE_CONTEXT`）に直した。
  - mutable_static を「`static mut` と、型に `Mutex`・`RwLock`・`Cell`・`RefCell`・`Atomic*` を持つ `static`」に直し、`OnceLock`・`Lazy` を keep の理由から外した。
  - 束の支援 module が持つ不変の `LazyLock`・`OnceLock`（task-api `tests/common/mod.rs` の `TEST_PRINCIPAL`、celeris `tests/support/script_compare.rs` の `fresh_bundle_dir` の `DIR`）を 1 process で共有してよい理由を書いた。値はどの試験が先に初期化しても同じで、誰も書き換えない。
  - `reap_finished_children` の `waitpid(-1)`（同じ process の任意の子を回収する）を keep の理由に足した。`PG_PATTERNS` では検出せず、adjust-worker が `leaf_reasons` に手で足したことも書いた。相手を失った `ssh_cluster_manual` も keep と書いた。
  - 固定 path・signal の列挙を `PG_PATTERNS` の実際の対象（`/var/tmp`・`/dev/shm`、`SIGSTOP` などの名前）に合わせた。
- D4-1: named_suite に「`scripts/` の `#` comment でない行が file の path を名指しする target」（付記 bundle-api-b）、singleton_bundle_group に「adjust leaf の組も含む」（付記 worker-singleton）を足した。D4-3 に「実装では 5 件を 1 束にし、4 件を keep にした」を足した。
- 束ねの表:
  - 表の前に、表が 2026-11-12 の tree と `inventory.json` に合うこと、束の欄が `keep` で始まる行の意味を書いた。
  - 段 bundle の見出しを「76 target → 15 束。task-api 55 → 9、celeris 15 → 4、celerisctl 6 → 2」に直した。`unmapped_suite_a` を 7 件にし、`docs` を外した。`docs` を keep にした理由（`check-doc-links.sh` の `FOREIGN_DOCS` が path で挙げる）を表の下に書いた。
  - `group:none` の束の項を、sync-main が capability-map の不変を確かめた事実に直した。
  - 段 final の見出しを「決定 `r-landed` の b の後。R2・R3 の入った main で束ねた。R4 の後の調整は R6」にした。表は adjust-api の `cos_delivery_suite` 4 件、task-worker の `browser_eval_surface_suite` 5 件、束の欄が `keep` で始まる 4 行（`reap_finished_children`・`ssh_cluster_manual`・`browser_restore_deliver`〈実 browser・userns〉・`browser_action_recovery`〈worker-singleton の後の 1 件だけの組〉）にした。adjust-api が `cos_delivery_suite_*` へ足す案を採らなかった理由を表の下に書いた。
  - 動かさない target の表を `inventory.json` の `summary` に合わせた。task-api を keep 18（名前で呼ばれる suite 6）に直し、task-worker の行（keep 16、r6 0）を足した。内訳は `leaf_reasons` の全部の理由で数えたこと（重複あり）を書いた。
  - 束ねの手順 5 の binary の数の例を、実際の値（task-api 114 → 99。bundle-api-a の後）に直した。
- 付記「実装との突き合わせ」に 11 行を足した: affected-features・bundle-api-a・bundle-api-b・bundle-celeris・bundle-ctl・dedupe（dedupe-api・dedupe-celeris の 1 行）・sync-main・pick-g4・adjust-api・adjust-worker・worker-singleton。各行は leaf の進捗から ADR と違えた点を書いた（例: D6-2 の filter は `package(=…)`、D6-3 の doc test は 31 件でなく 5 件で measure.sh に足していない、`equivalent_scenario` は 1 件も束ねていない、sync-main は `classify.py --write` をやり直していない、adjust-api は `cos_delivery_suite_*` に足していない）。
- [adjust-worker.md](adjust-worker.md): front matter の status を done にし、`updated`・`completed` と完了日を足した。「変更」の 1 件だけの束の項に、worker-singleton が元の target に戻したことを足した。「未解決事項」の test-affected（`launch_notice_tests` の 3 件）を、R5 の進捗の resync-g4 と後の test-affected・measure-run の合格、measure-run の task-worker の 3 回の合格、worker-singleton.md の test-affected exit 0 で更新した（原因は未確認のまま）。「提案」を ADR に反映した事実に直した。
- この file。

## 証拠
提出時の HEAD の結果は下の「検査」に書く。

## 未解決事項
- R5 の進捗 [2026-11-10-health-v2-r5.md](../2026-11-10-health-v2-r5.md) の数は worker-singleton の前の値のまま（「86 target を 18 束」、task-worker の keep 15、束ねた target 6 → 2 束）。tree は 85 target・17 束、task-worker は keep 16・束 1 個。この unit の objective（ADR と adjust-worker.md）の外なので直していない。
- `classify.py` の `make_bundles` は adjust-* の 1 件だけの組を残す規則のまま。`classify.py --write` で作り直すと `browser_action_recovery` が adjust-worker に戻る（worker-singleton.md の提案）。
- `launch_notice_tests` の 3 件の失敗の原因は未確認（adjust-worker.md の「未解決事項」）。

## 提案
1. R5 の進捗の結論・前後の数値・残した target の表を worker-singleton の後の値（束ねた target 85・束 17、task-worker の keep 16・束 1、task-worker の束ねた target 5）に直す。統合（integrate-sync）か R5 の進捗を変えてよい unit が行う。
2. `classify.py` の `make_bundles` で adjust-* の 1 件だけの組も keep（`singleton_bundle_group:<leaf>`）にする（worker-singleton.md の提案と同じ）。直せば `--write` の出力と `inventory.json` が一致する。
3. 束の表を変える unit の check に `adr-check.py <ADR>` を入れる。今回の食い違い 5 件は、範囲 check が ADR を write set の外にした leaf で生まれた。
