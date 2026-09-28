# ADR-0075: ビルドキャッシュの 2 層化 — target は使い捨ての scratch、再利用は sccache の L1（ローカル）/ L2（NFS）に集約し、Celeris が semantic cache manager になる

- 日付: 2026-09-28
- 状態: **Accepted**（2026-09-28 Phase G1 完了・本番反映、Phase G2 実装、Phase G3 実装〈cache server の有効化は人〉）
- 入力: `docs/notes/build-cache-tiering-input-2026-09-28.md`（人の設計方針。本 ADR はこれに沿う。以下「入力メモ」）
- 関連:
  - ADR-0066（D1 共有 `CARGO_TARGET_DIR=<build_cache_dir>/cargo/<repo-key>`、D2 終端の作業場所の生成物の刈り取り）。本 ADR は D1 を**置き換える**（D2 は残す）
  - ADR-0074 D1.2（WU ごとの worktree）と「Phase F5-fix 実装時の逸脱・明確化」2・3・5（WU ごとの `CARGO_TARGET_DIR`、daemon の検査にも同じ target、終端の WU の target の削除）。本 ADR はこれを**包含する**（WU の target は scratch pool の 1 entry になる）
  - ADR-0043（worktree。`<workspace_root>/<task_id>/repos/<name>/`）、ADR-0042（`~/.local/celeris/` の層）、ADR-0064（DB はローカルディスク）
  - ADR-0070 / Phase 116（infra 障害の分類。`[dispatch] min_free_disk_mb` と「ディスク不足 (infra)」の通知）
  - ADR-0060（ssh master を daemon の cgroup の外に出す。常駐プロセスを `celeris@` の unit に入れない理由）
  - `docs/ops/home-nfs-migration-2026-09-25.md`（`/home` = TrueNAS の NFS、I/O の重いものはローカルへ）

## 1. 文脈（観測した事実）

- ルートディスクはローカル LVM 1 本（`/dev/mapper/pve-vm--100--disk--0`、252G）。Celeris の DB（`/var/lib/celeris/celeris.sqlite3`）も
  同じ filesystem にある。NVMe の別ボリュームは無い。`/home/rmaeda` は TrueNAS の NFS（6.4T、1GbE）。
- 2026-09-26〜28 にルートが 4 回満杯になった。原因は Celeris の run（ADR-0066 D1 の共有 target と、F5-fix で入った WU ごとの target）、
  release ゲート（`$SD_RELEASES/.cargo-target` → `/var/lib/celeris/release-build/.cargo-target` の symlink、21G 前後）、
  実装エージェント（`CARGO_TARGET_DIR=/var/lib/celeris/build-cache/cargo/agent-platform-<name>` を手で指定）の cargo target。
- 2026-09-28 G0 時点の観測（読み取りだけ）: `df /` = 252G 中 152G 使用・91G 空き。そのうち rmaeda から見えるのは
  `/tmp/agent-platform-f5-1-target` **32G**（scratch の外に置かれた野良の target）、`/var/lib/celeris` 11G、`/usr` 2.9G 程度で、
  残り 100G 前後は rmaeda 権限では見えない（root 専用の領域か、NFS の mount の下に隠れた旧 `/home` のデータの可能性。**未確認**、人が
  root で `du -x` して確かめる）。この差が実在するなら、入力メモの「scratch 最大 150G」はそのままでは収まらない（D1 の実効上限）。
- F5-1 dogfood（3 回目）の不具合 1: 並列 WU が同じ `CARGO_TARGET_DIR` を共有し、兄弟 WU の別ブランチの `task-core` の生成物で偽の
  コンパイルエラー（E0609）。cargo の path crate の fingerprint は**ソースの mtime と dep-info の mtime の比較**なので、先に checkout
  された worktree（ソースの mtime が古い）が、後から別ブランチで作られた生成物を「新しい」と誤認しうる。共有 target は同時実行だけでなく
  **時間差の共有でも**壊れうる（D3 の adopt の安全条件の根拠）。
- 実装エージェントでも同じ混線が出ていたので、`~/.cargo/config.toml` の `[build] target-dir` は 2026-09-28 に人が廃止した
  （現在は linker の設定だけ）。`docs/ops/home-nfs-migration-2026-09-25.md` §5 の「`~/.cargo/config.toml` の `[build] target-dir` で
  指定」は古い記述になっている（D7 で訂正を提案）。
- sccache は未導入（`which sccache` が空）。

## 2. 決定

### D1. レイアウトと容量

**scratch pool**（ローカルだけ。NFS に置かない）:

```
/var/lib/celeris/scratch/                 # [scratch] dir
  .lock                                   # flock。lease の作成・adopt・rename-to-delete を直列化（短時間だけ持つ）
  targets/
    task-<task_id>/
      lease.json                          # Task 単位の run・統合 WU の検査・reviewer の checks の target の lease
      target/                             # ← CARGO_TARGET_DIR
      wu-<work_unit_id>/
        lease.json
        target/                           # ← 自分の worktree で走る v2 の WU の CARGO_TARGET_DIR
    release-<sha12>/{lease.json,target/}  # release.sh のゲート
    agent-<name>/{lease.json,target/}     # Fable 配下の実装エージェント（<name> = worktree 名、例 agent-adb0a84ce1f1afa5f）
    .deleting-<owner-flat>-<ulid>/        # GC が rename した削除待ち（別スレッドで remove_dir_all）
  sccache-l1/                             # G2: sccache の local disk cache（SCCACHE_DIR）。G3: cache server の L1
```

- **owner** は `task-<task_id>` / `task-<task_id>/wu-<work_unit_id>` / `release-<sha12>` / `agent-<name>`。依頼文の `wu-<key>` ではなく
  **WU の行の id（ULID）**を使う（F5-fix と同じ理由: key は Task の中でだけ一意で、replan で superseded になった行と同じ key の新しい行が
  できうる。行の id なら replan で持ち越した WU は同じ target を使い続け、作り直された WU は別の target になる）。`lease.json` に key も
  写して表示に使う。
- owner のディレクトリは `lease.json` と `target/` を分ける（`target/` だけが `CARGO_TARGET_DIR`。cargo は target の中の未知のファイルを
  気にしないが、GC が `target/` だけを消して lease を残す〈「刈った」記録〉ことができる）。`task-<id>/` の GC は Task の `target/` と
  各 `wu-*/target/` を**別の entry**として扱う（WU を消しても Task の target は残る）。
- `lease.json`（`celeris.scratch-lease/1`、`deny_unknown_fields`）:
  `{schema, owner, kind: task|work_unit|release|agent, repo_key, repo_path, base_commit, work_unit_key?, created_at, released_at?,
  size_bytes?, measured_at?}`。**生存の合図は lease.json の mtime**（touch）。daemon 由来の owner（task / work_unit）の生存は DB の状態で
  決まるので mtime は補助、外部の owner（release / agent）は mtime が唯一の合図（D2）。
- **上限**（`[scratch]`。既定は入力メモの 256GB の目安）: `targets_max_gb = 100`、`l1_max_gb = 40`、`total_max_gb = 150`。
  **実効上限** = `min(total_max_gb, pool 以外の使用量を除いた filesystem の容量 − min_free_disk_mb)`。pool が同じ filesystem を
  OS・DB・その他と共有する間は、pool の外の成長（DB、/tmp の野良、ログ）で実効上限が縮む。daemon は起動時と GC ごとに実効上限を計算し、
  `total_max_gb` より小さければ `scratch status` と journal に「実効上限 N GB（設定 150 GB）」と出す（黙って 150G を仮定しない）。
- **DB と同居する注意**: SQLite（WAL）は満杯で書き込みに失敗すると daemon 全体が止まる（ADR-0064）。したがって
  (1) `min_free_disk_mb`（既定 5120）は pool の上限とは**別の最後の砦**として残し、pool の GC はその手前（high watermark）で動く、
  (2) GC の削除は rename → 別スレッドなので、満杯の瞬間に DB への書き込みを増やさない（journal は tracing、DB の event は増やさない）。
- **Proxmox 側の提案（人の判断。本 ADR では決めない）**: 第一案は CT に `mp1` として**専用のボリューム**（例 LVM-thin から 200G、
  ext4、`backup=0`）を `/var/lib/celeris/scratch` に足す。利点: statvfs がそのまま pool の使用量になる（`du` の推定が要らない）、
  pool が満杯でも `/`（DB）は守られる、snapshot / backup から外せる。第二案は rootfs の拡張（252G → 400G 程度）。どちらも Celeris の
  コードは同じ（`[scratch] dir` のパスが変わらないため）。`mp1` を足すまでは同居で動く設計にする。
- **L2（NFS）**: `~/.local/celeris/cache/sccache-l2/<k0k1>/<key>.zst`（例 `ab/ab14c8….zst`）。1 entry = 1 ファイルの
  content-addressed な immutable object（D5）。`[scratch.l2] dir` で変えられる。容量は `l2_max_gb = 300`（NFS 側の LRU。D5）。
- 起動時の検査（決定的）: `[scratch] dir` と `sccache-l1/` が NFS 上（`statfs` の `f_type == NFS_SUPER_MAGIC 0x6969`）なら
  **scratch を無効にして**従来の挙動（`build_cache_dir`）に戻し、起動ログと `scratch status` に理由を出す（NFS を build filesystem に
  しないという入力メモの原則を設定ミスから守る）。L2 の dir は逆に NFS でなくてもよい（テストはローカルの tmp を使う）。

