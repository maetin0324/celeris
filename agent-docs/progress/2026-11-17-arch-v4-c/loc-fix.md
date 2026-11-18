---
title: アーキ v4 C loc-fix — final review の指摘（(6) LoC の値と地図の行の無い file の一覧）を文書で直す
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: done
updated: 2026-11-18
completed: 2026-11-18
---

# loc-fix（WorkUnit）

完了日: 2026-11-18。final review（01M7PZYRW7RQY1NCHTDKQ1197R）の criterion 3 の不合格を文書だけで直した。code・profile・`scripts/`・`web/` は変えていない。

## 結論

- (6) LoC は提出時の HEAD（件名「integrate wu/closeout (phase close)」）で測り直した。production 全言語 375,099 → 375,763（+664）、production Rust
  330,408 → 331,072（+664）、own_total_with_tests 1,026,070 → 1,028,966（+2,896）。目安 400 行の超過は 264 行。前の版（+662・+662・+2,102）は件名
  「integrate wu/prompts (phase impl)」の commit の値で、cases の commit（production +2・試験 +792）が入っていなかった。進捗・metrics・ADR に「訂正」と書いた。
- before..HEAD に C 以外の task の commit は無い（非 merge 30 件は C の WorkUnit・前提の task 01M7NZW0BS・operator の hand merge だけ）。repair の申告は要らない。
- 地図の行の無い高 risk の file は、`owner_rows` の規則で判定すると 337 件中 233 件（production 58 件）。前の版の一覧のうち `cos/operations.rs`・`worker_finish.rs`・
  `browser_policy.rs`・`transition.rs`・`celeris/src/instance.rs`・`handlers/authority.rs` は行があり、誤りだった（`worker_finish.rs`・`handlers/authority.rs` は facade の行）。
- 進捗の「提案」に reviewer の申し送り（authz の正規表現が `cos/operations/instructions.rs` を覆わない、advisory 3 件）を足した。

## 変えた file

- `agent-docs/progress/2026-11-17-arch-v4-c.md`: 結論の (a)、計画の表（この WorkUnit の行）、§3 の (6) の行と測定コマンド・訂正、§4 の規模の超過、「未解決」の規模と地図の行、「提案」。
- `agent-docs/progress/2026-11-17-arch-v4-c/metrics.md`: after の定義（提出時の HEAD。(1)〜(5) は測り直していない旨）、結論の (4)・(6)・(a)、§6（表・commit ごとの
  production の内訳〈cases の commit を含む〉・file ごとの内訳・試験の純増の内訳〈`boundary_cases.rs` ほか〉）、再現コマンド。
- `agent-docs/adr/2026-11-17-boundary-change-risk-review.md`: 付記（実装）の「規模」と「既知の限界」の地図の行。
- この file。

## (6) LoC の測定

```sh
BEF=$(git rev-parse "$(git log --format=%H --grep='^ADR: PathClass に任意の欄 invariants を先に足す決定' -n 1 main HEAD)^1")   # 件名「integrate wu/closeout (phase closeout)」
git show $BEF:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $BEF:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
git diff --quiet $BEF HEAD -- scripts/dev/source-size-report.py scripts/dev/source-size-report.toml   # exit 0（before..HEAD で変わっていない）
python3 -I $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev <$BEF|HEAD> --format json [--files]
# commit ごと: 同じコマンドを <commit> と <commit>^1 に当てて views.product_only.lines の差をとる
```

| rev | production 全言語 | production Rust | own_total_with_tests | inline_test | external_test | tooling |
|---|---|---|---|---|---|---|
| before | 375,099 | 330,408 | 1,026,070 | 21,408 | 455,595 | 25,414 |
| 件名「integrate wu/prompts (phase impl)」（前の版の after） | 375,761 | 331,070 | 1,028,172 | 21,419 | 456,921 | 25,517 |
| 件名「integrate wu/cases (phase verify)」 | 375,763 | 331,072 | 1,028,966 | 21,419 | 457,713 | 25,517 |
| 提出時の HEAD（件名「integrate wu/closeout (phase close)」） | 375,763 | 331,072 | 1,028,966 | 21,419 | 457,713 | 25,517 |
| 差（HEAD − before） | +664 | +664 | +2,896 | +11 | +2,118 | +103 |

