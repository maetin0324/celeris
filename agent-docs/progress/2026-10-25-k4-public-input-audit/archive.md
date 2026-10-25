---
title: K4 公開入力の新 archive（派生物の除外）
tasks: [01M5SRJMA9CFNG2C9KCCS5VS4B]
status: done
updated: 2026-10-25
---
# K4 公開入力の新 archive（派生物の除外）

完了日: 2026-10-25

## 要約
- 監査（`audit.md`）の除外案どおり、派生 85 件を除いた新しい tgz を作った。旧 tgz は変えていない。
  - 旧: `/local/celeris/data/workspaces/01M5KVHA4222TK6VEKW599SNMA/artifacts/mlsys-main-4/measure-src/main/inputs/main-v3-n2-43706.tgz`
    sha256 `128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1`（5761 entry）
  - 新: `/local/celeris/data/workspaces/01M5SRJMA9CFNG2C9KCCS5VS4B/artifacts/k4-inputs-v2/main-v3-n2-43706-public2.tgz`
    sha256 `0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e`（5676 entry）
- 除外リスト: `.../artifacts/k4-inputs-v2/exclude.txt`（1 行 1 path、85 行。audit.json の `exclude_proposal` を名前順に写したもの）
- manifest: `.../artifacts/k4-inputs-v2/manifest.json`（`changed: true`、新旧 sha256、残した 5676 entry と除いた 85 entry の sha256、除外理由）
- 判断要 26 件（runner の実行コード、`review28/qstat-43722.txt`、`review28/v3-submissions.json` など）は人の決定が出ていないので残した。

## 作り方
`python3 -I crates/campaign/scripts/build-k4-public-archive.py --src <旧tgz> --exclude <exclude.txt> --out <新tgz> --manifest <manifest.json>`
- entry は名前順。mtime=0、uid/gid=0、uname/gname は空、mode は旧版のまま。PAX 形式で、gzip header も mtime=0・file 名なし。
- 除外理由は script の path 規則（`EXCLUDE_REASONS`）で決める。規則に当たらない path（原票）を除外リストに書くと失敗する。
  旧 tgz に無い path や、dir を除くのにその下の entry を書いていない場合も失敗する。
- 書いた後に新 tgz を読み直し、残した全 entry の sha256 が旧版と同じことを確かめる。旧 tgz と同じ file には書かない。

## 残したもの・除いたもの
- 残した: `review28/qstat-43706.txt`（grader の `cells_spec.jobs`）、`runs/43706-20261008T045834Z/` の原票 5672 件（`planned-cells.json`、`cells/*` の `cell.json`・`exit.json`・`stdout.log`・`ior.json`・`stderr.log`・`mdtest-ranks.csv` を含む）。
  review28/ に残るのは `qstat-43706.txt`・`qstat-43722.txt`・`v3-submissions.json` と dir の entry（review28 全体は消していない）。
- 除いた（85 件）: review28/ の集計物・判定 script・`__pycache__` 18 件、`runs/*/fs/**/logs-audit.json` 31 件、`runs/*/cells/**/nic-audit.json` 30 件、
  `runs/*/runner/` の `summarize.py`・`audit-nic.py`・`__pycache__` 6 件。

## 証拠（提出時の HEAD で実行）
- 2 回の生成: artifacts の新 tgz と `$TMPDIR` への 2 回目がどちらも上記の新 sha256 と全桁一致
- 旧 tgz の `sha256sum`: 生成の前後とも上記の旧 sha256 と全桁一致
- audit.json との照合（`python3 -I`）: grader が読む 147 件はすべて残っていて sha256 も同じ。残した file の sha256 の不一致は 0 件。
  除いた集合は `exclude_proposal` と一致し、全件に理由がある。判断要 26 件はすべて残した
- `tar -xzf`（grader の `extract_tgz` と同じ GNU tar）で展開できる。file 5675 件（新 tgz の entry 5676 件から dir 1 件を除いた数）
- `python3 -m unittest crates/campaign/scripts/test_build_k4_public_archive.py` → 5 tests OK
  （同じ入力から同じ byte列ができること・原票の bytes が保たれること・旧 tgz に無い path、原票の除外、子を書いていない dir、旧 tgz への上書きを拒否すること）

## 変更
- `campaign: K4 公開入力の派生物を除く決定的な archive 生成 script を足す` — script・unit test・本ファイル

## 未解決
- case.json の `inputs[].sha256`、grade-case の `recompute.input_archive`・`pinned_raw`、private の更新は case 段で行う。
- 判断要 26 件を除くかどうかは人の決定を待つ。除くことになったら、exclude.txt に足して script の規則も足し、作り直す。