### D2. target の生存期間（semantic GC）

- **誰が**: daemon の tick の軽い phase `scratch_gc`（決定論的、LLM 無し、ADR-0066 D2 の `prune_one_workspace` と同じ形）。tick の
  中でやるのは「lease と DB の状態を読む → 純粋関数 `plan_gc` で削除順を決める → 上から `.deleting-*` へ rename」まで。
  `remove_dir_all` は専用スレッド 1 本（同時に 1 つ。F5-fix の `cleanup_work_unit_build_caches` を一般化して置き換える）。
  daemon が止まっている間は `celerisctl scratch gc [--dry-run]` が同じ `plan_gc` を使う（D6）。
- **分類**（owner ごと。DB は `tasks.status` と `work_units.status`、外部 owner は lease の mtime）:

| 分類 | 条件 | 扱い |
|---|---|---|
| **P0 pinned** | Task が `running` / `reviewing`（reviewer の checks が Task の target を使う）。WU が `pending` / `ready` / `running` / `needs_continuation` で、その Task が終端でない。release / agent の lease の mtime が `external_lease_ttl_secs`（既定 21600 = 6h）以内 | **絶対に消さない**（high watermark を超えても） |
| **P1 waiting** | Task が `blocked` / `ready`（infra の requeue、人の返答待ち）/ `draft`。WU が `blocked` / `failed` で Task が非終端（replan で同じ行がやり直しうる。F5-fix の規則） | 最後の更新から `waiting_keep_secs`（既定 172800 = 48h）は保持。以後は P3 と同じ |
| **P2 retry** | Task が `failed` で自動再試行の予定がある（`attempts < max_attempts` で requeue 待ち）、または人の `retry` がありうる直近の `failed` | `failed_keep_secs`（既定 86400 = 24h）保持 |
| **P3 completed** | Task が `done` / `cancelled`、`failed` の保持期限切れ。WU が `done` / `superseded` / `cancelled`。外部 lease の TTL 切れ・`released_at` あり。lease の無いディレクトリ（mtime が 1h より古いもの。作りかけと競合しないため）。DB に行が無い task（別の DB、消えた Task） | 回収可能（reclaimable） |
| **seed** | P3 のうち、同じ `repo_key` の最新の 1 つ（`warm_seeds_per_repo = 1`）で、その repo を使う非終端の Task があるもの | P3 より後に消す。次の run の adopt（D3）の種 |

- **「同 repo で近い commit」のヒューリスティック**: 入力メモの「task A = abc123、task B = abc128 の target は保持」は、保持しただけでは
  得をしない（cargo は別ディレクトリの target を使わない）。本 ADR では**seed + adopt**で実現する: 回収可能になった target のうち
  repo ごとに最新の 1 つを seed として残し、同じ repo の次の owner が割り当て時にそれを rename で引き継ぐ（D3）。commit の距離
  （`git rev-list --count <merge-base>..<base>`）は tick では計算しない（git を tick に入れない）。lease の `base_commit` を持っておき、
  adopt の候補選びのとき（run の開始時、既に git を使う経路）にだけ計算する。
- **watermark**（pool の使用量 = `targets/` の推定サイズの合計）:
  - `targets` が `targets_max_gb × high_watermark`（既定 0.90）を超えた → `targets_max_gb × low_watermark`（既定 0.70）まで下げる。
  - filesystem の空きが `min_free_disk_mb × 2` を下回った（DB を守る早めの合図）→ 同じ手順を、空きが `min_free_disk_mb × 3` に戻る
    見込みまで。
  - どちらも超えていなくても、P3 のうち WU の target（`wu-*`）と `release-*`（seed を除く）は即回収（F5-fix の「WU 終端で即削除」を保つ）。
    Task の target（P3）は `completed_grace_secs`（既定 600）経ってから回収（取り込み直後の再検査・rereview に使える短い猶予）。
- **削除順**（`plan_gc` の出力。上から順に、目標に届くまで）: ① legacy（D7 の旧 `build_cache_dir/cargo/*`、1h 以上更新が無いもの）と
  lease の無い野良 → ② P3（LRU: lease の mtime が古い順）→ ③ seed（古い順）→ ④ P2（保持期限の近い順）→ ⑤ P1（最後の更新が古い順）。
  **P0 は候補にしない**。同順位は owner の文字列順（決定論）。1 tick に rename するのは最大 `gc_max_per_tick`（既定 8）件。
- **サイズの測り方**: tick の中で `du` はしない（ADR-0066 D2 の方針）。専用の測定スレッドが owner を 1 つずつ（既定 30 秒に 1 つ）
  `st_blocks` で数え、`lease.json` の `size_bytes` / `measured_at` に書く。`plan_gc` はこの値（古くてもよい）と statvfs を使う。
  測定前の owner は「0 とみなさず、同じ repo の最大値」を仮に使う（過小評価で watermark を見逃さない）。`mp1` の専用ボリュームなら
  statvfs で足りるので測定は表示用になる。
- **外部の owner（release.sh・実装エージェント）**: `celerisctl scratch lease --owner <owner> --repo <path> [--base <sha>]` が
  lease.json を作る（既にあれば touch するだけ）と同時に `CARGO_TARGET_DIR` のパスを標準出力に出す。長いビルドの間は同じコマンドを
  繰り返すか `celerisctl scratch touch --owner <owner>`（cargo を呼ぶ前に毎回）。終わったら `celerisctl scratch release --owner <owner>`
  （`released_at` を書き、P3 に落とす）。TTL 切れは P3。daemon の GC と CLI の lease 操作は `.lock` の flock で直列化する。
- **journal**: watermark の到達・解除、GC の実行（rename した owner・推定サイズ・分類・理由）、実効上限の縮小は `tracing::info!/warn!`
  （systemd journal に出る）。DB の event は増やさない（満杯の瞬間に DB へ書かない。D1）。空きが `min_free_disk_mb` を割ったときの
  通知は既存の「ディスク不足 (infra)」をそのまま使う（D3）。

### D3. 割り当て

- **全ての cargo 実行経路**に、run の開始時に `CARGO_TARGET_DIR=<scratch>/targets/<owner>/target` を与える:

| 経路 | owner | 与える場所（G1 で触る） |
|---|---|---|
| Task 単位の worker run（暗黙 WU・v1・並列 1 に倒した v2） | `task-<id>` | `dispatcher.rs` の run 起動（今の `cargo_target_dir_env` の箇所）→ `adapter.with_env` |
| 自分の worktree で走る v2 の WU の run と、その WU の checks | `task-<id>/wu-<wu_id>` | 同上 + `LocalWorkspace::with_env`（F5-fix の `check_cargo_target_env`） |
| 統合 WU の検査・統合の repair WU（Task の worktree） | `task-<id>` | 同上 |
| reviewer の checks | `task-<id>` | `review.rs` の検査（F5-fix で `with_env` 済みの経路） |
| release ゲート | `release-<sha12>` | `scripts/selfdeploy/release.sh`（`celerisctl scratch lease`、D7） |
| 実装エージェント | `agent-<name>` | エージェントの手順（定型文、D7） |

- 対象は ADR-0066 D1 と同じく**ローカルの git worktree のホスト実行**。コンテナ（`[container] env` で人が決める）と Remote（クラスタ側の
  ディスク）は対象外のまま。worker の env（`claude_code.rs` / `codex.rs` の `.envs(config.env…)`）には dispatcher が `with_env` で
  足した値がそのまま入る（アダプタ側の変更は不要。`with_env` を持たないアダプタは今どおり `debug` ログを出して素通り）。
  `RunRequest.cargo_target_dir`（F5-fix の監査用の写し）には scratch のパスが入る。
- **adopt（引き継ぎ）**: owner の `target/` がまだ無いとき、同じ `repo_key` の P3 / seed の target があれば rename で引き継ぐ
  （コピーはしない。O(1)）。**安全条件**: 候補の target の最終書き込み（`lease.json` の mtime と `target/` 直下の mtime の新しい方）が、
  引き継ぐ側の worktree の checkout 時刻より**前**であること。checkout で書かれたソースは全て mtime がそれより新しいので、cargo は
  workspace のメンバー（path crate）を全て作り直し、registry の依存だけを再利用する（§1 の mtime の誤認が起きない）。条件を満たす候補の
  うち commit の距離が最小のもの（`adopt_max_distance`、既定 200 commits 以内）、無ければ最も新しいもの。候補が無ければ空から。
  `adopt = false` で無効化。adopt した事実は run のログと lease（`adopted_from`）に残す。
- **空きの確認**: 既存の `check_disk_space`（`/`・`workspace_root`・`build_cache_dir` の statvfs）の対象に `[scratch] dir` を足す。
  空きが `min_free_disk_mb` 未満なら、新しい run を始める前に **その tick で `scratch_gc` を緊急モード（目標 = 空き `min_free_disk_mb × 3`）
  で回し**、rename だけ済ませる。削除が終わって空きが戻るまで（数 tick）は dispatch を保留し、既存の「ディスク不足 (infra)」通知を
  1 回だけ出す（Phase 116 / F5-1 の infra 分類。attempts を消費しない）。P0 だけで上限を超えている（消せるものが無い）ときも同じ保留で、
  通知の本文に「scratch pool: pinned N GB（running の owner の一覧）」を足す。
