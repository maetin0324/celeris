# ADR-0129: sccache を host の Cargo 設定へ移し、btrfs の seed から target を作る

---
tasks: [01M3YD2Z585N1YCBZK4AH8QXR0]
---

- 日付: 2026-10-02
- 状態: Accepted（実装と host への適用は後続工程）
- 関連: [ADR-0075](0075-tiered-build-cache.md) D1–D6、R7-7、R7-8
- 実測: [target コピー後の Cargo fresh 判定実験](../progress/reflink-target-experiment.md)

## 1. Celeris からの sccache 撤去と互換

Celeris は Cargo の target 所有者と GC を引き続き管理するが、sccache の所有者にはならない。dispatcher と検査経路から `RUSTC_WRAPPER`、`RUSTC_WORKSPACE_WRAPPER`、`SCCACHE_*` の差し込み、上書き、環境からの除去を外す。`celerisctl scratch env` は target と `[scratch.cargo]` の設定だけを出し、`--server` と sccache backend の選択・記録を廃止する。Celeris が継承した host の Cargo 設定と環境変数は通常どおり子へ渡す。`CARGO_TARGET_DIR`、`CARGO_INCREMENTAL`、`CARGO_PROFILE_DEV_DEBUG` と owner ごとの target 分離は維持する。

`crates/scratch-cache`（専用 WebDAV L1/L2）、`crates/celeris/src/cache_server.rs`、`celeris cache-server`、`celeris-scratch-cache.service`、`celeris-sccache.service`、Celeris が作る wrapper・token・backend mode・sccache 計測を撤去する。`[scratch.sccache]`、`[scratch.cache_server]`、`[scratch.l2]` とその下の関連項目を有効設定から外す。旧 config のこれらの節は起動エラーにせず読み飛ばし、廃止された設定として一度警告する。新 config には書かない。既存の `scratch status` / API の sccache・cache server 欄は破壊的な schema 変更として後続の schema と利用側を同時に更新し、旧欄を残す移行期間を設けるなら `null` とする。既存 client を黙って別の値へ読み替えない。

worker の preamble / プロンプトで「Celeris の sccache を起こさない・止めない」「`RUSTC_WRAPPER` / `SCCACHE_*` を固定する」という規則を削る。worker が受け取った `CARGO_TARGET_DIR` を勝手に変えない規則は残す。旧 scratch の `sccache-l1/`、`cache-l1/`、token 等は移行時に参照元がなくなったことを確認してから人が削除する。daemon 起動時に自動削除しない。

## 2. host 側の Cargo と sandbox

host 管理者が `~/.cargo/config.toml` の `[build] rustc-wrapper` に `~/.local/bin` の小さな wrapper を指定する。Cargo config は `~` を展開しない前提で絶対 path を記す。wrapper は最初の引数（実際の rustc）と残りを受け取り、自分の実行場所から設定済みの sccache server への到達を試す。到達しなければ最初の引数を直接 `exec` する。到達すれば sccache を `exec` する。通信不可を rustc の失敗と混同して compiler を二重実行しない。probe に sccache client を使わず、worker から server を自動起動しない。これは ADR-0075 R7-7 の挙動を host 側へ移すものである。wrapper の名前・`CARGO_TARGET_DIR` を sccache の key から外す挙動は既存実装を踏襲する。

server は Celeris の unit と依存関係を持たない user unit とし、`SCCACHE_DIR` は `/local` の host 管理領域に置く。`/local` が使えない間は host 管理者が別のローカル保存先を設定するか、wrapper が素の rustc に戻る。unit の起動・設定・再起動は人が行う。host の `~/.cargo/config.toml` は worker sandbox から読めるとは限らない。Codex など独自の `CARGO_HOME` を使う worker ではその config と wrapper の読み取り・実行可能性を個別に確認し、必要な sandbox の read access を設定する。server の TCP/UDS が seccomp 等で塞がれる場合は wrapper が素の rustc に戻る。server を使うためだけに worker のネットワーク権限を自動で広げない。`CARGO_TARGET_DIR` の書き込み許可は ADR-0075 R7-8 のまま維持する。

## 3. scratch の `/local` への移行

`[scratch] dir` を設定可能なままにし、`/local` の btrfs mount と書き込み権限を人が確認した後に `/local/celeris/scratch` などへ切り替える。`/local` がない環境の既定値は現行の scratch path のままとする。マウントが失われた場合に root filesystem 上へ同名ディレクトリを作らないよう、切替後は mount point と filesystem を起動時に検査し、満たさなければ scratch を無効化して理由を表示する。移行は daemon 停止中に既存の lease と target を保存して行い、切替後の owner path・lease・空き容量を確認してから再開する。Proxmox host の LVM-thin volume の作成、btrfs format、container への bind mount は人の作業であり、Celeris は実行しない。

## 4. seed と task・WU の target

