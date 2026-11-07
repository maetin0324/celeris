# repo health の Celeris profile と単一入口（check-health.sh）

- 日付: 2026-11-07
- 状態: 採用（実装済み）。[ADR 2026-11-07 汎用 repo health の契約](2026-11-07-repo-health-contract.md) の D1・D2・D5 への付記。
- 範囲: 健全性 v2 G1 の WorkUnit celeris-profile（task 01M6TYVJSRM3SFQ3VHWQE4MZ9M）。

## 決定

### D1. root と base は repo_health.py の snapshot 1 か所で決める
- `repo_health.py --base auto` は `health/snapshot.py` の `default_base` が `git merge-base HEAD refs/heads/main` に解決する。`--print-base` はその sha を出すだけ。
- 既存検査の script（`check-source-size.py`・`check-architecture-map.py`・`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh`・`check-human-text.py`）は repo の root を探さない（`git rev-parse --show-toplevel`・`__file__` からの算出を消した）。root は `--root`（既定 `.`）、base は `--base`（`check-source-size.py` は必須、`progress-index.sh --check`・`check-adr-numbers.sh --refs` は省けば `repo_health.py --print-base`）で受け取る。`CELERIS_WU_BASE` はどの検査も読まない。
- 既定の base を WorkUnit の base でなく main との merge-base にした理由: policy は base 側で読む（契約 D2）。WorkUnit の base は task branch の未 review の commit で、そこにある policy を正としない。承認済みの policy は main にある。契約 D1 の「WorkUnit では `$CELERIS_WU_BASE` を渡す」はこの付記で改める（範囲を絞りたい呼び出し側は `--base` に明示する）。
- 採らなかった: 各 script に `CELERIS_WU_BASE` → merge-base の解決を残す案（同じ判断が 4 か所に並び、食い違う。実際に source-size だけが WU base を使っていた）。

### D2. 既存検査は profile が書いた command を rule として走らせる
- `rules/existing_checks.py` が rule `doc.links`・`adr.numbers`・`progress.index`・`human-text`・`source-size` を登録する。走らせる argv は profile の `rules."<id>".commands` にだけ書く（検査器のコードは repo 固有の script 名を持たない）。`{root}`・`{base}` を snapshot の値に置き換える。command が無ければ not_applicable、起動できなければ unsupported、非 0 は fail。head だけで評価する（今の「全部通る」を合格の条件のまま保つ）。
- commit の head は展開先を独立した Git の作業ツリーにして走らせる（object は alternates、branch の ref は写す）。command には `GIT_*` を渡さない。`GIT_DIR`・`GIT_WORK_TREE` を渡す最初の実装では、command の中の試験が作る一時 repo の `git init`・`commit` が対象 repo の config・branch を書き換えた。
- architecture-map は arch-map.* の rule（`check-architecture-map.py` の `find_diffs`）で見る。地図の path の不在は arch-map.wrong_path で、base に無い食い違いだけが fail になる。

### D3. Celeris 固有の値は profiles/celeris.toml にだけ置く
- `profiles/default.toml` は `extends = "celeris"` だけ。`celeris.toml` が policy_paths（検査器・包んだ script・baseline・地図の対照）、crate の層と許可する依存、size の上限、構造指標（TaskStore・Dispatcher。B0 の `slices.json` の structural_metrics と同じ数え方）、候補 rule の例外（理由付き）、既存検査の command を持つ。
- profile の新しい項目 `[metrics."<name>"]`（path・item・start・end・source）は report の `metrics.structural` に base と head の値を書く。判定には使わない。

### D4. check-health.sh が単一入口
- `bash scripts/dev/check-health.sh [--base <rev>] [--out <report.json>]` は `repo_health.py --base auto --head worktree --policy-version 1` を呼び、report の path を出し、repo_health.py の exit code（0・1・2）で終わる。合格なら続けて検査器の unit 試験とゲートの試験（`scripts/dev/tests/repo_health_gate.sh`）を走らせる。doc-only の変更ではそれも省く。
- policy の file を変える branch は policy.changed で落ちる（契約 D2）。main に profile が無い間だけ bootstrap（warning）になる。