- **WU の終端**: WU が `done` / `cancelled` / `superseded` になった時点で P3 になり、次の `scratch_gc` で即回収（F5-fix と同じ挙動。
  ただし seed の条件を満たせば 1 つだけ残る）。`failed` / `blocked` は P1（F5-fix と同じく replan のやり直しに備える）。
- **Task の終端**: `done` / `cancelled` は `completed_grace_secs` 後に回収。ADR-0066 D2 の「作業場所の `repos/*/target` を刈る」は、
  scratch 導入後は通常は空振りになるが、scratch を無効にした環境と野良の target のために残す。

### D4. sccache L1

- **配線**（G2。scratch と同じ全経路。Celeris の run の env に dispatcher が足す）:
  `RUSTC_WRAPPER=<tools>/sccache/bin/sccache`、`SCCACHE_DIR=<scratch>/sccache-l1`、`SCCACHE_CACHE_SIZE=<l1_max_gb>G`、
  `SCCACHE_SERVER_PORT=<[scratch.sccache] server_port>`（既定 4236。人が自分で使う sccache の既定 4226 と分ける）、
  `SCCACHE_IDLE_TIMEOUT=0`。
- **sccache の server は 1 つにする**: sccache は 1 つの port に 1 つの server が立ち、**最初に server を起こした client の env
  （`SCCACHE_DIR` 等）で動き続ける**（一般知識。細部は**未確認**）。env が経路ごとにずれると、後から来た client の設定は黙って無視される。
  したがって (1) env は Celeris が一か所で組む（`task_worker::scratch::sccache_env`）、外部の経路は `celerisctl scratch env
  [--owner <owner>]`（`export …` を出力）で**同じ値**を得る、(2) server は run の中から起こさない: run が起こすと `celeris@<sha12>` の
  cgroup に入り、昇格（停止→起動）で巻き添えに殺される（ADR-0060 と同じ教訓）。専用の systemd user unit `celeris-sccache.service`
  （`sccache --start-server` を前景で動かす。前景起動の方法は `SCCACHE_NO_DAEMON=1` と理解しているが**未確認**）を
  `scripts/selfdeploy/install-units.sh` に足し、人が有効化する（本 ADR の作業では `systemctl` に触れない）。unit が無いときは
  dispatcher は `RUSTC_WRAPPER` を与えない（素の cargo。scratch の効果は残る）。
- **導入**: `tools/` の流儀（`tools/langmem/requirements.txt` + `scripts/knowledge/setup-langmem.sh`）に合わせ、`tools/sccache/VERSION`
  （版と配布バイナリの sha256）+ `scripts/scratch/setup-sccache.sh`（`$CELERIS_STATE_DIR/tools/sccache/bin/sccache` に置く。
  配布バイナリ〈GitHub release の musl 版〉を取って sha256 を照合、無理なら `cargo install sccache --locked --version <VERSION>
  --root <tools>/sccache`）。**`cargo test` の一部ではない**（ネットワークに出る。人が 1 回だけ手で叩く。ADR-0009 P-34）。
  `[scratch.sccache] binary` の既定はこのパス。無ければ `enabled` でも配線しない（起動ログに理由）。
- **incremental との相性**: sccache は incremental compilation（`-C incremental`）の rustc 呼び出しをキャッシュしない（一般知識）。
  cargo が incremental にするのは workspace のメンバー（path crate）だけで、registry の依存は常に非 incremental なので、
  **依存 crate は `CARGO_INCREMENTAL` に関係なく sccache に載る**。`CARGO_INCREMENTAL=0` が変えるのは workspace メンバーの扱い:
  (+) メンバーも sccache に載りうる、(+) `target/*/incremental/` が作られない（target の容量の大きな割合を占める。**ディスクの主な
  得はこちら**）、(−) 同じ run の中の「編集 → 再ビルド」が遅くなる（例: `task-dispatch` の `dispatcher.rs` は 3 万行で、1 行の変更でも
  crate 全体を作り直す）。Celeris の run は新しい worktree での短命なビルドが多いので、**Celeris の run では `CARGO_INCREMENTAL=0` を
  既定**にする（`[scratch.cargo] incremental = false`）。実装エージェントの定型文では既定を同じにしつつ、長い編集ループのエージェントは
  外してよいと書く。G2 で壁時計を測り、悪化が大きければ既定を見直す（§4 U3）。
- **debug info**: `CARGO_PROFILE_DEV_DEBUG=line-tables-only`（cargo の環境変数による profile の上書き。値 `line-tables-only` は
  Rust 1.71 以降と理解しているが**未確認**）を Celeris の run の既定にする（`[scratch.cargo] dev_debug = "line-tables-only"`）。
  target の容量が大きく減り、panic の backtrace の file:line は残る。デバッガで変数を見る用途は Celeris の run には無い。`test`
  profile は `dev` を継ぐので同じ効果（**未確認**、G2 で `cargo test` の生成物の容量で確かめる）。release profile は触らない。
  env の差は sccache の key に入る（flags が違うと別 entry）ので、ここでも「全経路で同じ env」が効く。
- **path への依存（最大の未確認事項）**: sccache の Rust の key には rustc の引数・ソース・一部の env が入る。worktree ごとに
  ソースの絶対パスが違う workspace メンバーと、`CARGO_TARGET_DIR` ごとに違う `--out-dir` / `-L dependency=…` が key に入るかどうかで
  hit 率が大きく変わる（入ると owner をまたいだ hit は registry の依存だけになる）。**未確認**。G2 の最初の受け入れ条件として実測する
  （同じ commit を別の owner の target で 2 回ビルドし `sccache --show-stats` の hit 率を見る）。依存 crate の hit が出ないなら G2 を
  止めて人に聞く（§4 U1）。

### D5. L2 の方式の比較と決定

候補:

- **(a) flusher 方式**: sccache の local disk cache（`SCCACHE_DIR = sccache-l1/`）をそのまま L1 にし、Celeris の flusher が L1 の新しい
  entry を NFS の `<key>.zst` へ write-back（tmp → write → fsync → rename、帯域 20〜30 MB/s）。task start 時に「同 repo / 近い commit」
  の entry を L2 → L1 へ promote（事前コピー）する。
- **(b) webdav 方式**: sccache の webdav backend（`SCCACHE_WEBDAV_ENDPOINT=http://127.0.0.1:<port>/`）に対して、Celeris が loopback で
  小さな cache server を立てる。GET: L1 → L2 → 404（miss）、L2 hit は L1 へ promote して返す。PUT: L1 に書いて即 201、L2 へは
  非同期の flusher（帯域制限）が書く。**真の階層キャッシュ**（入力メモの挙動そのもの）。

| 観点 | (a) flusher | (b) webdav cache server |
|---|---|---|
| miss のとき L2 を見る | **見ない**。L1 の miss はそのまま rustc。L2 を使えるのは事前の promote で当たった分だけ | 見る（入力メモの compile request → L1 → L2 → compile の順そのもの） |
| promote の対象選び | sccache の key は入力の hash で、repo や commit との対応を持たない。「同 repo / 近い commit の entry」を選ぶには key → repo の索引を別に作る必要があり、その索引の作り方（どのビルドがどの key を作ったか）を sccache は外に出さない | 要らない（要求された key だけを取りに行く） |
| 既存のエントリ形式との整合 | sccache の disk cache の内部構造（ディレクトリの切り方、サイズ管理）に外から書き足す。disk cache のサイズ計算は server の起動時の走査と自身の書き込みで管理されていると理解しており（**未確認**）、外からの promote は LRU とサイズ上限の勘定を狂わせうる。版上げで内部構造が変わると壊れる | sccache が外に出している backend の契約（HTTP の GET / PUT、entry は不透明なバイト列）だけに依存。entry の中身は解釈しない |
| NFS 停止時の可用性 | L1 だけで動く（flusher が止まるだけ） | cache server の L2 の I/O を専用の有界スレッドプールとタイムアウト（GET は `l2_get_timeout_ms` 既定 500）に閉じ込め、連続失敗で L2 を切り離す（ADR-0066 D3 と同じ指数バックオフ）。**L1 だけで動く**。NFS の hard mount で I/O スレッドが D state になっても L1 の経路は止まらない |
| cache server 自身の停止 | 該当なし | sccache の backend が応答しないときの挙動は「キャッシュ無しでコンパイルを続ける」と理解しているが**未確認**。G3 の受け入れ条件で確かめる。加えて dispatcher は run の開始時に cache server の `/healthz` を見て、応答が無ければ `RUSTC_WRAPPER` を与えない（素の cargo）。server は `celeris@` の外の専用 unit（D4 と同じ理由） |
| 複数 worker の同時書き込み | sccache の server は 1 つなので L1 は sccache が直列化。L2 は tmp → rename で安全 | L1 は cache server 1 プロセスが直列化（tmp → rename）。L2 も tmp（`<key>.zst.tmp-<pid>-<ulid>`）→ fsync → rename。同じ key の同時書き込みは後勝ちで、どちらの中身も正しい（sccache の key は入力の hash なので、どちらも同じ入力に対する有効な出力） |
| 実装量 | 小〜中（ディレクトリの監視・diff、promote の索引が難所） | 中（axum の小さな HTTP server、WebDAV の必要最小限のメソッド、L1 の LRU、flusher、L2 の GC）。axum は workspace で既に使っている |
| テスト容易性 | sccache の内部形式に依存するので、本物の sccache を使うテストが要る | cache の核（`TieredStore` の trait、L1 / L2 の実装、flusher の token bucket）は純粋な Rust で、sccache 無しにテストできる。HTTP は loopback の port 0 でテスト（外部ネットワークに出ない）。本物の sccache を使う e2e は `#[ignore]` の手動確認に分ける |