commit ごと・file ごとの内訳は [metrics.md](metrics.md) §6。commit ごとの和は +667 で、前提の task と risk-rule が同じ欄 `PathClass.invariants` を別々に足し、
統合の merge で 1 つになった分（production 3 行・試験 44 行）が純増との差になる。

## 地図の行の判定

`crates/task-dispatch/src/health_review/input.rs` の `owner_rows`・`path_tokens`・`expand_braces`・`covers` を Python に写した script（`$TMPDIR` に置いて実行した。
commit はしていない）。対象は提出時の HEAD の profile の `[review].high_risk` の正規表現（`re.search`）に合う追跡 file、地図は `[review].owner_map`（`docs/architecture-map.md`）。

```python
import re, subprocess, sys, tomllib
repo, rev = sys.argv[1], sys.argv[2]
show = lambda p: subprocess.run(["git", "-C", repo, "show", f"{rev}:{p}"], capture_output=True, text=True, check=True).stdout
review = tomllib.loads(show("scripts/dev/repo-health/profiles/celeris.toml"))["review"]
files = subprocess.run(["git", "-C", repo, "ls-tree", "-r", "--name-only", rev], capture_output=True, text=True, check=True).stdout.split("\n")
def expand_braces(t):
    o, c = t.find("{"), t.find("}")
    if o < 0 or c < 0 or c < o: return [t]
    return [x for a in t[o + 1:c].split(",") for x in expand_braces(f"{t[:o]}{a.strip()}{t[c + 1:]}")]
def covers(t, p):
    t = t.rstrip("/"); under = lambda d: p.startswith(d) and p[len(d):].startswith("/")
    return t != "" and (p == t or under(t) or (t.endswith(".rs") and under(t[:-3])))
rows = [[t for s in r.split("`")[1::2] for t in expand_braces(s) if "/" in t and not any(ch.isspace() for ch in t)]
        for r in (l.strip() for l in show(review["owner_map"]).splitlines())
        if r.startswith("|") and not all(ch in "|-: " for ch in r)]
hit = {f: [c["class"] for c in review["high_risk"] if any(re.search(p, f) for p in c["paths"])] for f in files if f}
hit = {f: c for f, c in hit.items() if c}
missing = [f for f in sorted(hit) if not any(covers(t, f) for toks in rows for t in toks)]
print(len(hit), len(missing)); print("\n".join(f"{','.join(hit[f])}\t{f}" for f in missing))
```

`python3 -I maprows.py . HEAD` → 対象 337 件・行なし 233 件（production 58・試験 53・migration 102・`scripts/selfdeploy/` の script 19・Cargo.toml 1）。
行なしの production の file の一覧は [進捗](../2026-11-17-arch-v4-c.md) の「未解決」、ADR の付記（実装）の「既知の限界」に書いた。
地図に行がある主な境界の file: `task-api/src/knowledge.rs`・`cos/operations.rs`・`task-worker/src/browser_policy.rs`・`celeris/src/instance.rs`・`task-core/src/transition.rs`
（path そのもの）、`dispatcher/worker_finish.rs`・`handlers/authority.rs`（`dispatcher.rs`・`handlers.rs` の module dir の規則で facade の行）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → 4 path（ADR `2026-11-17-boundary-change-risk-review.md`・進捗 `2026-11-17-arch-v4-c.md`・`metrics.md`・この file）。
  `git diff --name-only "$CELERIS_WU_BASE" -- crates scripts web` → 0 行。profile（`scripts/dev/repo-health/profiles/celeris.toml`）も変えていない。
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」、repo_health_gate: all checks passed）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → ok、`bash scripts/dev/check-doc-links.sh` → ok、`sh scripts/dev/check-adr-numbers.sh` → ok（457 files）、
  `python3 -I scripts/dev/check-architecture-map.py` → OK（1,076 path）、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok。
- 判定の script（上の Python）の出力と、owner_rows の規則を関数ごとに写した版（run の artifacts の `maprows.py`）の出力は、行なしの 233 件が一致した。
- この run に main との同期は無い。

## リリースゲート・回帰条件

- リリースゲート: 未実施（統合後の release の gate は人・運用者が行う。本番への昇格はしていない）。
- 回帰条件: 文書だけの変更。進捗・metrics・ADR の (6) の値が上の測定コマンドの出力と一致すること。
