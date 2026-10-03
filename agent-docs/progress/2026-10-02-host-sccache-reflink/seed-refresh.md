---
tasks: [01M3YD2Z585N1YCBZK4AH8QXR0]
---
# seed の更新と GC の実装（ADR-0129 (4)(5)、2026-10-02）

(4)(5) の実装で決めた細部。コードは `crates/task-worker/src/scratch/seed.rs`、`crates/task-dispatch/src/scratch_gc.rs` の `refresh_seeds`、`crates/task-dispatch/src/dispatcher/housekeeping.rs` の `seed_housekeeping`。

- **時期**: dispatcher は起動直後に 1 回、以後 `SEED_CHECK_INTERVAL_SECS`（300 秒）ごとに確かめる。昇格は新しい release の daemon の起動なので、起動直後の確認が「昇格のとき」に当たる。対象は登録された全案件の local の git repo（`default_branch` の設定、無ければ検出した既定ブランチの commit）。commit の根に `Cargo.toml` が無い repo には seed を作らない。manifest の commit・`[scratch.cargo]`・`rustc -V` のどれかが違えば作り直す。`[scratch] seed_reflink = false` なら更新も GC もしない。
- **build**: 専用の checkout `seeds/<key>/checkout`（`git worktree add --detach`、以後は `checkout --detach --force` で進める）で `cargo build --workspace --all-targets` を `CARGO_TARGET_DIR=seeds/<key>/.building-gen-<commit12>-<nanos>/target` と `[scratch.cargo]` の env で走らせる。別スレッド（同時に 1 本）で、tick は待たない。LLM は呼ばない。
- **差し替え**: build と manifest（`size_bytes` を含む）が揃ってから、pool の `.lock` の下で `.building-*` を `gen-*` へ rename し、`current` を一時 symlink の rename で切り替え、旧世代を `seeds/.deleting-*` へ退避する（中身の削除は lock の外・削除スレッド）。owner への写しも `.lock` の下なので、写している最中の世代は消えない。既に写した owner の target は独立した path で、切り替えの影響を受けない。失敗したら作りかけを消し、旧 seed を残す。
- **排他**: 同じ repo の更新・GC は `claim_seed`（同じ process の中は static の集合、process の間は `.refresh.lock` の POSIX record lock）で排他する。flock は fork した子に引き継がれ、並行して子を起こす daemon では解放直後に「使用中」と誤るので使わない。取れなければ更新は次の確認へ回す。
- **GC**: owner の semantic GC（`targets/` の走査）は `seeds/` を見ない（緊急 GC でも）。seed の GC は current 以外の世代と放棄された `.building-*` を消す。登録から外れた repo には `.unregistered-since` を書き、`SEED_UNREGISTERED_GRACE_SECS`（7 日）を過ぎたら seed 全体を退避する。登録に戻れば印を消す。
- **容量**: seed は `targets_bytes`（`targets_max_gb` の判定）に数えない。filesystem 全体の空きには含まれる。更新は、pool が watermark を超えている、または空きが `min_free_disk_mb + 今の seed の size_bytes` を下回るなら保留する。旧 seed は残し、journal に警告を出す（満杯の瞬間に DB へ書かない ADR-0075 D2 に合わせ、通知は出さない）。