**決定: (b)**。理由: (1) 入力メモの挙動（miss のとき L2 を見て L1 へ promote、L2 への書き込みは critical path の外）を満たすのは (b) だけで、
(a) は「L1 の miss = compile」になり L2 は事前 promote の当たりでしか使えない。(2) (a) の promote は key と repo の対応を必要とし、sccache
がそれを外に出さないので「同 repo / 近い commit」の選び方が作れない。(3) (b) は sccache の**公開された backend の契約**だけに依存し、
sccache の版上げに強い。(4) 可用性は (b) でも、L2 の I/O の隔離と「server が無ければ `RUSTC_WRAPPER` を外す」で (a) と同等にできる。
(5) 核が純粋な Rust でテストしやすい。

(b) の詳細:

- **置き場と形**: 新しい crate `crates/scratch-cache`（lib）と、`celeris cache-server` サブコマンド（`celeris` バイナリに同梱。常駐は
  専用の systemd user unit `celeris-cache.service`。`~/.local/celeris/current/bin/celeris` を指し、昇格で自動的には再起動しない。
  protocol を変えた版だけ人が再起動する）。`127.0.0.1:<[scratch.cache_server] port>`（既定 4237）だけに bind し、認証は Basic
  （`SCCACHE_WEBDAV_USERNAME` / `SCCACHE_WEBDAV_PASSWORD`。token 系の変数名は**未確認**なので Basic を第一候補）。秘密は
  `~/.config/celeris/cache-server.token`。
- **WebDAV の範囲**: sccache（内部では opendal の webdav）が実際に発行するメソッドは**未確認**（GET / PUT / HEAD に加え、ディレクトリ
  作成の MKCOL や PROPFIND がありうる）。G3 の最初の区切りで、ログを取るだけの stub に本物の sccache を向けて発行されるメソッドとパスを
  記録し（loopback のみ）、その集合だけを実装する。path の最後の要素を key とし、`[0-9A-Za-z_-]` 以外を含む key は 400。
- **L1**: `sccache-l1/<k0k1>/<key>`（sccache が PUT したバイト列そのまま）。書き込みは tmp → rename。L1 のサイズと LRU は
  cache server がメモリに持つ（起動時に 1 回走査し、以後は自分の書き込みと hit〈mtime を touch〉で更新）。`l1_max_gb × 0.9` を超えたら
  `× 0.7` まで LRU で落とす。**L2 に未 flush の entry は落とさない**（先に flush するか、flush 待ちの間は残す）。G2 の sccache の disk
  cache の中身は形式が違うので G3 の初回起動で消して作り直す（キャッシュなので失っても再生成できる）。
- **L2**: `sccache-l2/<k0k1>/<key>.zst`。中身は PUT されたバイト列を zstd（level 3、**content checksum 付き**）で包んだもの。
  sccache の entry は既に圧縮済みと理解しており（**未確認**）容量の得は小さいが、checksum で NFS 上の切れたファイルを検出し、読めなければ
  miss 扱いにして消す（壊れた entry を L1 へ promote しない）ために包む。immutable（同じ key を上書きしない。存在すれば書かない）。
- **flusher**: 専用スレッド 1 本。PUT の順に key を積む（上限 `flush_queue_max_mb`、既定 4096 MB。超えたら古いものから「L2 に
  書かない」で落とし、落とした数を metrics に出す）。token bucket で `flush_mb_per_sec`（既定 25）に制限。L2 に既にあれば書かない
  （stat 1 回）。書き込みは tmp → write → fsync → close → rename。未 flush の key は `sccache-l1/.pending`（ローカルの追記ログ）にも
  書き、再起動後に積み直す。
- **L2 の GC**: cache server が 1 日 1 回、`l2_max_gb`（既定 300）を超えていれば mtime の古い順に消す。L2 の hit で mtime を touch するのは
  1 entry につき 1 日 1 回まで（NFS への書き込みを増やさない）。immutable なので、消している最中の別プロセスの読み込みは開いた fd で完結する。
- **task start 時の promote**（入力メモの (a) にある事前コピー）は (b) では**しない**（要求駆動で足りる）。hit 率が L2 の遅延で頭打ちになる
  ことが G3 の測定で分かったら、別 Phase で「同 repo の直近の run が要求した key の一覧」を先読みする案を検討する（§4 U4）。

### D6. 監視

- **`celerisctl scratch status [--json]`**: pool の dir・filesystem の容量 / 空き・実効上限・watermark の状態、`targets` の合計（推定）、
  owner ごとの行（owner、分類 P0〜P3 / seed、推定サイズと測定時刻、lease の mtime、repo_key、base_commit の先頭 12 桁、adopt 元）、
  L1 / L2（G2 は `sccache --show-stats` の hit / miss とサイズ、G3 は cache server の `/stats`: L1 hit・L2 hit・miss、L1 / L2 のサイズ、
  flush の待ち行列の MB と最古の待ち時間〈flusher の遅延〉、L2 の切り離し状態と最後のエラー）、直近の GC（時刻、rename した owner、回収量）。
  daemon が止まっていても lease と statvfs と DB（読み取り専用）から出せる部分は出す。
- **`GET /api/v1/metrics/scratch`**（依頼文の `/metrics/scratch` を既存の API の接頭辞に合わせた）: 上と同じ JSON
  （`celeris.scratch-status/1`）。dispatcher が tick ごとに組んだ `ScratchView` を `DaemonSnapshot.scratch`（`#[serde(default)]`、古い
  スナップショットには無い）に載せ、API はそれを返す。L1 / L2 は G3 まで `null`。`docs/api/v1` の schema と GUI の生成型に影響する。
- **GUI のデーモン画面**（`gui/app/routes/daemon.tsx`）に 1 行: 「scratch 62 / 100 GB（pinned 18 GB、実効上限 150 GB）· L1 hit 71% ·
  L2 hit 12% · flush 遅延 3 s」。watermark 超過・L2 切り離し・実効上限の縮小のときだけ注意色。
- **journal**: D2 のとおり（watermark の到達・解除、GC の実行、実効上限の縮小、L2 の切り離し・復帰、flusher の待ち行列の溢れ）。

### D7. 互換と移行

- **`[workspace] build_cache_dir` / `shared_build_cache`**: `[scratch]` が有効（既定 `enabled = true`）なら dispatcher は
  `build_cache_dir/cargo/<repo-key>[/wu-<id>]` を**使わない**。`[scratch] dir` の既定は `build_cache_dir` の**親の `scratch/`**
  （本番は `build_cache_dir = /var/lib/celeris/build-cache` なので `/var/lib/celeris/scratch`。既定の `~/.local/celeris/build-cache` の
  ままの環境では `~/.local/celeris/scratch` になり、それが NFS なら D1 の検査で scratch が無効になり従来どおり動く。本番の config では
  `[scratch] dir` を明示することを推奨）。`shared_build_cache = false` は「scratch も含めて `CARGO_TARGET_DIR` を与えない」の意味で残す。
  `[scratch] enabled = false` で ADR-0066 D1 / F5-fix の挙動に戻せる（1 リリースの間の退路。G3 の後に撤去を提案）。
- **既存の `build-cache/cargo/*`**: 初回の `scratch_gc` で legacy として削除順の先頭（1h 以上更新の無いものだけ。今動いている実装エージェントが
  手で指している `agent-platform-<name>` を途中で消さない）。`celerisctl build-cache prune` は残し、help に「scratch へ移行済み。
  `celerisctl scratch gc` を使う」と書く（撤去は G3 の後）。
- **`~/.cargo/config.toml` に target-dir を置かない規約**: 2026-09-28 の人の変更（`[build] target-dir` の廃止）を規約にする。
  `CARGO_TARGET_DIR` は常に経路ごとに env で与える。`docs/ops/home-nfs-migration-2026-09-25.md` §5 の最終項の記述はこの規約と
  逆なので、G1 で訂正する（ops 手順書の追記・訂正は docs 配下の変更）。`/tmp` にも target を置かない（§1 の 32G の野良）。
- **`scripts/selfdeploy`**: `lib.sh` の `SD_CARGO_TARGET` を `$(celerisctl scratch lease --owner release-<sha12> --repo "$SD_REPO"
  --base <sha>)` の結果にする（`release.sh` が sha12 を決めた後に組む）。`celerisctl` が無い・scratch が無効なときは従来の
  `$SD_RELEASES/.cargo-target` に戻る。ゲートの各 step の前に `celerisctl scratch touch`、終了時（成功・失敗とも `trap`）に
  `celerisctl scratch release`。release の target は adopt で直前の release の target を引き継げるので、今の「revision をまたいで共有し
  ロックで直列化」（commit 3d66bb5）の利点は残る（release.sh の直列化のロックはそのまま）。既存の
  `/var/lib/celeris/release-build/.cargo-target` は legacy として初回 GC の対象に加える（symlink の先。`.build` の worktree は対象外）。
