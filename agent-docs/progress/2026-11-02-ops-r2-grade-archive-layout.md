---
title: ops-r2 採点 archive の配置（tree は src/ の下）を isolation 文書に書く
tasks: [01M6DR4ADPVWJ6MDDYAQA2CK44]
status: done
started: 2026-11-02
updated: 2026-11-02
completed: 2026-11-02
---
# ops-r2 採点 archive の配置（tree は src/ の下）を isolation 文書に書く

完了日: 2026-11-02

## 変更
- `docs: 採点 archive では試行の tree を src/ の下に置くと ops-r2 の手順に書く` — `docs/ops/ops-r2-isolation.md` §4.3 に配置規則、`src/` の下に入れる tar command と確認 command、root に置いたときの誤判定 2 通りを書いた。`docs/ops/ops-r2-k2-preflight.md` §4 から §4.3 へ案内を足した。ADR 2026-10-30-ops-r2-isolation-principal に付記 H9 を足した。

## 確かめた事実
- K2 第 1 回の判定 JSON（A・B の 2 件。漏洩検査を通った outbox の file）は A・B とも `fail`・`model_failure`、implementation の先頭の reason は `evaluation command exited with Some(2)`。`artifact repo ... is missing` は出ていない。
- 第 2 回の採点 archive では `src/Cargo.toml` と `src/src/` があり、`src/src/Cargo.toml` は無い。root に置くと crate 自身の `src/` が repo と見なされることと合う。
- 隠し試験と private の中身は読んでいない。

## 他の archive の確認
- 採点 archive を作る script は repo に無い。`scripts/campaign/ops-r2-seed.sh` が作るのは試行の初期の種で、採点 archive ではない。
- 採点 archive の作り方を書いた文書は `docs/ops/ops-r2-isolation.md` §4.3 だけ。`docs/ops/ops-r2-k2-preflight.md` §4 は wrapper の動作だけを書いていたので §4.3 への案内を足した。
- `docs/ops/ops-r1-pipeline.md` の `--a-artifact`・`--b-artifact` は tgz を作らず dir を写すので、変えていない。implementation の検査がある case をこの経路で採点するときの配置は未確認。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし、docs のみ。check-doc-links self-test ok）
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh` → exit 0、`sh scripts/dev/progress-index.sh --check` → exit 0
- 手順の tar command を一時 dir の試験用 tree（隠し file・symlink・hard link を含む）で実行 → 全 member が `src/` の下、symlink・hard link は通常 file になった。wrapper の `safe_extract` と同じ判定（file か dir か）を通る。実 wrapper への投入はしていない。

## 未解決
- root 配置の tgz は判定 JSON の形が正常で、`model_failure` に数えられる。今回は文書だけで防ぐので、人が手順を外せばまた起きる。

## 提案
- grader の implementation の検査で `<repo>/Cargo.toml` のような case ごとの目印を確かめ、無ければ `model_failure` ではなく入力の誤りとして判定を作らない。今回の task の範囲外なので実装していない。