seed は repo key ごとに `<scratch>/seeds/<repo-key>/current/target` と manifest（main commit、Cargo/rustc profile、作成日時）を置く。repo key は既存 lease と同じ正規化を使い、異なる repo の seed を混ぜない。main の固定 commit を専用の安定した checkout で build し、既存 owner の target を動かさず新しい seed を作る。main が進んだ後、および release 昇格で新しい main が有効になった後に更新を試みる。古い main と seed が一致しなくても通常の build は続け、次の更新で追いつく。build 成功後だけ、同じ filesystem に置いた世代ディレクトリへの `current` symlink を一時 symlink の rename で原子的に切り替える。`current` が指す旧世代はコピー中の参照がなくなるまで保持する。切替・owner 作成・GC は scratch lock の下で直列化し、コピー中の seed を削除しない。失敗時は旧 seed を残す。

新しい task または WU の空の owner には、互換な seed から `cp -a --reflink=auto` で target を一時ディレクトリへ写してから公開する。container の `/local` では `FICLONE` / `FICLONERANGE` が EPERM でも `copy_file_range(2)` が btrfs extent を共有するため、`--reflink=always` の成否を判定に使わない。`cp --reflink=auto` が通常コピーへ落ちた場合は一時ディレクトリを削除し、空の target から始める。共有の判定は実際のコピーについて `filefrag` の shared 表示、または十分大きい試験ファイルのコピー前後の `df` 増分で行い、判定できない場合も空から始める。`du` や `st_blocks` の合計は共有の証明にならない。既存 owner の target は seed で上書きしない。seed の compiler version、profile、Cargo 設定が違えばコピーせず空から始める。

[実験](../progress/reflink-target-experiment.md)（Cargo/rustc 1.98.1、`CARGO_INCREMENTAL=0`）では 28 MiB の seed を ext 系 filesystem で実体コピーした。同じ source・別 target で `Compiling` は 0 行、0.03 秒、`df` は +27.6 MiB。source path も変えて `cp -a` で mtime を保持した場合も 0 行、0.03 秒、+27.6 MiB。source の mtime を更新すると path 依存の pdep と app の 2 crate が再ビルドされ、0.23 秒だった。最初の空 build は 5 crate、0.74 秒。同じ target の対照は 0 行、0.04 秒。したがって target と source の絶対 path が違うだけでは、この小さな例での依存再ビルドは確認されなかった。ただし実際の worktree の mtime が新しければ path 依存とその下流は再ビルドされる。`--remap-path-prefix` や target path の統一だけではこの dirty 判定を消せない。seed の効果は registry 依存の割合と worktree の mtime に制限される。

上記実験は btrfs 上の extent 共有を測っていない。人の 2026-10-02 の別実測では、container の `/local` で 512 MiB を `copy_file_range` および GNU coreutils 9.5 の `cp -a --reflink=auto` で写すと、`df` 増分は 0 MiB、`filefrag` は shared を示した。この環境では `cp --reflink=always` は EPERM になる。実装後に同じ方法で end-to-end の共有を再確認する。

## 5. ADR-0075 D2 の GC と容量

seed は lease 付き owner ではなく、D2 の P0–P3 と旧「最新の回収可能 owner を seed として adopt」の候補から外す。repo ごとの `current` は通常の owner GC の対象外とするが、scratch 全体の物理使用量と空き容量には含める。更新済みの旧 seed は active なコピーが終わってから削除できる。main の旧 commit に戻すための永久保存はしない。repo が登録から外れた場合は人が確認できる猶予を置いて seed 全体を削除する。

reflink 共有 extent の `st_blocks` を owner と seed ごとに足すと二重計上になる。空き容量の安全判定には `statvfs` / `df` 相当の filesystem 全体の物理使用量を用いる。`targets_max_gb` の判定に filesystem 全体の使用量をそのまま当てず、btrfs の排他的使用量を取得できる場合はそれを使い、取得できない場合は `st_blocks` 合計を保守的な上限として扱う。owner 別の `size_bytes` は表示・削除順の概算と明示し、削除で実際に戻る量を保証しない。共有 extent は最後の参照が消えるまで空きにならないので、GC は削除後の空き容量を再測定する。seed が容量を圧迫した場合は旧 seed の削除、次いで更新の保留と人への警告を行い、現行 seed を黙って失わせない。

## 6. 試験方針

通常の tmp（reflink できない filesystem）で、seed コピーを採用せず空 target に戻ること、途中失敗の一時ディレクトリと lock の回復、既存 owner を上書きしないことを決定的な試験にする。btrfs の実試験は `CELERIS_REFLINK_TEST_DIR` が指定されたときだけ実行し、そのディレクトリが btrfs であること、`cp --reflink=auto` 後に extent が shared であることを確認する。共有の証拠は `filefrag` とコピー前後の `df` を記録する。`FICLONE` が EPERM でも共有できる経路を確認し、`--reflink=always` の成功を必須にしない。Cargo fresh 判定は registry 依存・path 依存・mtime 更新を分け、`Compiling` 行、時間、物理使用量を記録する。host の unit と Cargo 設定の適用は人向け運用文書の手順で確認する。
