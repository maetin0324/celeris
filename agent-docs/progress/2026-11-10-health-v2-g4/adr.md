---
title: 健全性v2 G4 adr — ADR health-generic-fixtures
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 G4 adr — ADR health-generic-fixtures

完了日: 2026-11-10

WorkUnit `adr`（G4 の ADR: fixture の置き場所と builder、kind と variant、目的 (a)〜(h) と試験の割り当て、測り方、write_set）。基準は `$CELERIS_WU_BASE`。コードは変えていない。

## 変更
- 件名「G4 adr: ADR health-generic-fixtures と task・unit の進捗」の commit — [ADR 2026-11-10 非 Celeris の isolated Git fixture で repo health の汎用性とすり抜け耐性を確かめる](../../adr/2026-11-10-health-generic-fixtures.md)（状態: 提案）、task の進捗 [2026-11-10-health-v2-g4.md](../2026-11-10-health-v2-g4.md)、この file を追加した。

## 決めたこと（要約）
- (1) fixture は試験の実行時に `$TMPDIR` の一時 Git repo として生成する。builder は `scripts/tests/repo_health_fixtures.py` の 1 か所で、Python は import、Rust は CLI `python3 -I scripts/tests/repo_health_fixtures.py build <kind> <variant> <dest>` で呼ぶ。B0 の `scripts/dev/repo-health/fixtures/` は Celeris profile の `policy_paths` に当たるので採らない。
- (2) kind は ts・rust・multi。variant は good・dup-contract・dup-state・dep-inversion・security-double・vendor-generated・docs-only、policy の弱化 5 種（policy-exclude・policy-rule-off・policy-exception・policy-threshold・baseline-rewrite）、判定 3 種（non-git・unsupported-tool・history-smuggle）。fixture の base は自分の `default.toml` を持ち、rule と層と例外（reason 付き）を宣言する。
- (3) (a)(b)(c) は `scripts/tests/test_repo_health_generic.py`（`test_generic_`）、(d)(f)・束縛は `test_repo_health_generic_bypass.py`（`test_generic_`）、(g)・flag・複数 repo は `crates/task-dispatch/src/dispatcher/tests/health_generic.rs`（`health_generic_`）、(e)(f) は `health_generic_stale.rs`（`health_generic_stale_`）、(h) は `health_generic_limits.rs`（`health_generic_limits_`）。期限付き例外は決定 exception-expiry と子 task exc-expiry に回す。
- (4) 精度は期待のラベルと fail 級の finding の照合、誤検出は期待の無い variant の fail 級の数、追加遅延は検査器 1 回の実時間の中央値（3 回）、LLM の費用は有料の再測定をしないので未測定と書く。
- (5) write_set の外の path（builder、dispatcher/tests の `health_generic*.rs`）は slices.json を変えず各 unit の範囲 check で許す。

## 目的文との違い
- 目的文の variant に「判定の 3 種（non-git・unsupported-tool・history-smuggle）」を足した。(d) の non-Git・unsupported・tool 故障と (f) の「候補の branch の途中の commit を base にしない」を fixture の行列で測るため。
- Rust の試験の置き場所は目的文のとおり `crates/task-dispatch/src/dispatcher/tests/`。B0 の write_set（`crates/e2e/tests/`）とは違うので ADR の (5) に理由を書いた。

## 証拠（WU の作業ツリーで、commit の前に実行。基準は `$CELERIS_WU_BASE`）
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (414 files)`）
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）
- `python3 scripts/dev/check-human-text.py` → exit 0（0 findings）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、doc-only、new=0・worsened=0）
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、出力は ADR と進捗の 3 file だけ。
- 受け入れ条件 0: ADR を `### (` で分けた 5 節のすべてに「- 決定」と「採らなかった」の行があり、後続 unit 5 つの試験 file と prefix 4 つ、exc-expiry への回送、費用の「未測定」が本文にあることを python で確かめた（すべて True）。
- コードを変えていないので、`test-affected.sh`・clippy・fmt は record で task の提出時の HEAD に対して走らせる。

## 未解決
- 後続 unit は未着手。(2) の期待のラベルの鍵の形は py-detect が検査器の実装に合わせて確かめる。

## 提案
- なし（task の進捗の提案節に slices.json の `consolidate_into_owner` の整理を書いた）。
