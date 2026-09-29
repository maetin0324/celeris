use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;
use task_dispatch::ClusterSpec;

use super::Config;

/// `[[clusters]]`（ADR-0018）: ssh でコマンドを実行するクラスタ。接続は人が張った ControlMaster を借りる。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterConfig {
    /// タスクの `WorkspaceSpec::Remote{cluster}` が指す名前。
    pub id: String,
    /// `~/.ssh/config` の `Host` 名（`ControlMaster` の設定が要る）。
    pub host: String,
    /// ADR-0059 D6: クラスタ側の実効の作業ディレクトリ（例 `/work/NBB/rmaeda`）。省略可。
    /// `WorkspaceSpec::Remote.path` が省略・相対のときの基準になる（絶対パス・`~`/`~/…` はそのまま）。
    /// GUI から `PUT /clusters/{id}/settings` で上書きできる（DB の値が勝つ。`ClusterSettings`）。
    #[serde(default)]
    pub work_dir: Option<PathBuf>,
    /// このクラスタで同時に走らせる run の上限。
    #[serde(default = "default_cluster_concurrency")]
    pub concurrency: usize,
    /// `worktree`（ADR-0019 D4 の既定の選択。git 管理下のプロジェクト）、`rsync`（設定の既定）、`none`（共有ファイルシステム）。
    #[serde(default = "default_cluster_sync")]
    pub sync: String,
    /// ADR-0032 D1: 接続の認証方式。`"manual"`（既定。celeris は接続を張らない。ADR-0018 D2 のまま）/
    /// `"publickey"`（鍵だけで入れる。ディスパッチャが自動で接続を試みる。ADR-0032 D3）/
    /// `"totp"`（publickey の後に検証コードが要る。GUI から中継する。ADR-0032 D4）。
    #[serde(default = "default_cluster_auth")]
    pub auth: String,
    /// push（手元 → クラスタ）で手元に無いファイルを消すか。既定 false（既存プロジェクトを壊さない）。
    #[serde(default)]
    pub delete_on_push: bool,
    /// コマンドの前に流す準備（`module load ...` など）。
    #[serde(default)]
    pub setup: Vec<String>,
    /// リモートで `export` する環境変数。
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// `rsync` から除外するパターン（`.taskd/` は常に除外）。
    #[serde(default)]
    pub rsync_excludes: Vec<String>,
    /// ADR-0019 D1: worktree を置く親ディレクトリ（既定は `<project>/.celeris-worktrees`）。`sync = "worktree"` のときだけ使う。
    #[serde(default)]
    pub worktree_root: Option<PathBuf>,
    /// ADR-0019 D1: worktree を切り出す元（既定 `HEAD`）。
    #[serde(default = "default_worktree_base")]
    pub worktree_base: String,
    /// ADR-0019 D1: sparse-checkout で残すパス（空なら全追跡ファイル）。巨大な追跡データを外すのに使う。
    #[serde(default)]
    pub worktree_paths: Vec<String>,
    /// ADR-0019 D2: worktree をいつ消すか。`"never"`（既定、人が消す）のみ実装。
    #[serde(default = "default_remove_worktree_when")]
    pub remove_worktree_when: String,
    /// ADR-0053 D3（Phase 66）: このクラスタの ssh master に張る port forward（Qwen トンネル等）。
    /// 空（既定）なら celeris はトンネルの生存を見ない（従来どおり）。
    #[serde(default)]
    pub forwards: Vec<ClusterForwardConfig>,
    /// ADR-0060（Phase 103）: master の起こし方。`"auto"`（既定）は `systemd-run` が PATH にあり
    /// `XDG_RUNTIME_DIR` が設定されていれば `"systemd-run"`、無ければ `"inline"` として扱う。
    /// `"systemd-run"`: `systemd-run --user --scope` で celeris（`celeris@<sha12>` unit）の cgroup の
    /// 外の一時 scope に起こす。unit が `KillMode=control-group`（既定）で止まっても master は道連れに
    /// ならない。`"inline"`: 従来どおり celeris の直接の子として起こす（cgroup の中に留まる）。
    #[serde(default = "default_master_launcher")]
    pub master_launcher: String,
    /// ADR-0062 A（Phase 107）: master の argv に足す `-o ServerAliveInterval=<n> -o
    /// ServerAliveCountMax=3 -o TCPKeepAlive=yes`。既定 30 秒、`0` で無効。
    #[serde(default = "default_keepalive_secs")]
    pub keepalive_secs: u64,
    /// ADR-0062 A: master 越しの実通信（`ssh -o BatchMode=yes <host> -- true`）による生存確認の間隔。
    /// 既定 300 秒、`0` で無効。
    #[serde(default = "default_liveness_probe_secs")]
    pub liveness_probe_secs: u64,
    /// ADR-0078 D1: master の argv に足す `-o ControlPersist=<v>`。既定 `"yes"`（idle で終わらない）。
    /// `"yes"` か正の秒数（例 `"28800"`）だけを受ける（それ以外は `Config::validate` のエラー）。
    /// 既定を変える運用は想定しない（逃げ道）。
    #[serde(default = "default_control_persist")]
    pub control_persist: String,
}