- **実装エージェントのプロンプトの定型文**（Fable が委譲文に貼る。G1 で `.claude/agents/implementer.md` / `auditor.md` にも入れる）:

  > cargo を使う前に `eval "$(celerisctl scratch env --owner agent-<worktree 名> --repo <worktree の絶対パス>)"` を 1 回実行する
  > （`CARGO_TARGET_DIR` などが設定され、scratch の lease が作られる）。長いビルドの前には `celerisctl scratch touch --owner agent-<worktree 名>`。
  > `CARGO_TARGET_DIR` を自分で決めない。`~/.cargo/config.toml` と `/tmp` に target を置かない。作業が終わったら
  > `celerisctl scratch release --owner agent-<worktree 名>`。

  `celerisctl` が PATH に無い環境では `~/.local/celeris/current/bin/celerisctl` を使う（`--config` は `CELERIS_CONFIG`）。
- **設定の新しい節は、それを知る release の昇格後にだけ本番 config に足す**（2026-09-28 G1 の本番反映で `[scratch]` を先に足し、
  旧い版〈N-1〉が `unknown field scratch` で起動できず verify の N-1 が壊れた）。`[scratch]` 以下の節（G2 の `[scratch.sccache]` /
  `[scratch.cargo]`、G3 の `[scratch.l2]` / `[scratch.cache_server]`）はどれも**既定値だけで動く**ように作り、変えたいときだけ、
  昇格して N-1 がその節を知る版になった後に足す。rollback 先の版が知らない節を config に残すと rollback も壊れる（提案 P-G1-1:
  release.sh のゲートに「現行 config を N-1 の版でも parse できるか」を足す）。

## 3. 採らない案

- **target の NFS への write-back**（ビルド中・終了後とも）: Cargo / rustc が書き換え続けるファイルと競合し整合した snapshot にならない。
  target は大量の小ファイルで、結局それを 1GbE の NFS へ流すことになる（入力メモ）。再利用価値は sccache に集約する。
- **NFS 上の target**（`build_cache_dir` を NFS に置く、`~/.local/celeris/build-cache` の既定のまま使う）: metadata の往復が遅く、NFS 移行後
  release ゲートでタイミング依存テストが落ちた実績がある（phase-F.md の release 90d418e2036a の節）。D1 の起動時検査で拒否する。
- **rsync 方式の L1 → L2 同期**（`sccache-l1/` を定期的に rsync）: NFS に数十万の小ファイルの tree を作り、差分検出のたびに metadata を
  全走査する。immutable な `<key>.zst` 1 ファイルの方式（D5）の方が、同時書き込み・部分書き込み・GC のどれも単純。
- **D5 (a) flusher 方式**: D5 の比較のとおり（miss のとき L2 を見ない、promote の対象を選べない、sccache の内部形式に依存）。
- **target のコピーによる複製**（F5-fix の U-F2 にあった「WU ごとの target をコピーから始める」）: 数 GB〜十数 GB のコピーは I/O が重く、
  mtime の誤認（§1）の危険も持ち込む。adopt（rename、安全条件つき）と sccache で代える。
- **単純な LRU だけの GC**: RUNNING の target を消しうる。Celeris は状態を知っているので semantic GC（D2）にする（入力メモ）。
- **sccache の server を run の中から起こす**: `celeris@` の cgroup で昇格の巻き添えになる（ADR-0060 の教訓）。
- **`du` を tick の中で回す**: ADR-0066 D2 と同じ理由。測定は専用スレッドで 1 owner ずつ。
- **cache server を daemon（`celeris@`）に同居させる**: 昇格のたびに止まり、実装エージェントと release のビルドも巻き込む。

## 4. 未解決（G1〜G3 で測って決める）

- **U1**: sccache の Rust の key が絶対パス（worktree のソース、`--out-dir`、`-L dependency=`）を含むか。含むなら owner をまたいだ hit は
  registry の依存に限られる。G2 の最初の測定で決める（依存の hit が出なければ止めて人に聞く）。対策の候補（`--remap-path-prefix`、
  sccache の basedir 系の設定）が効くかも**未確認**。
- **U2**: 実効上限（D1）。§1 の「見えない 100G」が実在するか（人が root で確認）。`mp1` を足すか（人の判断）。
- **U3**: `CARGO_INCREMENTAL=0` の壁時計の悪化（D4）。G2 で F5-1 相当の run の時間を比べる。
- **U4**: L2 の事前 promote（D5 の最後）。G3 の hit 率と L2 の遅延の測定で要否を決める。
- **U5**: sccache の webdav backend の実際の挙動（発行するメソッド、backend 停止時にコンパイルを続けるか、起動時の storage check）。
  G3 の最初の区切りで loopback の stub に向けて記録する。
- **U6**: `clippy-driver`（`RUSTC_WORKSPACE_WRAPPER`）と `RUSTC_WRAPPER=sccache` の組み合わせでの caching の可否（**未確認**）。
  clippy は workspace メンバーにだけ効くので、依存の hit には影響しない見込み。
- **U7**: F5-fix（branch `worktree-agent-aea5cf7690b5b2bc5`、commit 879d01c / 701ee5f / e7a0f23）は G0 時点で main に未 merge。G1 は
  F5-fix の `with_env` / `check_cargo_target_env` / `cleanup_work_unit_build_caches` / `RunRequest.cargo_target_dir` の上に作るので、
  **F5-fix の merge が G1 の前提**。

## 5. 実施計画（Phase G1〜G3）

各 Phase の完了時に `cargo fmt --all -- --check` / `cargo test --workspace --no-fail-fast` / `cargo clippy --workspace --all-targets -- -D warnings`、
GUI に触れた分は `pnpm -C gui typecheck && lint && test && gen:types`（差分ゼロ）、スキーマに触れた分は `UPDATE_SCHEMA=1` の再生成と差分の
commit。テストは外部ネットワークに出ない（HTTP は loopback の port 0）。

### Phase G1: scratch pool + 割り当て + semantic GC + watermark + celerisctl / metrics（sccache 無しでも価値がある）

前提: F5-fix が main に merge 済み（U7）。

- **触るファイル**: `crates/task-worker/src/scratch.rs`（新規。owner とパス、`lease.json` の読み書き、`plan_gc`〈純粋関数〉、adopt の候補選び
  〈純粋関数 + git の距離は呼び出し側〉、`.lock`）、`crates/task-worker/src/build_cache.rs`（legacy の扱い）、`crates/task-worker/src/preamble.rs`
  （1 行の文言を scratch に）、`crates/celeris/src/config.rs`（`[scratch]`、NFS 検査、既定の dir）、`crates/task-dispatch/src/dispatcher.rs`
  （run 起動時の割り当てと adopt、`scratch_gc` の tick phase と削除スレッド・測定スレッド、`check_disk_space` の対象と緊急 GC、F5-fix の
  `cleanup_work_unit_build_caches` の置き換え）、`crates/task-dispatch/src/review.rs`（checks の env を scratch に）、
  `crates/celerisctl/src/commands/scratch.rs`（新規: `status` / `gc [--dry-run]` / `lease` / `touch` / `release` / `env`）と
  `commands/build_cache.rs`（help の案内）、`crates/task-ops/src/daemon.rs`（`DaemonSnapshot.scratch`）、`crates/task-api/src/handlers.rs`
  （`GET /api/v1/metrics/scratch`）、`docs/api/v1/*.schema.json`、`gui/app/routes/daemon.tsx` と生成型、`scripts/selfdeploy/{lib.sh,release.sh}` と
  `scripts/selfdeploy/tests/`、`.claude/agents/{implementer,auditor}.md`（定型文）、`docs/ops/home-nfs-migration-2026-09-25.md`（D7 の訂正）。
- **受け入れ条件**:
  1. 全ての経路（Task 単位の run、v2 の WU の run と checks、統合 WU の検査、reviewer の checks）の `CARGO_TARGET_DIR` が
     `<scratch>/targets/<owner>/target` で、`request.json` の `cargo_target_dir` と一致。コンテナ・Remote には与えない。
  2. `plan_gc` が P0 を決して選ばず、削除順 ① legacy / 野良 → ② P3（LRU）→ ③ seed → ④ P2 → ⑤ P1 を守り、low watermark で止まる。
  3. WU の終端で WU の target が次の tick で rename され、別スレッドで消える（seed を除く）。`failed` / `blocked` の WU の target は残る。
  4. adopt は安全条件（候補の最終書き込み < checkout 時刻）を満たすときだけ起き、満たさなければ空から始める。
  5. 空きが `min_free_disk_mb` 未満 → 緊急 GC → 空きが戻るまで dispatch 保留と「ディスク不足 (infra)」通知 1 回、戻れば自動解除。
  6. 外部 lease（release / agent）の TTL 切れ・`release` 後に P3 になる。`celerisctl scratch env` の出力と dispatcher が組む env が同じ。
  7. `scratch dir` が NFS なら scratch 無効で従来の挙動（起動ログに理由）。`[scratch] enabled = false` で F5-fix の挙動に戻る。
  8. `celerisctl scratch status --json` と `GET /api/v1/metrics/scratch` が同じ schema。GUI のデーモン画面に 1 行。schema / 生成型の差分は
     commit 済み、再生成で差分ゼロ。
  9. `release.sh` が scratch の lease を取り、終了時に release する（`scripts/selfdeploy/tests` の偽 `celerisctl` で確認。本番のパスには触れない）。
