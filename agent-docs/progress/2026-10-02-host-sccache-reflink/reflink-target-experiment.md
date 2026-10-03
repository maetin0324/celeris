# target コピー後の Cargo fresh 判定実験

---
tasks: [01M3YD2Z585N1YCBZK4AH8QXR0]
---

## 条件と結果

2026-10-02、`bash scripts/dev/reflink-target-experiment.sh` を実行した。Cargo 1.98.1、rustc 1.98.1、`RUSTC_WRAPPER=/var/lib/celeris/scratch/bin/sccache`、`CARGO_INCREMENTAL=0`。外部アクセスを避けるため全 build に `--offline` を付けた。`CARGO_TARGET_DIR` の値は変更せず、その配下の `reflink-exp/` に小さな workspace と target を作り、終了時に削除した。

実験の workspace は app、path 依存の pdep（`build.rs` が `rerun-if-changed` を出し `OUT_DIR` に生成）、registry 由来の itoa、libc、anyhow から成る。seed の target を `cp -a --reflink=auto` でコピーした。実験場所のファイルシステムは ext 系で、今回のコピーは実体コピーだった。`/local` は btrfs だが、この run からは読み取り専用であり、そこへのコピーは実行していない。`btrfs` と `filefrag` コマンドもこの run には無いため、`btrfs filesystem du` と extent の共有状態は未計測。

| 条件 | Compiling 行 | 時間（秒） | Compiling した crate | コピー前→後の `df` 使用量（MiB） | コピー先の `du` |
|---|---:|---:|---|---:|---:|
| seed を空から build | 5 | 0.74 | anyhow, app, itoa, libc, pdep | 対象外 | 対象外 |
| 対照: 同じ source・同じ target | 0 | 0.04 | なし | 対象外 | 対象外 |
| (1) 同じ source・別 target | 0 | 0.03 | なし | 309837.2→309864.7（+27.6） | 28 MiB |
| (2) 別 source・別 target、`cp -a` で mtime 保持 | 0 | 0.03 | なし | 309864.8→309892.3（+27.6） | 28 MiB |
| 補助条件: 別 source・別 target、mtime 更新 | 2 | 0.23 | app, pdep | 309892.4→309920.0（+27.6） | 28 MiB |
| (3) 別 source・同じ target path にコピーを戻す | 0 | 0.03 | なし | 309920.0→309947.6（+27.6） | 28 MiB |

`df` はファイルシステム全体の使用量であり、並行する書き込みの影響を受ける。各コピーの `du` は見かけの占有量で、extent の共有を証明しない。開始時の `df` は 309729.4 MiB、終了時（削除前）は 309947.6 MiB。生の標準出力は run の成果物ディレクトリの `reflink-run.log` に保存した。

人の 2026-10-02 13:45 の追記では、container の `/local` で `FICLONE` / `FICLONERANGE` は EPERM だが、`copy_file_range(2)` は btrfs 上で extent を共有し、512 MiB のコピーで `df` 増分 0 MiB、`filefrag` に shared と表示された。GNU coreutils 9.5 の `cp -a --reflink=auto` も、`FICLONE` 失敗後に `copy_file_range` へ落ちて同じ結果だった。これは人の別途の実測であり、この run の表の結果ではない。

## 結論

mtime を保った target と source のコピーでは、target path と source path が変わっても、この小さな例の registry 依存・path 依存ともに作り直されなかった。従ってこの条件では fingerprint の絶対 path を理由とする再ビルドは確認されない。ただし新規 checkout を模した mtime 更新では pdep と app が再ビルドされた。Cargo `-v` の理由は `pdep/build.rs` が前回 build より新しいこと、および app の依存 pdep が再ビルドされたことだった。registry 依存 3 crate は再ビルドされなかった。

実際の worktree の mtime が変われば、path 依存とその利用側の再ビルドは残りうる。seed 方式の節約効果は依存の構成と checkout の mtime に左右される。`--remap-path-prefix` や target path を揃えるだけでは、この mtime による Cargo の dirty 判定は解決しない。`cp --reflink=always` の成否も `/local` での共有可否の判定には使えない。seed からのコピーには `cp --reflink=auto` を使い、実際の共有は `df` の増分または `filefrag` の shared で確かめる必要がある。
