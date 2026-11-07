---
title: 健全性v2 G2 review-policy-cli — --print-review-policy と policy の file だけの変更の扱い
tasks: [01M6VVB2X5EBVCYB655QJVHCN8]
status: done
updated: 2026-11-07
---
# 健全性v2 G2 review-policy-cli — --print-review-policy と policy の file だけの変更の扱い

完了日: 2026-11-07（人が policy の変更を承認し、operator がこの branch を main に no-ff merge した。件名「integrate 01M6VVB2X5: --print-review-policy と D6.2（policy の branch、人の承認済み。ADR health-review-integration D6.6）」。ADR D6.6）

unit `review-policy-cli`（段 base「dispatcher: risk 判定・高 risk 設計 review・final review の health 文脈と構造化 verdict」の子）。
[ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の D2.3・D6.2・D7.3（check-health.sh の冒頭注記と profile.py の docstring）に従う。
基準は `git merge-base HEAD main`。policy の file を変える branch なので、main に入るまで必須検査 `test-affected.sh` は `policy.changed` で落ちる（D6.6。前例は [G2 profiles](../guideline-profiles.md)）。Rust は変えていない。

## 変更
- 件名「G2 review-policy-cli: --print-review-policy と policy の file だけの変更の扱い（D2.3・D6.2・D7.3）」の commit
  - `scripts/dev/repo-health/repo_health.py`: `--print-review-policy --repo <dir> --base <rev> [--profile <name|path>]`。base の commit の profile（extends を解決した結果）の `[review]` を JSON `celeris.repo-health.review-policy/1` で stdout に出して exit 0。`--head`・`--out`・`--policy-version`・`--rules`・`--print-base` と一緒に使うと exit 2、`--base` が無くても exit 2。base が解決できない（shallow を含む）・profile が無い・profile が不正なら exit 2（stdout は空）。
  - `scripts/dev/repo-health/health/profile.py`: 解釈を 1 つに保つため、`resolve`（report 用。base → head（bootstrap）→ 検査器自身）と `resolve_base`（review policy 用。head を読まない）が共通の `_resolve` を通る。`Profile.review_policy()` と `Profile.definition_sha256()` を足した。`[review]` が無ければ `source = absent`（owner_map は null、high_risk と invariants は空）、あれば profile の `policy_source`（base か builtin）。`definition_sha256` は解決に使った profile の内容（子、extends の親の順）を 1 file ごとに `<path>\0<byte 数>\0<内容>` で続けた sha256。組み込み generic は path `builtin:generic` と sort 済みの JSON。docstring は項目の正本として残し、解決順と base 側の読み込みの説明を ADR 2026-11-07-repo-health-contract D2 への参照に縮めた（D7.3）。
  - `scripts/dev/repo-health/health/snapshot.py`: `resolve_base(repo, base)`。base の解決（`auto` を含む）は既存の `default_base`・`_resolve_commit` を使う。
  - `scripts/dev/repo-health/schema/review-policy-v1.schema.json`（新規）: 欄は schema・source・owner_map・high_risk・invariants・definition_sha256 だけ（追加欄は不可）。
  - `scripts/dev/repo-health/health/engine.py`: `policy_review_files(report, changed)`。verdict が fail で、fail 級の finding が `policy.changed` だけ（ほかの必須 rule の fail も無い）、変更 path が全部 `policy.files` にあるとき、変えた file の一覧を返す。
  - `scripts/dev/check-health.sh`: repo_health.py が exit 1 のとき、report の base の sha で snapshot を取り直して変更 path（base と作業ツリーの差分。未 commit・未追跡の file を含む）を得て、`policy_review_files` が file を返せば `policy review required: <files>` を出して exit 0 にする（D6.2）。report の verdict は fail のまま。冒頭注記は ADR 2026-11-07-repo-health-celeris-profile・repo-health-contract・health-review-integration D6.2 への参照に縮めた（D7.3）。
  - `scripts/tests/test_repo_health_review_policy.py`: 11 件を足した（計 19 件、prefix `test_health_review_`）。base 側だけを読む（head の commit と作業ツリーの変更を無視）、definition_sha256 の再計算と親の変更での変化、extends の子の下位項目が勝つ、`[review]` が無いと absent、base に profile が無いと absent・名前の profile は builtin、読めないと exit 2、head 等の引数の拒否、schema が不正な形を拒む、この repo の HEAD で source=base と必須 7 class、D6.2 の判定の表、check-health.sh が policy の file だけの commit で exit 0・code も変えれば exit 1。
  - `scripts/dev/tests/repo_health_gate.sh`: (6) を足した。policy の file だけの変更は作業ツリーでも commit 済みでも exit 0 で `policy review required: scripts/dev/check-bad.sh` を出し、report の verdict は fail。app.py も変えれば exit 1。

## 取り込みの後の確認（2 回目の run。提出時の HEAD で実行。基準は git merge-base HEAD main）
main は取り込んでいない（この branch の先端が main の merge の親になったので、`git merge-base HEAD main` はこの進捗の commit の直前の HEAD に一致する）。この run で変えたのはこの進捗 file だけ。
- 受け入れ条件 4: 人の決定 `policy-cli-merge` は「承認する」。operator の merge commit は main にあり、その親にこの branch の先端がある（`git log -1 --format=%P main`）。
- `bash scripts/dev/test-affected.sh` → exit 0（最初の check-health.sh は verdict pass、`policy.changed` は pass）
- `bash scripts/dev/check-health.sh` → exit 0（verdict pass、fail_grade 0、自己試験の unit 73 件 OK と gate (1)〜(6) OK を含む）
- 受け入れ条件 0: `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py' && sh scripts/dev/tests/repo_health_gate.sh` → exit 0（unit 73 件 OK、gate (1)〜(6) OK）
- 受け入れ条件 1: `--print-review-policy --repo . --base HEAD` の出力の検査 → exit 0（source=base）
- 受け入れ条件 2: `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0

## 証拠（1 回目の run。取り込みの前の HEAD で実行。基準は git merge-base HEAD main）
- 受け入れ条件 0: `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py' && sh scripts/dev/tests/repo_health_gate.sh` → exit 0（unit 73 件 OK、gate は (1)〜(6) OK）
- 受け入れ条件 1: `python3 scripts/dev/repo-health/repo_health.py --print-review-policy --repo . --base HEAD` の出力の検査 → exit 0（schema と source=base、class に store・events、definition_sha256 は 64 桁）
- 受け入れ条件 2: `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- `bash scripts/dev/check-health.sh` → exit 1。verdict fail、`fail_grade=1`。fail の rule は `policy.changed` だけで、変えた policy の file は 6 件（check-health.sh・engine.py・profile.py・snapshot.py・repo_health.py・review-policy-v1.schema.json）。試験 2 file とこの進捗は policy の file でないので D6.2 の対象にならない（下の「未解決」）。
- `bash scripts/dev/test-affected.sh` → exit 1。最初の check-health.sh が同じ `policy.changed` で止まる（D6.6 の想定どおり）。
- `bash scripts/dev/check-health.sh --base HEAD`（この branch の HEAD を base にする。branch を main に取り込んだ後の main に当たる）→ exit 0、`policy.changed` は pass。main にだけある commit（G7 run2 の raw・n6.md など）は policy の file を変えていない（`git diff --name-only HEAD...main` に policy の path が無い）。
- `git merge-tree --write-tree main HEAD` → exit 0（衝突なし）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0（Rust は変えていない）

## 未解決
- 取り込みで、この unit の 3 commit のほかに親 task の branch の 17 commit（merge でない 11 件: G2 の guideline/generic・plan-brief・protocol・profiles の進捗・guide。段の統合の merge 6 件。Rust の変更を含む）も main に入った。operator は、Rust の新しい欄は省略可能で未配線のため本番の挙動は変わらないと確認した。親 task の final review では、これらの commit が main 側にあり diff に出ない。
- D6.2 の条件「変更 path が全部 policy.files」では、進捗 file（`agent-docs/progress/`）と検査器の試験（`scripts/tests/test_repo_health*.py`・`scripts/dev/tests/repo_health_gate.sh`）は policy の file に入らない。Celeris の task は必ず進捗 file を書くので、今のままでは D6.2 が Celeris の branch で働くことはほぼ無い（この branch も対象外）。
- D6.4（policy の変更に人の承認を要求する dispatcher の経路）は U5 の範囲で、まだ無い。D6.2 が働く branch（policy の file だけを変え、進捗 file を書かない branch）は、U5 が入るまで check-health を exit 0 で通り、reviewer の合格だけで配送されうる。
- check-health.sh は作業ツリーの版が走る。候補が check-health.sh を変えれば D6.2 の判定も変えられる（`policy.changed` は出る）。この点は D6.2 の前から同じ。
- review policy の `source` の enum に `bootstrap` を置いた（ADR D2.3 と report の `policy.source` の語彙に合わせた）。この出力は head を読まないので `bootstrap` は出ない。

## 提案
- D6.2 の対象に profile の `doc_only.paths`（進捗・文書）を足し、検査器の試験を Celeris profile の `policy_paths` に入れる。D6.4（U5）が入った後に、別の policy の branch で行う。先に行うと、上の「未解決」の配送の穴が広がる。
- U3・U4 の Rust 側（`task_core::design_review::ReviewPolicy`）は `source` を読み、`bootstrap` を受けたら読み込みエラーにする。
