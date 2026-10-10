AUDIT runs_scanned=56 prod_path_commands=27 nondry_prune_on_prod=0 removed_files=0 removed_gib=0
CAUSE rebuilt_in_place 08:40Z の target/.rustc_info.json 更新・deps 件数増加と 08:59Z built_at の release 5b60d8b7bd80 が同じ期間にあり、09:36Z inventory は 67→11 の残存候補を示す。56 個の run 記録に本番非 dry prune/release.sh 実行はないため、再 build による mtime 更新が最も整合する。ただし全 56 個の再 build 前後 inode を結び付ける記録がなく、56 個が再生成された個別証明はない。

# prod-touch-audit: 本番 release-build 接触監査

## 対象と集計方法

仕様記載の親 task と子 7 件、計 8 workspace を対象にした。`runs/*/stdout.jsonl` は 56 file、wu 作業 log と work-unit-checks / integration-checks は全 file を再帰走査した。走査 script は `/local/celeris/data/workspaces/01M4JNZ7RDF0E32GX6HK4DAR5J/wu/prod-touch-audit/artifacts/scan_prod_touch.py`、抽出記録は同 artifacts の `scan.tsv` にある。AUDIT の `prod_path_commands=27` は stdout の tool_use に現れる本番 scratch/release state 絶対 path または dry-run 環境変数に触れた command 数。ログ内の説明文や一時 fixture の `release-build` 文字列は本番操作として数えていない。check ログと作業 log に追加で本番への削除・prune command は見つからなかった。

時刻は stdout 記録の UTC。ここでは本番 path を引数にした直接接触を抜き出した。結果は記録にある終了コードまたは出力（read-only の list/stat は成功、dry run は報告のみ）で示す。編集を含む command に本番 path の文字列が出ても、本番 path 自体を対象にしないものは除外した。

| UTC | run id | command / 動作 | dry 判定 | 結果 |
|---|---|---|---|---|
| 08:27:41Z | 01M4JEH7S9ZX2Q1XVDJ8WHTN6V | `ls` と `find` で release-build deps の 50 MiB 超を集計 | read-only | exit 0 |
| 08:27:49Z | 同上 | deps の実行 file を package 名 / integration 名に分類 | read-only | exit 0 |
| 08:37:38Z | 01M4JFB4B1N7M79QB9WCJJPW1N | release-build deps を `ls` | read-only | exit 0 |
| 08:37:42Z | 同上 | `.celeris-release-build-start`、target、prune 関数を確認 | read-only | exit 0（marker は当時無く、別 start marker を追跡） |
| 08:38:00Z | 同上 | prune 関数を変更する command。実行先は repo の `lib.sh`、prod path には書き込みなし | 該当なし | command 成功 |
| 08:40:57Z | 同上 | 試験 fixture を変更する command。実行先は repo の test file | 該当なし | command 成功 |
| 08:42:33Z | 同上 | `.celeris-release-tree`、`.rustc_info.json` mtime、deps 件数を `cat/stat/ls` | read-only | `deps` は観測間に 3782→3800。marker 08:40:45Z |
| 08:42:42Z | 同上 | `SD_RELEASE_PRUNE_DRY_RUN=1` を設定し、本番 target に対し prune 関数を実行 | **dry** | 関数 log は `dry run: would remove`。unlink は条件分岐で実行されず。deps 数は実行前後不変 |
| 08:42:57Z | 同上 | cargo metadata と本番 deps の stat /分類集計 | read-only | exit 0。削除処理なし |
| 08:43:56Z | 同上 | ADR 付記を repo に書く command（本文に本番実測値を記述） | 該当なし | command 成功、本番 path 書き込みなし |
| 08:45:59Z | 01M4JFSEVW35ZSWBXSGDX8F60H | `ls` scratch targets、lease を `cat` | read-only | exit 0。`rm -rf` は `$TMPDIR/rprobe` のみ |
| 09:33:43Z | 01M4JJHA9EBV81ANDEXY95RX1H | script/進捗と deps directory の存在確認 | read-only | exit 0 |
| 09:54:02Z | 01M4JKQB8RTYX447A20H1WN0J1 | deps ディレクトリ確認 | read-only | exit 0 |
| 09:54:09Z | 同上 | prune 関数と `find`/`du` で現状計数 | read-only | exit 0 |
| 10:27:39Z | 01M4JNMQK99F3E1SBWFWSB58X7 | scratch targets と lease の一覧 | read-only | exit 0 |

残る接触 command は run 01M4JEYFTJ1J842BEWBJTDVXQR 08:32:38Z の check 定義生成、01M4JFB4… の ADR/commit、09:34:50Z の計画生成、09:54:02Z の進捗参照、10:31:57Z の仕様文作成で、本番 scratch を読み書きする shell 部分はない。全 27 command の全文・run/time は `scan.tsv` を参照。

## 非 dry 削除の有無

対象 8 workspace の run・check・wu log から、本番 `release-build` に対する非 dry `prune`、`rm`、`find -delete`、または `release.sh` 実行を確認できなかった。よってこの task 群に帰属できる削除は **0 file / 0 GiB**。本番を対象にした prune 実行は 08:42:42Z のみで、`SD_RELEASE_PRUNE_DRY_RUN=1` が shell から Python に渡り、Python の `if not dry: os.unlink(p)` が false になる。関数は候補数/allocated bytes を報告するだけで、`SD_RELEASE_PRUNE_DRY_RUN=1` では unlink しないことを commit `38dcb248` の `scripts/selfdeploy/lib.sh`（783–853 行）で確認した。

## 67→11 と 08:40Z build の由来

08:40:45Z の marker を持つ本番 target は run `01M4JFB4B1N7M79QB9WCJJPW1N` の 08:42:33Z 観測で `.rustc_info.json` の mtime も 08:40:45Z、deps entry 数は 1 分で 3782 から 3800 に増えている。これはその時間に cargo build が走った証拠。現在の read-only 状態では release `5b60d8b7bd80` の `manifest.json` が `built_at=2026-10-10T08:59:22Z`、release directory は 09:00Z、lease の target/deps mtime は 08:59:05Z。`state/releases` の一覧は `1904b3c249c9`（06:34）、`70efa238ad18`（08:09）、`5b60d8b7bd80`（09:00）、`fe20a5a76b04`（12:03）。よって 08:40Z build は少なくとも現存 release 5b60d8b7bd80 の build と時間的に整合する。一方、指定 8 workspace の stdout/check/log に 08:40Z の `release.sh` 起動者を特定する command はないため、起動元 run/人は特定不能。

08:40Z 時点の実測候補 67 個・15.4 GiB は、09:36Z の候補 11 個・0.095 GiB に減った。現在 checkout の `release-build-deps-inventory.tsv` は 199 workspace executable を記録し、`older_than_marker=true` は 11 行、allocated bytes 102,502,400（約 0.095 GiB）。候補から消えた 56 件を削除した記録はなく、この task の実行記録にも本番削除は無い。さらに cargo が同名/hash file を再生成すれば mtime は新しくなり marker より古い候補から外れる。08:40Z の mtime 更新と増加中の deps、08:59Z build、後の inventory と整合するため cause は `rebuilt_in_place` と判定した。ただし 67 件それぞれの inode/mtime の前後対応を保存した原票がなく、56 件すべての個別再生成までは立証できない。

この調査で現行候補判定が integration test binary を漏らすとは確認しなかった。08:28Z review 指摘を受けた `prune-workspace` で候補名は全 workspace の `cargo metadata` target 名に拡張されている。別途、仕様の inventory fixture は 199 workspace executable のうち 11 件だけが marker より古いと示す。