- **テスト名（案）**: `task_worker::scratch::tests::{owner_paths_nest_work_units_under_their_task, plan_gc_never_selects_pinned_owners,
  plan_gc_follows_the_semantic_order_and_stops_at_low_watermark, plan_gc_keeps_one_warm_seed_per_repo, plan_gc_treats_unmeasured_owners_as_the_repo_maximum,
  external_lease_expires_after_ttl, adopt_requires_the_target_to_predate_the_checkout, adopt_prefers_the_nearest_commit_within_the_limit,
  legacy_build_cache_entries_are_reclaimed_only_when_idle}`、`task_dispatch::dispatcher::tests::{every_cargo_path_uses_the_scratch_target_dir,
  terminal_work_unit_target_is_reclaimed_on_the_next_tick, low_disk_runs_emergency_gc_before_pausing_dispatch, scratch_on_nfs_falls_back_to_build_cache_dir}`、
  `celeris::config::tests::{scratch_defaults_follow_the_build_cache_parent, scratch_can_be_disabled}`、
  `celerisctl::commands::scratch::tests::{env_matches_the_dispatcher_env, lease_touch_release_round_trip, gc_dry_run_lists_without_removing}`、
  `task_api::handlers::tests::metrics_scratch_matches_the_status_schema`、`scripts/selfdeploy/tests/release_uses_scratch_lease.sh`。
- **並行可否**: dispatcher.rs を大きく触るので、dispatcher.rs を触る他の Phase とは並行しない。内部では `scratch.rs`（純粋部）・
  `celerisctl scratch`・selfdeploy・GUI の 1 行は implementer を並列にできる（dispatcher の配線は `scratch.rs` の API が固まった後）。

### Phase G2: sccache L1 の導入と Celeris の run への配線 + `CARGO_INCREMENTAL=0`

前提: G1 完了、人が `scripts/scratch/setup-sccache.sh` を 1 回実行し `celeris-sccache.service` を有効化（手順は G2 で docs に書く）。

- **触るファイル**: `tools/sccache/VERSION`（新規）、`scripts/scratch/setup-sccache.sh`（新規）、`scripts/selfdeploy/install-units.sh`
  （`celeris-sccache.service` の雛形を足すだけ。有効化は人）、`crates/task-worker/src/scratch.rs`（`sccache_env`・`cargo_env`）、
  `crates/celeris/src/config.rs`（`[scratch.sccache]`・`[scratch.cargo]`）、`crates/task-dispatch/src/dispatcher.rs`（env への追加、server の
  有無の確認）、`crates/celerisctl/src/commands/scratch.rs`（`env` に同じ値、`status` に `sccache --show-stats`）、`preamble.rs`（1 行）。
- **受け入れ条件**:
  1. U1 の実測（最初に行う）: 同じ commit を別 owner の target で 2 回ビルドし、2 回目の依存 crate が sccache で hit する（`--show-stats` の
     hit 数と壁時計を PROGRESS に残す）。依存の hit が出なければ止めて人に聞く。
  2. sccache が有効で server が応答するとき、全経路の env に `RUSTC_WRAPPER` / `SCCACHE_DIR` / `SCCACHE_CACHE_SIZE` / `SCCACHE_SERVER_PORT` /
     `CARGO_INCREMENTAL=0` / `CARGO_PROFILE_DEV_DEBUG=line-tables-only` が入り、`celerisctl scratch env` と一致。server が無い・バイナリが無い・
     `enabled = false` のときは sccache 系を与えない（scratch 系は残る）。
  3. `CARGO_INCREMENTAL=0` と debug の抑制で target の容量が下がる（本リポジトリの `cargo test --workspace` の target の容量を before / after で
     記録）。壁時計の変化も記録（U3）。
- **テスト名（案）**: `task_worker::scratch::tests::{sccache_env_is_complete_and_stable, sccache_env_is_omitted_without_binary_or_server}`、
  `task_dispatch::dispatcher::tests::{runs_get_sccache_env_when_the_server_is_up, runs_fall_back_to_plain_cargo_when_the_server_is_down}`、
  `celeris::config::tests::scratch_cargo_defaults_disable_incremental`。本物の sccache を使う測定は `#[ignore]` の
  `tests/scratch_sccache_e2e.rs`（手動、証跡を PROGRESS へ）。
- **並行可否**: G1 の後。G3 の cache server の crate（`crates/scratch-cache` の純粋部）とは並行できる。

### Phase G3: L2（D5 (b) の webdav cache server）+ flusher + 監視

- **触るファイル**: `crates/scratch-cache/`（新規 crate: `TieredStore`、L1、L2、flusher〈token bucket〉、L2 の GC、axum の WebDAV サブセット、
  `/healthz`・`/stats`）、`crates/celeris/src/main.rs`（`celeris cache-server` サブコマンド）、`crates/celeris/src/config.rs`（`[scratch.l2]`・
  `[scratch.cache_server]`）、`crates/task-worker/src/scratch.rs`（`sccache_env` を webdav に切り替え）、`crates/task-dispatch/src/dispatcher.rs`
  （`/healthz` による配線の判断、`ScratchView` に L1 / L2）、`crates/celerisctl/src/commands/scratch.rs`（`status` に `/stats`）、
  `gui/app/routes/daemon.tsx`、`docs/api/v1` の schema、`scripts/selfdeploy/install-units.sh`（`celeris-cache.service` の雛形）。
- **受け入れ条件**:
  1. U5 の記録（最初に行う）: loopback の記録用 stub に本物の sccache を向け、発行されるメソッドとパスを PROGRESS に残し、その集合だけを実装。
  2. GET は L1 → L2 → 404 の順、L2 hit は L1 へ promote。PUT は L1 に書いて即応答し、L2 への書き込みは flusher が帯域制限つきで行う
     （token bucket のテストで 25 MB/s を超えない）。L2 の書き込みは tmp → fsync → rename、同じ key の同時書き込みで壊れない。
  3. L2 が読めない（dir を消す・権限を落とす・遅い偽の L2 でタイムアウト）とき、GET は L1 だけで応答し続け、連続失敗で L2 を切り離し、
     バックオフ後に復帰する。壊れた `.zst`（checksum 不一致）は miss 扱いで消す。
  4. L1 の上限で LRU に落とし、未 flush の entry は落とさない。再起動後に `.pending` から積み直す。
  5. cache server が止まっているとき、dispatcher は `RUSTC_WRAPPER` を与えず run は素の cargo で成功する。backend 停止時の sccache 自身の挙動
     （コンパイルを続けるか）を手動で確かめ PROGRESS に残す。
  6. `scratch status` / metrics / GUI に L1 / L2 の hit 率・サイズ・flush の遅延が出る。
- **テスト名（案）**: `scratch_cache::tests::{get_prefers_l1_then_l2_then_miss, l2_hit_is_promoted_to_l1, put_returns_before_the_l2_write,
  flusher_respects_the_bandwidth_limit, l2_write_is_atomic_under_concurrent_writers, corrupt_l2_object_is_a_miss_and_is_removed,
  l2_failures_detach_and_back_off, l1_eviction_keeps_unflushed_entries, pending_log_is_replayed_after_restart, invalid_keys_are_rejected}`、
  `task_dispatch::dispatcher::tests::cache_server_down_means_no_rustc_wrapper`。本物の sccache を使う e2e は `#[ignore]`。
- **並行可否**: `crates/scratch-cache` の純粋部（store・flusher・GC）は G2 と並行できる。dispatcher / celerisctl / GUI の配線は G2 の後。

## Phase G1 実装時の逸脱・明確化（2026-09-28）

G1 の実装で本文と食い違った点・本文が決めていなかった点を記録する（黙って逸脱しない）。

1. **`lease.json` に `ttl_secs` を足した**（D1 の欄の追加）。`celerisctl scratch lease --ttl <secs>` で外部の owner ごとに TTL を
   変えられる（無ければ `[scratch] external_lease_ttl_secs`）。`deny_unknown_fields` のまま、欠けていてもよい欄として足した。
2. **測定は lease の mtime を変えない**（D2 の明確化）。測定スレッドは `size_bytes` / `measured_at` を書いた後に mtime を元に戻す
   （mtime は外部の owner の唯一の生存の合図で、測定で延命させない）。daemon の測定結果はプロセス内の cache にも持つ（legacy・野良は
   lease が無いのでここだけ）。
3. **seed の「その repo を使う非終端の Task」は pool の中で近似した**（D2）。P0〜P2 に分類された owner と同じ `repo_key` を「使われている
   repo」とみなす（まだ lease を持たない ready の Task の repo までは tick で DB から引かない）。G2 以降に不足が分かれば dispatcher の
   `task_workspaces` から足す。
4. **未測定の推定**: 同じ repo の最大値。repo に測定済みが無ければ **pool 全体の最大値**（本文は repo の最大値だけ。0 にしないための補い）。
5. **野良・lease の無い owner・legacy は「測定した木の最新の mtime」が 1 時間以上前のときだけ回収**する（ディレクトリ自身の mtime は
   cargo の書き込みで更新されないため）。未測定のものは P0 扱いで消さない。legacy は pool の外なので pool の使用量・pinned に数えない
   （空きの目標には数える）。
6. **刈った後の lease の記録は 7 日で片づける**（`LEASE_RECORD_KEEP_SECS`、本文に無い）。target を刈られた P3 の owner で、`wu-*` の
   子を持たないものだけ。
