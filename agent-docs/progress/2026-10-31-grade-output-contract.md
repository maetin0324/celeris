---
title: celeris-campaign grade の出力契約（不合格の判定 JSON を stdout に）と採点 wrapper の追従
tasks: [01M67RK397A0VJXGC60J5BFEJN]
status: done
updated: 2026-10-31
---
# celeris-campaign grade の出力契約と採点 wrapper の追従

完了日: 2026-10-31

ADR: [2026-10-30-ops-r2-isolation-principal](../adr/2026-10-30-ops-r2-isolation-principal.md) 付記 H7（grade・grade-out の出力契約）・H8（wrapper の扱い）

## 変更（commit の件名）
- grade/grade-out: 判定 JSON は合否とも stdout（合格 0・不合格 1）、エラーは stderr・exit 2
- grade 出力契約: ADR 付記 H7/H8、wrapper は exit 0/1 と判定 JSON のときだけ outbox、CLI と wrapper の試験を新契約に
- set-private-sha 試験: 漏れた旧 hash は script の定数から読む（case.json は新 hash に替わった）
- dagbench graded: 不合格の判定は exit 1 の stdout から読む（grade 出力契約に合わせる）
- 本進捗ファイルと cargo fmt の整形

契約: 判定 JSON は合格・不合格とも stdout に出す。合格 exit 0、不合格 exit 1。判定 JSON を作れないエラー（引数・grade-case 読込・private hash 不一致など）は stderr に短い理由を出し exit 2、stdout は空。grade-out も同じ契約。不合格の判定を `error: ` 付きで stderr に出す旧い経路は main.rs から消した。

wrapper（scripts/campaign/ops-r2-grade.py）: exit 0/1 以外、または stdout が判定 JSON でないときは outbox に何も置かず exit 70。不合格（exit 1 + 判定 JSON）は漏洩検査を通して outbox に置き、wrapper 自身は exit 0 で outbox の path を 1 行出す。docs/ops/ops-r2-isolation.md §4.3 を同じ内容に直した。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `cargo nextest run -p celeris-campaign --test grader_independent`（本物の binary）: cli_grade_outputs_json_and_exit_code（不合格 fixture で stdout が判定 JSON・exit 1・stderr に判定なし）と cli_grade_error_is_exit_2_with_empty_stdout（存在しない grade-case で exit 2・stdout 空）が通過。
- `python3 -m unittest discover -s scripts/campaign -p test_ops_r2_grade.py`: 17 tests OK（偽 binary は新契約。不合格は漏洩検査を通して outbox に置かれる、exit 2 のエラーでは outbox が空）。
- `cargo nextest run -p celeris-campaign --test dagbench_graded`: 5 passed。
- `bash scripts/dev/test-affected.sh`: exit 0、2897 tests run: 2897 passed, 9 skipped。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0（最初の実行で grader_independent.rs の整形差分が出たので `cargo fmt --all` で直した。整形だけの変更）。

## 未解決事項
- wrapper の試験で本物の celeris-campaign を通す経路は作っていない（本物の binary の契約は grader_independent.rs が確かめる）。
- SetShaTest の 2 件（test_refuses_leaked_and_bad_sha・test_refuses_repo_grade_case_sha）は base（main）の時点で既に失敗していた。case.json の hash が新しい値に替わったのに、試験が case.json の値を「漏れた旧 hash」として使っていたため。試験が script の LEAKED / LEAKED_GRADE_CASE 定数を読むように直した。
- 評価器 uid での実機の採点は未確認（下の手順で operator が確かめる）。

## 本番への移行（operator が順に行う）
1. この branch が main に入った release の `celeris-campaign` と wrapper を入れ替える:
   `sudo install -o celeris-eval -g celeris-eval -m 0555 <release の celeris-campaign> /var/lib/celeris-eval-bin/celeris-campaign`
   `sudo install -o celeris-eval -g celeris-eval -m 0555 scripts/campaign/ops-r2-grade.py /var/lib/celeris-eval-bin/grade`
   確認: `sha256sum /var/lib/celeris-eval-bin/grade scripts/campaign/ops-r2-grade.py` の 2 行が一致し、`sha256sum /var/lib/celeris-eval-bin/celeris-campaign <release の celeris-campaign>` も一致すること。
2. wrapper 経由の採点をやり直す（`agent-docs/progress/2026-10-30-k2-grade-eval-uid.md` の手順 4、`docs/ops/ops-r2-k2-preflight.md`）: 未修正の K2 公開初期 tree を §4.3 の `tar --dereference --hard-dereference` で tgz にして drop box に置き、`sudo -u celeris-eval /var/lib/celeris-eval-bin/grade --tgz <tgz> --sha256 <hex> --case k2-bf-xattr-restart-campaign` を実行する。期待: 判定は不合格でも wrapper は exit 0 で outbox の path を 1 行出し、outbox の判定 JSON は `pass` が false で env_failure が無い。exit 70 なら stderr の理由と EVAL_ROOT/work のログを見る。
3. grade-case の形式は変えていないので、評価器の領域の grade-case と private は作り直さなくてよい。repo の K2 case.json の hash も変えていない。

## 提案
- なし。
