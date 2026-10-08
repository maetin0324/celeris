---
tasks: [01M4D3XKDXJARTS46KHFXDWY16]
---
# CoS chat 最適化: skill delivery の差分コピー

`task-worker::skills::deliver_to` に内容照合による再コピー省略を実装した。Claude Code と Codex/ACP の共通経路に適用し、配送される skill の内容と公開 API は維持する。

- marker v3 は `roots[root][skill] = SHA-256` の形式。v1 の名前配列と v2 の root 別名前配列を読み、hash が無い旧写しは一度コピーして v3 に移行する。
- hash はソートした相対 path・entry 種別・bytes を長さ付きで取り込み、空ディレクトリも区別する。コピー元の symlink・特殊 file は従来のコピーと同じく辿らず省く。
- 省略条件は所有印 `.gitignore == COPY_IGNORE`、marker の元内容 hash 一致、届け先の実内容一致のすべて。元の `.gitignore` 自体も marker の hash に含め、届け先との比較では生成される所有印へ置換した期待 hash を使う。届け先の symlink・特殊 file は一致扱いしない。
- 更新は同じ root 下の一時 dir に全コピーと所有印を完成させ、既存 dir を退避してから rename する。コピー途中の失敗では既存 dir を維持し、一時 dir を除去する。rename 失敗時には退避した既存 dir を戻す。復元も失敗した場合は完全な旧コピーを退避先に保持し、場所を error で返す。marker も一時 file から rename して更新する。
- 未所有の `.agents/skills` 同名衝突、未 mount skill の削除、adapter 切替の旧 root 掃除、AGENTS.md の celeris 節、無効名拒否を維持した。旧 root の掃除は新コピー成功後に行う。

検証:

- `cargo test -p task-worker skills 2>&1 | tee /dev/stderr | grep -qE 'test result: ok\. [1-9]'`: exit 0、39 passed / 0 failed。mtime/inode 不変、元内容変更、届け先の改変・追加・欠落・所有印欠落、削除、adapter 切替、未所有衝突、v1/v2 移行、symlink、コピー途中失敗の保全を含む。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `git diff --check`: exit 0。

本番 config/DB/KB/release/systemd の操作と LLM 呼び出しは行っていない。実 chat の待ち時間・token・課金への効果は不明。今回の確認対象は隔離した一時ディレクトリでの配送動作のみ。