7. **rename の行き先**: owner の target は `targets/.deleting-<owner-flat>-<nanos>`（Task の下の WU も pool の根に集める）、legacy は
   同じ親の中。削除スレッドは pool の `targets/` と legacy の親の `.deleting-*` を消す。
8. **`scratch_gc` は毎 tick 回す**（間引かない。「次の tick で rename」を守る）。空きが `min_free_disk_mb` 未満なら `check_disk_space` の
   中で緊急 GC を先に回し、その tick の通常の phase は重ねない。
9. **checks は adopt しない**（D3 の明確化）。WU の checks・統合 WU の検査・reviewer の checks は run と同じ owner の lease を touch して
   env を返すだけ。adopt は run の開始時（`run_worker` の `spawn_blocking`）だけで、commit の距離もそこでだけ計算する。lease の書き込みに
   失敗しても `CARGO_TARGET_DIR` のパスは与える（worktree 直下に target を作らせない）。
10. **`celerisctl scratch lease` / `env` の adopt**: 候補は同じ repo の P3 の**外部の owner**（release / agent）だけ（DB を開かない）。
    安全条件の checkout 時刻は `--worktree <path>`（無ければ `--repo`）の `.git` が**ファイル**（git worktree）のときのその mtime。分から
    なければ adopt しない。release.sh は `git worktree add` の直後に `--repo "$SD_REPO" --worktree "$BUILD"` で呼ぶ。owner は位置引数でも
    `--owner` でもよい。scratch が無効なら `lease` / `env` は失敗し、release.sh は従来の `$SD_RELEASES/.cargo-target` に戻る。
11. **`ScratchStatus` に L1 / L2 の欄はまだ無い**（D6 は「G3 まで null」）。G2 / G3 で `Option` の欄として足す（追加だけなので互換）。
    `celerisctl scratch status --json` の `last_gc` は常に null（直近の GC は daemon のメモリにしか無い）。`GET /metrics/scratch` は
    スナップショットが無い（daemon が動いていない）・`shared_build_cache = false` のとき 404 `scratch_unavailable`。
12. **`[scratch]` の欄**: 本文の欄に加えて `measure_interval_secs`（既定 30）を設定にした。`l1_max_gb` は G2 まで使わない。`total_max_gb`
    は実効上限の表示と journal にだけ使う（G1 の GC の目標は `targets_max_gb` の watermark と空き）。
13. **対象外のまま**: `[commands] setup`（worktree を作った直後に 1 度流すコマンド）には `CARGO_TARGET_DIR` を与えていない（F5-fix と同じ）。
    コンテナ実行は Remote と同じ条件（`container_plan.is_none()`）で外しており、専用のテストは Remote（`shared_build_cache_is_not_applied_to_remote_workspaces`
    に scratch の計画を渡す）だけ。

## Phase G2 実装時の逸脱・明確化（2026-09-28）

1. **U1 の答え: sccache 0.18 は `CARGO_` で始まる env を全て Rust の key に入れる**（`src/compiler/rust.rs` の `generate_hash_key`。除外は
   `CARGO_MAKEFLAGS` / `CARGO_REGISTRIES_*` / `CARGO_BUILD_JOBS` / `CARGO_ENCODED_RUSTFLAGS` だけ）。cargo は自分の env を rustc に渡すので、
   owner ごとに違う `CARGO_TARGET_DIR` が全ての key を変え、owner をまたいだ Rust の hit は **0 / 192**。`--out-dir` / `-L` / `--extern` の
   パスは key から除かれ（extern は中身の hash）、registry の依存は cwd も同じ。`--remap-path-prefix` は env の値を変えず、
   `SCCACHE_BASEDIRS` は 0.18 では C/C++ の preprocessor 出力にしか効かない。**D4 の配線を変えた**: `RUSTC_WRAPPER` は本物の sccache
   ではなく、Celeris が生成する `<scratch>/bin/sccache`（`unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR` → `exec <本物> "$@"`。
   `task_worker::scratch::{wrapper_script, ensure_wrapper}`）。これで別 owner の target で Rust **163 / 192（84.9 %）**、別のパスに展開した
   同じ commit でも 162 / 192 が hit（phase-G.md の G2 checkpoint 1）。残る miss は workspace のメンバーと、build script の `OUT_DIR`
   （target の中の絶対パス。dep-info の env として値ごと key に入る）に依存する crate とその下流（内訳は未確認）。
2. **wrapper のファイル名は `sccache`**。cc-rs は `RUSTC_WRAPPER` の stem が `sccache` のときだけ C/C++ にも同じ wrapper を使う
   （`cc-1.4.6` の `rustc_wrapper_fallback`）。置き場は `targets/` の外の `<scratch>/bin/`（GC の対象にしない）。中身が同じなら書き直さない。
3. **`SCCACHE_IDLE_TIMEOUT=0` も env に入れる**（D4 本文の列挙どおり。G2 の受け入れ条件 2 の列挙には無いが、client が万一 server を起こした
   ときに idle で止まって別の env の server に入れ替わらないように）。env の順は固定: `CARGO_TARGET_DIR`、`CARGO_INCREMENTAL`、
   `CARGO_PROFILE_DEV_DEBUG`、`RUSTC_WRAPPER`、`SCCACHE_DIR`、`SCCACHE_CACHE_SIZE`、`SCCACHE_SERVER_PORT`、`SCCACHE_IDLE_TIMEOUT`。
4. **`[scratch.cargo]` は sccache と独立に、scratch が有効な経路に常に与える**（受け入れ条件 2 の「sccache 系を与えない（scratch 系は残る）」の
   「scratch 系」に含めた。容量の得〈受け入れ条件 3〉は sccache の有無に依らないため）。欄の名前は D4 の `dev_debug`（依頼文の `debug`）。
   `incremental = true` は `CARGO_INCREMENTAL` を与えない（`=1` にしない）、`dev_debug = ""` は `CARGO_PROFILE_DEV_DEBUG` を与えない。
5. **`[scratch.sccache]` の欄は `enabled` / `port` / `binary`**（D4 の `server_port` は `port`）。既定は `enabled = true`、`port = 4236`、
   `binary = $CELERIS_STATE_DIR/tools/sccache/bin/sccache`。`enabled` でもバイナリが無い・server が応答しなければ自動で配線しない
   （`resolve_sccache` の `unavailable`）。**節を書かなくても動く**（D7 の N-1 の規則）。`ScratchSettings::with_dir`（テスト用の既定）は
   sccache を配線しない（手元の server をテストが拾わないため）。
6. **server の有無は `127.0.0.1:<port>` への TCP 接続（300 ms）だけで見る**。sccache の client（`--show-stats` など）は server が無いと
   自分で起こすので、run の開始時・checks・tick では呼ばない。`celerisctl scratch status` だけが、TCP で確かめた直後に `--show-stats
   --stats-format=json` を呼ぶ（万一の起動に備えて server と同じ env を渡す）。確かめた直後に server が落ちた run の中では client が
   server を起こしうる（run の cgroup に入る。残る危険として `docs/ops/sccache-l1.md` に書いた）。
7. **`celeris-sccache.service` の起動方法**: `SCCACHE_START_SERVER=1 SCCACHE_NO_DAEMON=1 sccache`（sccache の内部の起動経路。0.18.0 で
   前景に留まることを確認）。env は `celerisctl scratch env --server`（新設。`SCCACHE_DIR` / `SCCACHE_CACHE_SIZE` / `SCCACHE_SERVER_PORT` /
   `SCCACHE_IDLE_TIMEOUT` と `CELERIS_SCCACHE_BIN`。sccache が無効・バイナリが無ければ失敗）を `eval` して exec。unit の雛形は
   `deploy/systemd/celeris-sccache.service`（`install-units.sh` が置くだけ）。手順書 `docs/ops/sccache-l1.md`。
8. **`tools/sccache/VERSION` は版だけ（`0.18.0`）**。D4 の「配布バイナリの sha256 を照合」は採らず、`setup-sccache.sh` は
   `cargo install sccache --locked --version <VERSION> --root <tools>/sccache`（target は scratch の `agent-setup-sccache`、終わったら消す）か、
   `--from <binary>`（同じ版の既存のバイナリを写す。ネットワークに出ない）。
9. **`ScratchStatus.sccache`（`ScratchSccacheView`: state / reason / binary / port / dir / max_bytes / stats）を足した**（`#[serde(default)]`。
   逸脱 11 の「G2 で `Option` の欄として足す」）。daemon のスナップショットは状態だけ（stats は `None`）、`celerisctl scratch status` は
   `--show-stats` の要約（hits / misses / Rust の hits / misses / cache_size）も入れる。GUI の 1 行は G3（L1 / L2 の hit 率）まで変えない。
10. **run の経路を `cargo_env` に揃えた**（G1 の申し送り）。`run_worker` の scratch の分岐は `spawn_blocking` の中で allocate → resolve →
    `cargo_env_with`。checks は `check_cargo_target_env` → `cargo_env`。legacy（scratch 無効）は従来どおり `CARGO_TARGET_DIR` だけ。
    `CargoTargetPlan::Scratch` の設定は `Box`（clippy の `large_enum_variant`）。