/// `[[clusters.forwards]]`（ADR-0053 D3）: 1 本の port forward。
///
/// ```toml
/// [[clusters]]
/// id = "pegasus"
/// host = "pegasus"
/// auth = "totp"
///
/// [[clusters.forwards]]
/// listen = "127.0.0.1:18000"   # celeris がこの手元のアドレスに bind する
/// target = "bnode150:18000"    # pegasus（master のホスト側）から見た転送先
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterForwardConfig {
    /// ローカル（celeris が bind する側）。`"127.0.0.1:18000"` の形。
    pub listen: String,
    /// master のホスト側から見た転送先。`"bnode150:18000"` の形。
    pub target: String,
    /// ADR-0053 Phase 85: target（`/v1/models`）の健康 probe をこの秒数より短い間隔では行わない
    /// （既定 30 秒）。listener（`-O forward` の有無）の確認はこれに縛られず毎回行う。forward は
    /// 張れているのに先方（bnode150 の vLLM 等）が落ちている間、probe を毎 tick 叩いて tick を
    /// 遅くしないためのバックオフ（本番観測、`docs/adr/0053-llm-source-proxy.md`「Phase 85 追記」）。
    #[serde(default = "default_tunnel_probe_interval_secs")]
    pub probe_interval_secs: u64,
}

fn default_tunnel_probe_interval_secs() -> u64 {
    task_dispatch::dispatcher::DEFAULT_TUNNEL_PROBE_INTERVAL_SECS
}

fn default_cluster_concurrency() -> usize {
    2
}
fn default_cluster_sync() -> String {
    "rsync".to_string()
}
fn default_cluster_auth() -> String {
    "manual".to_string()
}
fn default_master_launcher() -> String {
    "auto".to_string()
}
fn default_keepalive_secs() -> u64 {
    30
}
fn default_control_persist() -> String {
    "yes".to_string()
}
fn default_liveness_probe_secs() -> u64 {
    300
}
fn default_worktree_base() -> String {
    "HEAD".to_string()
}
fn default_remove_worktree_when() -> String {
    "never".to_string()
}

impl Config {
    /// ADR-0018: `[[clusters]]` を task-dispatch の型に写す（`env` はキー順で決定的に並べる）。
    pub fn cluster_specs(&self) -> HashMap<String, ClusterSpec> {
        self.clusters
            .iter()
            .map(|c| {
                let mut env: Vec<(String, String)> =
                    c.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                env.sort();
                (
                    c.id.clone(),
                    ClusterSpec {
                        id: c.id.clone(),
                        host: c.host.clone(),
                        concurrency: c.concurrency,
                        sync: match c.sync.as_str() {
                            "none" => task_worker::SyncMode::None,
                            "worktree" => task_worker::SyncMode::Worktree,
                            _ => task_worker::SyncMode::Rsync,
                        },
                        delete_on_push: c.delete_on_push,
                        setup: c.setup.clone(),
                        env,
                        rsync_excludes: c.rsync_excludes.clone(),
                        auth: c.auth.clone(),
                        worktree: task_worker::WorktreeSettings {
                            root: c.worktree_root.clone(),
                            base: c.worktree_base.clone(),
                            paths: c.worktree_paths.clone(),
                            ..Default::default()
                        },
                        // ADR-0059 D6: 設定ファイルの `work_dir`。DB の上書きは dispatcher 側
                        // （`cluster_of`）が実行時に合成する。
                        work_dir: c.work_dir.clone(),
                        // ADR-0062 A（Phase 107）。
                        keepalive_secs: c.keepalive_secs,
                        liveness_probe_secs: c.liveness_probe_secs,
                        // ADR-0053 D3（Phase 66）。
                        forwards: c
                            .forwards
                            .iter()
                            .map(|f| task_dispatch::dispatcher::ClusterForwardSpec {
                                listen: f.listen.clone(),
                                target: f.target.clone(),
                                probe_interval_secs: f.probe_interval_secs,
                            })
                            .collect(),
                    },
                )
            })
            .collect()
    }

    /// ADR-0019 D2: `TaskDetail.worktree` を組み立てるのに要る分だけを写す。
    pub fn cluster_view_infos(&self) -> HashMap<String, task_ops::view::ClusterViewInfo> {
        self.clusters
            .iter()
            .map(|c| {
                (
                    c.id.clone(),
                    task_ops::view::ClusterViewInfo {
                        sync: c.sync.clone(),
                        worktree_root: c.worktree_root.clone(),
                        auth: c.auth.clone(),
                    },
                )
            })
            .collect()
    }
}