11. **U3 の結果**: `cargo test --workspace --no-run` で target 19.48 GiB → 6.47 GiB（incremental の廃止で −5.2 GiB、`line-tables-only` で
    −7.8 GiB。`test` profile にも効く）。空からのビルドは 53.5 s → 39.3 s と速くなり、1 行の編集後の再ビルドは 5.5 s → 11.2 s（約 2 倍）。
    既定は `incremental = false` のまま（Celeris の run は空の target から始まることが多い）。sccache の依存の hit（85 %）はこのリポジトリの
    壁時計をほとんど縮めない（39.3 s → 41.0 s。リンクと workspace のメンバーが律速）。
12. **U6 の結果**: `cargo clippy --workspace --all-targets` は wrapper 経由の sccache でも exit 0。依存（`--emit=metadata`）は hit し、
    clippy-driver を通る workspace のメンバーは non-cacheable。
13. **`[commands] setup` には与えない**（G1 の逸脱 13 のまま）。setup は worktree を作った直後に 1 度だけ流すコマンドで、cargo を呼ぶ
    とは限らない。必要になれば別 Phase。

## Phase G3 実装時の逸脱・明確化（2026-09-28）

1. **U5 の答え**（phase-G.md の G3 checkpoint 1。loopback の記録用 stub に本物の sccache 0.18 を向けた）:
   - 発行されるメソッドは `GET`（entry と起動時の `/<prefix>/.sccache_check`）、`PUT`（同）、`PROPFIND`（Depth 0。PUT の前に毎回
     親の collection）、`MKCOL`（PROPFIND が 404 のときだけ親から順に）。HEAD・DELETE・MOVE・COPY・PROPPATCH・Range は出なかった。
     path は `/<SCCACHE_WEBDAV_KEY_PREFIX>/<k0>/<k1>/<k2>/<key>`（key は 64 桁の 16 進）。**207 の応答に `getlastmodified` が
     無いと opendal は PUT せずに失敗する**。cache server は collection の PROPFIND に常に 207 を返す（ディレクトリは仮想）。
   - **backend が起動時に応答しない（接続拒否・500）と sccache の server は起動に失敗する**（`Server startup failed: cache storage
     failed to read`、exit 2）。**起動後に backend が死んでもコンパイルは続く**（GET の失敗は miss、PUT の失敗は write error）。
     **backend が遅いと sccache は GET と PUT の両方を timeout なしで待つ**（8 秒の遅延で 1 crate のビルドが 16.4 秒）。
   - D5 の「cache server 自身の停止」の行の未確認事項（コンパイルを続けるか）は「続ける。ただし起動時は失敗し、遅い backend は待つ」
     で確定。これに合わせて配線を次の 2 と 3 にした。
2. **sccache の server の backend は起動時に `celerisctl scratch env --server` が選ぶ**: cache server の `/healthz` が応答すれば webdav
   （`SCCACHE_WEBDAV_ENDPOINT` / `SCCACHE_WEBDAV_KEY_PREFIX=sccache` / `SCCACHE_WEBDAV_TOKEN` / `SCCACHE_SERVER_PORT` /
   `SCCACHE_IDLE_TIMEOUT`、`SCCACHE_DIR` なし）、応答しなければ G2 の local disk（`SCCACHE_DIR`）。選んだ方を
   `<scratch>/bin/sccache-server.mode` に書く。backend は起動時に決まるので、cache server を後から有効にしたら
   `celeris-sccache.service` を再起動する（unit の順序は `After=` / `Before=` だけで、互いを引き込まない）。
3. **dispatcher の `/healthz` の確認は、mode が webdav のときだけ**（D5 は「run の開始時に `/healthz` を見る」）。disk で動く sccache は
   cache server の有無に関係なく使える。webdav で動く sccache に対して cache server が応答しなければ（hang を含む。U5 の「遅い
   backend を待ち続ける」への備え）`RUSTC_WRAPPER` を与えない。client（run）の env は G2 のまま（webdav 系も token も入れない。
   client が万一 server を起こしても local disk で起動する）。
4. **unit の名前は `celeris-scratch-cache.service`**（D5 本文の `celeris-cache.service` から、依頼文の名前に合わせた）。
   `celeris --config … --log-format text cache-server`。port の既定は D5 のとおり 4237。
5. **認証は Bearer**（D5 は Basic を第一候補）。sccache 0.18 は `SCCACHE_WEBDAV_TOKEN` を読み `Authorization: Bearer` で送る（U5 で確認）。
   token は **`<scratch>/cache-server.token`**（D5 は `~/.config/celeris/cache-server.token`。scratch pool はローカルで owner だけが
   書ける場所なので同じ保護になり、人の手順が要らない）。cache server が初回に `/dev/urandom` から作る（0600）。
   `[scratch.cache_server] token_file` で変えられる。`/healthz` と `/stats` は認証しない（loopback だけに bind）。
6. **cache server の L1 は `<scratch>/cache-l1/<k0k1>/<key>`**（D1 / D5 は `sccache-l1/` を G3 で作り直す）。cache server が落ちたときの
   fallback で sccache が `sccache-l1/` を disk cache として使い続けるので、同じ dir にすると sccache の LRU が cache server の entry を
   消しうる。**2 つの dir が両方育つと最悪 `2 × l1_max_gb`**（実際にはどちらか一方だけが使われる。`scratch status` に両方出る）。
   G2 の `sccache-l1/` は消さない。
7. **設定の欄の名前**: `[scratch.l2]` = `enabled` / `dir`（既定 `$CELERIS_STATE_DIR/cache/sccache-l2`）/ `max_gb`（D1 の `l2_max_gb`、
   既定 300）/ `flush_mbps`（D5 の `flush_mb_per_sec`、既定 25、MB = 10^6 byte、0 = 無制限）/ `flush_queue_max_mb`（4096）/
   `get_timeout_ms`（D5 の `l2_get_timeout_ms`、500）/ `io_threads`（4）/ `gc_interval_secs`（86400）。`[scratch.cache_server]` =
   `enabled` / `port`（4237）/ `token_file`。どちらも書かなくても動く（D7 の N-1 の規則）。L1 の上限は `[scratch] l1_max_gb` を使う。
8. **L2 の I/O の閉じ込め**: GET の L2 は 4 スレッド・待ち行列 16 の `L2Exec` で `get_timeout_ms` まで待ち、タイムアウト・満杯・I/O
   エラーは miss。**3 回連続の失敗で切り離し**（degraded）、5 s から倍々で最大 300 s のバックオフ、明けたら 1 回試して成功で復帰。
   flusher の書き込みは flusher のスレッドで直接行う（NFS が hang すると flusher だけが止まり、`flush_oldest_age_secs` が伸びる）。
   **L2 の root が見えない（mount が外れた・dir を消された）ときは作り直さずにエラー**（mount point の下のローカルに書かない）。
   壊れた `.zst`（checksum 不一致・切れ）は miss にして消し、切り離しの理由には数えない。
9. **L2 の書き込み**: tmp は同じ shard の `.<key>.zst.tmp-<pid>-<nanos>-<seq>`（D5 の `<key>.zst.tmp-<pid>-<ulid>` を隠しファイルにした。
   GC の走査で 1 時間より古いものを片づける）→ write → fsync → rename。既にあれば書かない（flusher も stat してから）。
10. **`.pending` は追記ログ**: `<key>`（積んだ）と `-<key>`（済んだ）。起動時に最後の記録が「積んだ」の key だけを積み直し、待ち行列が
    空になったら切り詰める（D5 は「未 flush の key を書く」だけ）。
11. **token bucket は負債を許す形**（不足分を待ってから書く）。burst は 1 秒分。区間 T に書く量は `burst + rate × T` を超えない
    （`token_bucket_limits_bytes_over_any_interval`、`flusher_respects_the_token_bucket` は仮想の時計で 100 MB を 25.00 MB/s 以下）。
12. **L2 の GC**: 走査して `max_gb` を超えていれば mtime の古い順に `× 0.9` まで消す。起動 60 s 後に初回（`l2_bytes` が分かる）、以後
    `gc_interval_secs` ごと。LRU は mtime（L2 hit で 1 日 1 回まで touch。NFS の atime は当てにしない）。
13. **HEAD は GET と同じに数える**（sccache は HEAD を送らない）。ファイルの PROPFIND は L1 の索引、無ければ L2 の stat（stats に数えない）。
14. **`celeris cache-server` は scratch が無効なら起動しない**（L1 を NFS に置かない。D1 の原則）。SIGTERM で待ち行列を `.pending` に
    残して止まる。
15. **sccache 0.18 は multilevel cache（disk + remote）を内蔵している**（U5 の調査で判明）。D5 (b) のまま Celeris の cache server が
    階層を持つ（L2 への書き込みを critical path の外に置く・帯域制限・切り離し・GC を Celeris が制御するため）。
16. **U4 は保留のまま**: 手動 e2e（このリポジトリ、L2 はローカル）で L1 を消した後の再ビルドが Rust 165 / 196 hit・promote 582 で、
    L1 hit と同じ率（壁時計 37.8 s）。NFS の遅延での頭打ちは未測定（本番の L2 で測る）。事前 promote は入れていない。
17. **所見（提案 P-G3-2）**: 毎回 miss する約 30 crate（workspace のメンバーと `OUT_DIR` 依存。G2 の残り）は owner ごとに key が変わるので、
    毎 run 約 100 MB を L2 に書き、二度と hit しない（L2 の LRU で回収されるが NFS の帯域を使う）。
