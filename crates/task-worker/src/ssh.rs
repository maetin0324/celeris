//! クラスタ上でコマンドを実行する `Workspace`（ADR-0018）。
//!
//! **ワーカー（LLM）は手元で動く。** ここで行うのは「コマンドをクラスタで実行すること」と「ファイルの同期」だけ。
//! 接続は**人が張った `ControlMaster` の多重接続を借りる**（`BatchMode=yes` で、対話的な認証は絶対に行わない）。
//! 接続が無ければ `WorkspaceError::Unreachable` を返し、呼び出し側（ディスパッチャ）が供給側失敗として扱う。

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use task_core::{ArtifactRef, Task};

use crate::workspace::{ExecResult, LocalWorkspace, Workspace, WorkspaceError};

/// ワークスペースの同期方法（ADR-0018 D4、ADR-0019 D1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// run の前後で rsync を往復させる。git 管理外の小さなディレクトリ向け。
    Rsync,
    /// 共有ファイルシステム。何もしない。
    None,
    /// クラスタ側で `git worktree` を切り、その中だけを rsync する（ADR-0019）。
    /// 未追跡の巨大データを持ち込まない。git 管理下のプロジェクトの既定の選び方。
    Worktree,
}

/// ADR-0019 D1: worktree の設定。`SyncMode::Worktree` のときだけ使う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSettings {
    /// worktree を置く親ディレクトリ（既定は `<project>/.celeris-worktrees`）。
    pub root: Option<PathBuf>,
    /// 切り出す元（既定 `HEAD`）。
    pub base: String,
    /// sparse-checkout で残すパス（空なら全追跡ファイル）。
    pub paths: Vec<String>,
    /// ブランチ名の接頭辞（既定 `celeris/`）。
    pub branch_prefix: String,
}

impl Default for WorktreeSettings {
    fn default() -> Self {
        Self {
            root: None,
            base: "HEAD".to_string(),
            paths: Vec::new(),
            branch_prefix: "celeris/".to_string(),
        }
    }
}

/// 同期の両方向で常に除外するもの（P-46）。celeris が写しに作る管理用のディレクトリで、
/// クラスタ側の既存プロジェクトに持ち込まないし、`--delete` 付きの pull で手元から消してもいけない。
/// `artifacts/` は**除外しない**（成果物はクラスタで作られることがあり、受け入れ条件の照合に要る）。
/// ADR-0059 D5: `.taskd/` はプラットフォーム名が taskd だった頃の名残（`.celeris/` に改名した）。
/// 後方互換のため両方を除外に残す（古い写し・クラスタ側の残骸を同期に巻き込まない）。
pub const SYNC_ALWAYS_EXCLUDED: [&str; 4] = [".taskd/", ".celeris/", "runs/", "inputs/"];

/// ADR-0079 R5b-fix2: `--delete` 付きの pull で**手元から消さない**もの（rsync の protect フィルタ
/// `P`）。転送は両方向とも行う（成果物はクラスタで作られることがあり、クラスタ側の判定コマンドが
/// 見ることもあるので除外はしない）。クラスタ側に無いからといって手元の成果物を消すと、申告済みの
/// 成果物（兄弟 task の `artifacts/experiment-plan.md` など）が次の pull で失われる（本番 2026-09-29）。
pub const SYNC_PULL_PROTECTED: [&str; 1] = ["artifacts/"];

/// ADR-0079 R5b-fix2: 手元の写しの「まだクラスタへ push できていない」印。`.celeris/` の下なので
/// 同期の対象外（pull の `--delete` でも消えない）。これがある間の pull は、先に push を済ませる
/// （push が落ちたら pull しない = 手元の編集を消さない）。
pub const PUSH_PENDING_MARKER: &str = ".celeris/push-pending";

/// ADR-0019 付記（Phase R6-3）: worktree 準備のスクリプトが、submodule を初期化したときに標準出力へ出す行の頭
/// （続けて初期化後の submodule の数）。初期化が要らなかった（`.gitmodules` が無い・全部済み）ときは出さない。
const SUBMODULES_INITIALISED_PREFIX: &str = "celeris-submodules-initialised ";

/// ADR-0019 付記（Phase R7-4）: submodule を 1 つずつ初期化して、失敗したものごとに標準出力へ出す行の頭
/// （続けて `<path>\t<stderr の要点の 1 行>`）。失敗は準備を止めない（警告の進行の行になる）。
const SUBMODULE_FAILED_PREFIX: &str = "celeris-submodule-failed ";

/// ADR-0019 付記（Phase R6-3 / R7-4）: submodule の段が使えないときの exit（65 / 66 と区別する）。
/// R7-4 から個々の submodule の初期化の失敗では使わない（警告にする）。`git submodule status` 自体が
/// 動かない（git に submodule が無い・worktree が壊れている）ときだけ。
const SUBMODULE_INIT_FAILED_EXIT: i32 = 67;

/// 1 タスク分のリモート実行の設定。
#[derive(Debug, Clone)]
pub struct SshSettings {
    /// 設定の `[[clusters]] id`（ログとイベントに出す）。
    pub cluster: String,
    /// `~/.ssh/config` の `Host` 名。
    pub host: String,
    /// このタスクのリモート作業ディレクトリ（`remote_workdir` + タスクのディレクトリ名）。
    pub remote_dir: PathBuf,
    /// コマンドの前に流す準備（`module load ...` など）。
    pub setup: Vec<String>,
    /// リモートで `export` する環境変数（値は決定的な順で並べる）。
    pub env: Vec<(String, String)>,
    pub sync: SyncMode,
    /// ADR-0019: `sync = "worktree"` のときの設定。
    pub worktree: WorktreeSettings,
    /// タスク ID（worktree のディレクトリ名とブランチ名に使う。ADR-0019 D2）。
    pub task_id: String,
    /// push（手元 → クラスタ）で、手元に無いファイルをクラスタ側から消すか（ADR-0018 D4）。
    /// **既定は false**。既存プロジェクトを指すタスクでファイルを失わないため。celeris 専用の作業ディレクトリなら true にしてよい。
    pub delete_on_push: bool,
    /// `exec` の前に push、後に pull するか（既定 true）。判定コマンドが手元の編集を見て、
    /// その結果の成果物が手元に戻るようにするため（ADR-0018 D4）。
    pub sync_around_exec: bool,
    /// `rsync` から除外するパターン（`.taskd/` は常に除外する）。
    pub rsync_excludes: Vec<String>,
    /// `ssh` の起動コマンド（テストで差し替える。既定は `["ssh"]`）。
    pub ssh_command: Vec<String>,
    /// `rsync` の起動コマンド（同上）。
    pub rsync_command: Vec<String>,
}

impl SshSettings {
    pub fn new(
        cluster: impl Into<String>,
        host: impl Into<String>,
        remote_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            cluster: cluster.into(),
            host: host.into(),
            remote_dir: remote_dir.into(),
            setup: Vec::new(),
            env: Vec::new(),
            sync: SyncMode::Rsync,
            worktree: WorktreeSettings::default(),
            task_id: String::new(),
            delete_on_push: false,
            sync_around_exec: true,
            rsync_excludes: Vec::new(),
            ssh_command: vec!["ssh".to_string()],
            rsync_command: vec!["rsync".to_string()],
        }
    }

    /// ADR-0019 D1: 同期とコマンド実行の対象。`worktree` のときは worktree のパス、それ以外は `remote_dir`。
    pub fn effective_remote_dir(&self) -> PathBuf {
        match self.sync {
            SyncMode::Worktree => self.worktree_dir(),
            _ => self.remote_dir.clone(),
        }
    }

    /// worktree のパス（`worktree_root`/`<task_id>`。既定の root は `<project>/.celeris-worktrees`）。
    pub fn worktree_dir(&self) -> PathBuf {
        let root = self
            .worktree
            .root
            .clone()
            .unwrap_or_else(|| self.remote_dir.join(".celeris-worktrees"));
        root.join(&self.task_id)
    }

    /// worktree のブランチ名（ADR-0019 D2: celeris は commit しない。人が見てから扱う）。
    pub fn worktree_branch(&self) -> String {
        format!("{}{}", self.worktree.branch_prefix, self.task_id)
    }
}

/// `sh` に渡す 1 引数としての安全な引用（シングルクォートで囲み、中の `'` を `'\''` にする）。
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// ADR-0059 D2: リモート側のパスをシェルの引数として安全に書く。先頭の `~`（単独）または `~/…` だけを
/// `"$HOME"`（クォート済みのシェル変数）に展開する（`~user` は展開しない。ADR-0039 D5 の
/// `expand_home` と同じ方針: celeris は他ユーザの home を知らない）。それ以外はこれまでどおり `shq`
/// でシングルクォート引用する。worktree 準備・`mkdir`・`cd`・`.celeris/remote-exec` の `cd` など、
/// クラスタ側のパスをスクリプトに埋め込むすべての箇所がこれを通る。
fn shell_remote_path(raw: &str) -> String {
    if raw == "~" {
        return "\"$HOME\"".to_string();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return format!("\"$HOME\"{}", shq(&format!("/{rest}")));
    }
    shq(raw)
}

/// ADR-0059 D6: `path`（タスクの workspace）と実効 `work_dir`（呼び出し側が DB 上書き > 設定 > 無し の
/// 順で決めたもの）から、クラスタで使うディレクトリを決める（純粋関数。celeris はこれ以上パスを
/// 書き換えない）。絶対パス（`/` 始まり）・`~`/`~/…` はそのまま使う（展開は上の `shell_remote_path`
/// が実行時に行う）。空、またはそれ以外の相対パスは `work_dir` からの相対にする。`work_dir` が無ければ
/// `None`（呼び出し側は `path` をそのまま返し、`remote_dir_is_resolved` で「解決できなかった」ことを
/// 検出する）。
pub fn resolve_remote_dir(path: &Path, work_dir: Option<&Path>) -> Option<PathBuf> {
    let raw = path.to_string_lossy();
    if raw.starts_with('/') || raw.starts_with('~') {
        return Some(path.to_path_buf());
    }
    let base = work_dir?;
    if raw.is_empty() {
        Some(base.to_path_buf())
    } else {
        Some(base.join(path))
    }
}

/// ADR-0059 D6: `resolve_remote_dir` が解決できたかどうかを、結果の `PathBuf` から判定する
/// （`work_dir` が無いまま `path` をそのまま返した場合は絶対でも `~` 始まりでもない）。
pub fn remote_dir_is_resolved(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    raw.starts_with('/') || raw.starts_with('~')
}

/// ローカルの作業ディレクトリを持ちつつ、コマンドをリモートで実行するワークスペース。
#[derive(Debug, Clone)]
pub struct SshWorkspace {
    local: LocalWorkspace,
    settings: SshSettings,
    /// Phase R6-3: 準備の途中で人に見せたい進行の行（submodule の初期化など）。呼び出し側が
    /// `take_progress_notes` で取り出して `WorkerProgress` に積む（clone は同じ入れ物を共有する）。
    notes: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl SshWorkspace {
    pub fn new(dir: impl Into<PathBuf>, settings: SshSettings) -> Self {
        Self {
            local: LocalWorkspace::new(dir),
            settings,
            notes: std::sync::Arc::default(),
        }
    }

    /// Phase R6-3: 準備（`prepare` / `push` / `pull` / `exec`）の途中で溜まった進行の行を取り出す
    /// （例: `initialised 3 submodules in <worktree> on cluster sirius`）。取り出した行は消える。
    pub fn take_progress_notes(&self) -> Vec<String> {
        let mut guard = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }

    fn push_progress_note(&self, line: String) {
        let mut guard = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(line);
    }

    pub fn dir(&self) -> &Path {
        self.local.dir()
    }

    pub fn settings(&self) -> &SshSettings {
        &self.settings
    }

    /// ADR-0019 D1: 同期とコマンド実行の対象。`worktree` のときは worktree のパス、それ以外は `remote_dir`。
    pub fn effective_remote_dir(&self) -> PathBuf {
        self.settings.effective_remote_dir()
    }

    /// worktree のパス（`worktree_root`/`<task_id>`。既定の root は `<project>/.celeris-worktrees`）。
    fn worktree_dir(&self) -> PathBuf {
        self.settings.worktree_dir()
    }

    /// worktree のブランチ名（ADR-0019 D2: celeris は commit しない。人が見てから扱う）。
    pub fn worktree_branch(&self) -> String {
        self.settings.worktree_branch()
    }

    /// ADR-0019 D1: クラスタ側に worktree を用意する（既にあれば再利用）。sparse-checkout の指定があれば絞る。
    async fn ensure_worktree(&self) -> Result<(), WorkspaceError> {
        if self.settings.task_id.is_empty() {
            return Err(WorkspaceError::Remote(
                "sync = \"worktree\" needs the task id (the worktree directory and branch are named after it)".to_string(),
            ));
        }
        let project = self.settings.remote_dir.to_string_lossy().to_string();
        let wt = self.worktree_dir().to_string_lossy().to_string();
        let branch = self.worktree_branch();
        let base = &self.settings.worktree.base;
        let mut inner = format!(
            "if [ ! -e {wt}/.git ]; then git worktree add -B {branch} {wt} {base} >/dev/null; fi\n",
            wt = shell_remote_path(&wt),
            branch = shq(&branch),
            base = shq(base),
        );
        if !self.settings.worktree.paths.is_empty() {
            let paths: Vec<String> = self
                .settings
                .worktree
                .paths
                .iter()
                .map(|p| shq(p))
                .collect();
            inner.push_str(&format!(
                "git -C {wt} sparse-checkout set --cone {paths} >/dev/null\n",
                wt = shell_remote_path(&wt),
                paths = paths.join(" ")
            ));
        }
        // Phase R6-3: submodule を使うリポジトリ（BenchFS の lib/locusta など）は、`git worktree add` の直後は
        // submodule のディレクトリが空で、Cargo の path 依存が解決できない。`.gitmodules` があり、未初期化
        // （`git submodule status` の行頭が `-`）のものが 1 つでもあれば `submodule update --init --recursive`
        // する（再利用のときは初期化済みなら何もしない = 冪等）。worktree ごとの `modules/` に clone するので
        // 共有の錠は先に外す（大きな submodule の clone で他のタスクの worktree 作成を待たせない）。
        // Phase R7-4: submodule は `.gitmodules` の path ごとに 1 つずつ初期化し、失敗しても続ける（本番
        // 2026-09-30: `ior_integration/ior` の固定 commit が remote に無く、1 つの失敗で準備ごと落ちた）。
        // 失敗は `<SUBMODULE_FAILED_PREFIX><path>\t<要点>` の行で返し、Rust 側で警告の進行の行にする。
        inner.push_str(&format!(
            "exec 9>&-\n\
             if [ -f {wt}/.gitmodules ]; then\n\
               sm_status=$(git -C {wt} submodule status --recursive 2>&1) || {{ printf '%s\\n' \"git submodule status failed: $sm_status\" >&2; exit {code}; }}\n\
               if printf '%s\\n' \"$sm_status\" | grep -q '^-'; then\n\
                 sm_paths=$(git -C {wt} config -f .gitmodules --get-regexp '^submodule\\..*\\.path$' 2>/dev/null | sed 's/^[^ ]* //' || true)\n\
                 sm_report=$(printf '%s\\n' \"$sm_paths\" | while IFS= read -r sm_path; do\n\
                   [ -n \"$sm_path\" ] || continue\n\
                   if sm_err=$(git -C {wt} submodule update --init --recursive -- \"$sm_path\" 2>&1 >/dev/null); then :; else\n\
                     sm_why=$(printf '%s\\n' \"$sm_err\" | grep -E '^(fatal|error):' | head -n 1 || true)\n\
                     [ -n \"$sm_why\" ] || sm_why=$(printf '%s\\n' \"$sm_err\" | grep -v '^[[:space:]]*$' | head -n 1 || true)\n\
                     printf '%s%s\\t%s\\n' '{failed}' \"$sm_path\" \"$sm_why\"\n\
                   fi\n\
                 done)\n\
                 [ -z \"$sm_report\" ] || printf '%s\\n' \"$sm_report\"\n\
                 sm_failed=$(printf '%s\\n' \"$sm_report\" | grep '^{failed}' | cut -f1 | sed 's/^{failed}//' || true)\n\
                 sm_count=$(git -C {wt} submodule status --recursive 2>/dev/null | sm_failed=\"$sm_failed\" awk 'BEGIN {{ n = split(ENVIRON[\"sm_failed\"], f, \"\\n\") }} /^-/ {{ next }} {{ p = substr($0, 2); sub(/^[^ ]* /, \"\", p); for (i = 1; i <= n; i++) if (f[i] != \"\" && (p == f[i] || index(p, f[i] \" \") == 1 || index(p, f[i] \"/\") == 1)) next; c++ }} END {{ print c + 0 }}' || true)\n\
                 echo \"{prefix}$sm_count\"\n\
               fi\n\
             fi\n",
            wt = shell_remote_path(&wt),
            code = SUBMODULE_INIT_FAILED_EXIT,
            prefix = SUBMODULES_INITIALISED_PREFIX,
            failed = SUBMODULE_FAILED_PREFIX,
        ));
        // 同じリポジトリに対して複数のタスクが同時に worktree を作ることがある（クラスタの並列度 > 1）。
        // git の worktree 管理は共有なので、あれば `flock` で直列化する（無ければそのまま実行する）。
        let script = format!(
            "set -e\n\
             cd {project} 2>/dev/null || {{ echo \"no such directory: {project_display}\" >&2; exit 66; }}\n\
             gitdir=$(git rev-parse --git-common-dir 2>/dev/null) || {{ echo \"not a git repository\" >&2; exit 65; }}\n\
             if command -v flock >/dev/null 2>&1; then\n\
               exec 9>\"$gitdir/celeris-worktree.lock\"\n\
               flock 9\n\
             fi\n\
             {inner}",
            project = shell_remote_path(&project),
            project_display = project,
            inner = inner,
        );
        let out = self.run_ssh(&script, Duration::from_secs(600)).await?;
        match out.exit {
            Some(0) => {
                if let Some(count) = out
                    .stdout_tail
                    .lines()
                    .find_map(|l| l.trim().strip_prefix(SUBMODULES_INITIALISED_PREFIX))
                    .map(str::trim)
                    .filter(|c| !c.is_empty() && *c != "0")
                {
                    let line = format!(
                        "initialised {count} submodules in {wt} on cluster {}",
                        self.settings.cluster
                    );
                    tracing::info!(cluster = %self.settings.cluster, worktree = %wt, submodules = %count, "initialised git submodules in the cluster worktree (R6-3)");
                    self.push_progress_note(line);
                }
                // Phase R7-4: 初期化できなかった submodule は準備を止めず、1 つずつ警告の進行の行にする。
                for rest in out
                    .stdout_tail
                    .lines()
                    .filter_map(|l| l.strip_prefix(SUBMODULE_FAILED_PREFIX))
                {
                    let (path, why) = rest.split_once('\t').unwrap_or((rest, ""));
                    let why = why.trim();
                    let why = if why.is_empty() { "unknown error" } else { why };
                    tracing::warn!(cluster = %self.settings.cluster, worktree = %wt, submodule = %path, error = %why, "a git submodule could not be initialised in the cluster worktree (R7-4, best-effort)");
                    self.push_progress_note(format!(
                        "submodule {path} could not be initialised: {why} (worktree {wt} on cluster {})",
                        self.settings.cluster
                    ));
                }
                Ok(())
            }
            Some(SUBMODULE_INIT_FAILED_EXIT) => Err(WorkspaceError::Remote(format!(
                "cannot run the git submodule step in the worktree {wt} on {} (exit {SUBMODULE_INIT_FAILED_EXIT}): {}",
                self.settings.cluster,
                out.stderr_tail.trim()
            ))),
            // ADR-0059 D3: 専用のバリアントにする（呼び出し側が「格下げしてよいか」を型で判定できるように。
            // 文字列のパースはしない）。
            Some(65) => Err(WorkspaceError::NotAGitRepository(format!(
                "{project} on {} is not a git repository; use sync = \"rsync\" for this cluster (ADR-0019 D3), \
                 or leave `mode` unset with no `repos` on a command-only task to run it as `shared` (ADR-0059 D3)",
                self.settings.cluster
            ))),
            other => Err(WorkspaceError::Remote(format!(
                "cannot prepare the git worktree on {} (exit {other:?}): {}",
                self.settings.cluster,
                out.stderr_tail.trim()
            ))),
        }
    }

    /// `ssh` に必ず付ける引数（対話的な認証を禁じる）。
    fn ssh_base(&self) -> Vec<String> {
        let mut args = self.settings.ssh_command.clone();
        args.push("-o".into());
        args.push("BatchMode=yes".into());
        args
    }

    /// 人が張った多重接続があるか（ADR-0018 D2）。無ければ celeris は何もできない。
    pub async fn control_master_alive(&self) -> bool {
        let mut args = self.ssh_base();
        args.push("-O".into());
        args.push("check".into());
        args.push(self.settings.host.clone());
        let Some((program, rest)) = args.split_first() else {
            return false;
        };
        match tokio::process::Command::new(program)
            .args(rest)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
        {
            Ok(status) => status.success(),
            Err(_) => false,
        }
    }

    /// リモートで走らせるスクリプト（作業ディレクトリへ移動 → env → setup → コマンド）。
    fn remote_script(&self, cmd: &str, timeout: Duration) -> String {
        let mut script = String::new();
        script.push_str(&format!(
            "cd {} && ",
            shell_remote_path(&self.effective_remote_dir().to_string_lossy())
        ));
        for (k, v) in &self.settings.env {
            script.push_str(&format!("export {k}={} && ", shq(v)));
        }
        for line in &self.settings.setup {
            script.push_str(&format!("{{ {line}; }} && "));
        }
        // ローカル側の timeout に加えて、リモートでも kill する（二重の安全弁。ADR-0018 D5）。
        let secs = timeout.as_secs().max(1);
        script.push_str(&format!("timeout -k 5 {secs} sh -c {}", shq(cmd)));
        script
    }

    /// リモートの作業ディレクトリを作る。
    async fn ensure_remote_dir(&self) -> Result<(), WorkspaceError> {
        // worktree のときは `git worktree add` が作るので、ここでは作らない。
        if self.settings.sync == SyncMode::Worktree {
            return self.ensure_worktree().await;
        }
        let dir = self.effective_remote_dir().to_string_lossy().to_string();
        let out = self
            .run_ssh(
                &format!("mkdir -p {}", shell_remote_path(&dir)),
                Duration::from_secs(60),
            )
            .await?;
        if out.exit != Some(0) {
            return Err(WorkspaceError::Remote(format!(
                "cannot create the remote directory {dir} on {}: {}",
                self.settings.cluster,
                out.stderr_tail.trim()
            )));
        }
        Ok(())
    }

    /// `ssh` で 1 コマンド。終了コード 255 は ssh 自身の失敗（= 接続の問題）として `Unreachable` にする。
    async fn run_ssh(&self, script: &str, timeout: Duration) -> Result<ExecResult, WorkspaceError> {
        let mut args = self.ssh_base();
        args.push(self.settings.host.clone());
        args.push(script.to_string());
        let result = run_command(&args, timeout).await?;
        if result.exit == Some(255) {
            return Err(WorkspaceError::Unreachable(format!(
                "ssh to {} ({}) failed: {}",
                self.settings.cluster,
                self.settings.host,
                result.stderr_tail.trim()
            )));
        }
        Ok(result)
    }

    /// push（手元 → クラスタ）の rsync 引数。既定では `--delete` を付けない（既存プロジェクトの
    /// ファイルを消さない）。副作用の無い組み立てだけ（テストで除外・保護の並びを確かめる）。
    pub fn push_args(&self) -> Vec<String> {
        let mut args = self.settings.rsync_command.clone();
        args.push("-a".into());
        if self.settings.delete_on_push {
            args.push("--delete".into());
        }
        args.push("-e".into());
        args.push(self.ssh_base().join(" "));
        // P-46: celeris の管理用ディレクトリ（ラッパ・run のログ・入力）はクラスタへ送らない。
        for pattern in SYNC_ALWAYS_EXCLUDED {
            args.push("--exclude".into());
            args.push(pattern.into());
        }
        for pattern in &self.settings.rsync_excludes {
            args.push("--exclude".into());
            args.push(pattern.clone());
        }
        args.push(format!("{}/", self.local.dir().to_string_lossy()));
        args.push(format!(
            "{}:{}/",
            self.settings.host,
            self.effective_remote_dir().to_string_lossy()
        ));
        args
    }

    /// pull（クラスタ → 手元）の rsync 引数。写しは celeris が作り直してよいので `--delete` を付けるが、
    /// 管理用ディレクトリは除外し、成果物（`SYNC_PULL_PROTECTED`）は消さない（R5b-fix2）。
    pub fn pull_args(&self) -> Vec<String> {
        let mut args = self.settings.rsync_command.clone();
        args.extend(["-a".into(), "--delete".into()]);
        args.push("-e".into());
        args.push(self.ssh_base().join(" "));
        // P-46: `--delete` で手元の run のログ・入力を消さない（クラスタ側には無いため）。
        for pattern in SYNC_ALWAYS_EXCLUDED {
            args.push("--exclude".into());
            args.push(pattern.into());
        }
        // R5b-fix2: 成果物は受け取るが、クラスタに無いことを理由に手元から消さない。
        for pattern in SYNC_PULL_PROTECTED {
            args.push(format!("--filter=P {pattern}"));
        }
        for pattern in &self.settings.rsync_excludes {
            args.push("--exclude".into());
            args.push(pattern.clone());
        }
        args.push(format!(
            "{}:{}/",
            self.settings.host,
            self.effective_remote_dir().to_string_lossy()
        ));
        args.push(format!("{}/", self.local.dir().to_string_lossy()));
        args
    }

    fn push_pending_path(&self) -> PathBuf {
        self.local.dir().join(PUSH_PENDING_MARKER)
    }

    /// 手元に「まだ push できていない編集」があるか（R5b-fix2 の印があるか）。
    pub fn push_pending(&self) -> bool {
        self.push_pending_path().exists()
    }

    /// 手元の写し → クラスタ（作業のあと、判定の前。ADR-0018 D4）。
    /// 既定では `--delete` を付けない（既存プロジェクトのファイルを消さない）。
    /// 成功したら R5b-fix2 の印（`PUSH_PENDING_MARKER`）を消す（手元の内容はクラスタに届いた）。
    pub async fn push(&self) -> Result<(), WorkspaceError> {
        if self.settings.sync == SyncMode::None {
            return Ok(());
        }
        self.ensure_remote_dir().await?;
        self.run_rsync(&self.push_args(), "push").await?;
        match tokio::fs::remove_file(self.push_pending_path()).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0079 R5b-fix2: ワーカー run が終わるたびに呼ぶ push。先に印を置いてから push するので、
    /// push が落ちても（接続断・途中で daemon が止まっても）印が残り、次の pull は `--delete` の前に
    /// push をやり直す（手元の編集を消さない）。rsync -a（`--delete` 無し）なので何度呼んでも同じ結果。
    /// `SyncMode::None`（共有ファイルシステム）では何もしない。
    pub async fn push_after_run(&self) -> Result<(), WorkspaceError> {
        if self.settings.sync == SyncMode::None {
            return Ok(());
        }
        let marker = self.push_pending_path();
        if let Some(parent) = marker.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&marker, b"push pending (ADR-0079 R5b-fix2)\n").await?;
        self.push().await
    }

    /// クラスタ → 手元の写し（run の前と、判定の後。ADR-0018 D4）。
    /// 写しは celeris が作り直してよいので、こちらは `--delete` してよい。
    /// ADR-0079 R5b-fix2: 手元に push できていない編集（印）があれば、先に push する。push が落ちたら
    /// pull はしない（`--delete` で手元の編集を消さない）。
    pub async fn pull(&self) -> Result<(), WorkspaceError> {
        if self.settings.sync == SyncMode::None {
            return Ok(());
        }
        if self.push_pending() {
            self.push().await?;
        }
        self.ensure_remote_dir().await?;
        self.run_rsync(&self.pull_args(), "pull").await
    }

    async fn run_rsync(&self, args: &[String], direction: &str) -> Result<(), WorkspaceError> {
        let result = run_command(args, Duration::from_secs(3600)).await?;
        match result.exit {
            Some(0) => Ok(()),
            // rsync の 255 / 12 は ssh の失敗（接続の問題）。
            Some(255) | Some(12) => Err(WorkspaceError::Unreachable(format!(
                "rsync {direction} to {} failed: {}",
                self.settings.cluster,
                result.stderr_tail.trim()
            ))),
            other => Err(WorkspaceError::Remote(format!(
                "rsync {direction} to {} exited with {other:?}: {}",
                self.settings.cluster,
                result.stderr_tail.trim()
            ))),
        }
    }

    /// ワーカーがクラスタでコマンドを実行するためのラッパ `.celeris/remote-exec`（ADR-0018 D3、
    /// ADR-0059 D5 で `.taskd/remote-exec` から改名）。
    pub async fn write_remote_exec_helper(&self) -> Result<PathBuf, WorkspaceError> {
        // ADR-0059 D5: 改名前の古いラッパが写しに残っていたら消す（新しいものと混同しないため。
        // `.taskd/artifacts/<task_id>/` は ADR-0036 D1 の別の規約なので触らない）。
        let old = self.local.dir().join(".taskd").join("remote-exec");
        let _ = tokio::fs::remove_file(&old).await;
        let dir = self.local.dir().join(".celeris");
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("remote-exec");
        let ssh = self.ssh_base().join(" ");
        let remote = self.effective_remote_dir().to_string_lossy().to_string();
        let mut prefix = String::new();
        for (k, v) in &self.settings.env {
            prefix.push_str(&format!("export {k}={} && ", shq(v)));
        }
        for line in &self.settings.setup {
            prefix.push_str(&format!("{{ {line}; }} && "));
        }
        let script = format!(
            "#!/bin/sh\n\
             # celeris が run ごとに作るラッパ（ADR-0018 D3）。クラスタ {cluster} でコマンドを実行する。\n\
             # 使い方: .celeris/remote-exec <コマンド ...>\n\
             # ADR-0062 Phase 108 追記: システムの /etc/ssh/ssh_config（と Include 先の\n\
             # /etc/ssh/ssh_config.d/...）は読まない。codex サンドボックス（sandbox_mode=workspace-write）\n\
             # の中ではその Include 先が別所有者に見え、ssh が \"Bad owner or permissions\" で\n\
             # 拒否することがある（本番 2026-09-23、software-engineering の run）。ユーザーの\n\
             # ~/.ssh/config（Host 別名・ControlMaster を持つ）だけを -F で明示的に読む。\n\
             # 無ければ何も足さず ssh の既定の探索に任せる。\n\
             set -u\n\
             if [ $# -eq 0 ]; then echo \"usage: $0 <command...>\" >&2; exit 2; fi\n\
             cmd=\"$*\"\n\
             if [ -f \"$HOME/.ssh/config\" ]; then\n\
             \x20   set -- -F \"$HOME/.ssh/config\"\n\
             else\n\
             \x20   set --\n\
             fi\n\
             exec {ssh} \"$@\" {host} \"cd {remote_q} && {prefix}sh -c \\\"$cmd\\\"\"\n",
            cluster = self.settings.cluster,
            ssh = ssh,
            host = self.settings.host,
            remote_q = shell_remote_path(&remote),
            prefix = prefix,
        );
        tokio::fs::write(&path, script).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio::fs::metadata(&path).await?.permissions();
            perms.set_mode(0o755);
            tokio::fs::set_permissions(&path, perms).await?;
        }
        Ok(path)
    }
}

/// `.celeris/remote-exec` の指示文の共通の頭（worker と reviewer で共有する。ADR-0079 R5b-fix2）。
fn remote_exec_head(settings: &SshSettings) -> String {
    format!(
        "\n\n[celeris] このタスクの正はクラスタ `{cluster}`（ssh host `{host}`）の `{dir}` です。手元の作業ディレクトリはその写しで、",
        cluster = settings.cluster,
        host = settings.host,
        dir = settings.effective_remote_dir().to_string_lossy(),
    )
}

/// `.celeris/remote-exec` の使い方の一文（worker と reviewer で共有する）。
const REMOTE_EXEC_USAGE: &str = "`.celeris/remote-exec <コマンド ...>` で実行してください\
     （クラスタの作業ディレクトリで実行され、終了コードと出力がそのまま返ります）。";

/// ADR-0019 D1/D3: worktree のときに足す一文（worker と reviewer で共有する）。worktree でなければ `None`。
fn remote_worktree_note(settings: &SshSettings) -> Option<String> {
    (settings.sync == SyncMode::Worktree).then(|| {
        format!(
            "これは `{project}` から切り出した git worktree（ブランチ `{branch}`）です。追跡ファイルだけが入っているので、\
             手元に見えないファイル（未追跡の巨大データなど）はクラスタ側にあります。",
            project = settings.remote_dir.to_string_lossy(),
            branch = settings.worktree_branch(),
        )
    })
}

/// ワーカーへ渡す指示文（ADR-0018 D3）。`RunRequest.task.objective` の末尾に足し、`.celeris/remote-exec`
/// の存在と使い方を伝える（ADR-0059 D5 で `.taskd/remote-exec` から改名）。
/// ワーカーが従うかは保証しない（受け入れ条件はクラスタ側で判定されるので、手元だけで済ませた仕事は条件で落ちる）。
/// ADR-0079 R5b-fix2: 「run の後にクラスタへ同期され」は `SshWorkspace::push_after_run` が守る。
pub fn remote_exec_instructions(settings: &SshSettings) -> String {
    let base = format!(
        "{head}run の後にクラスタへ同期され、受け入れ条件のコマンドはクラスタ側で実行されます。\
         重い処理・クラスタ上のデータやモジュールを使う処理は {REMOTE_EXEC_USAGE}",
        head = remote_exec_head(settings),
    );
    let base = match remote_worktree_note(settings) {
        None => base,
        Some(note) => format!("{base}\n{note}元のリポジトリの作業ツリーは触らないでください。"),
    };
    format!("{base}\n{}", cluster_job_wait_instructions(settings))
}

/// ADR-0090 D5: 長いクラスタ job（PBS / Slurm）の扱い（worker の remote-exec の前置きの 1 段落）。
pub fn cluster_job_wait_instructions(settings: &SshSettings) -> String {
    format!(
        "長く走るクラスタ job（qsub / sbatch）は、投入したら job id を控えて、この run を \
         `artifacts/result.json` に `{{\"type\": \"wait\", \"kind\": \"cluster_job\", \"cluster\": \"{cluster}\", \
         \"scheduler\": \"pbs\", \"jobs\": [\"<job id>\", ...], \"checkpoint\": {{\"completed\": [...], \
         \"remaining\": [...], \"next_action\": \"...\"}}, \"summary\": \"...\"}}` を書いて終えてください\
         （`scheduler` は `pbs` か `slurm`、`poll_secs` / `timeout_secs` は任意）。celeris が job の終了を待ち、\
         終わったら job の最終状態と終了コードを前置きに入れた続きの run を起こします。job が Q / R の間に完了を申告しない\
         こと（`summary` だけの result.json は完了の申告です）。結果の回収・受け入れ条件の確認は続きの run で行います。\
         job の終了を poll して run を引き延ばさないこと。",
        cluster = settings.cluster,
    )
}

/// ADR-0079 R5b-fix2: reviewer run（最終レビュー）へ渡す指示文。worker と同じ頭・使い方の一文を使い、
/// 検査（git の状態・ビルド・テスト）をクラスタ側で行うよう伝える。手元の写しの `.git` はクラスタ側を
/// 指す gitfile なので、手元で `git status` すると `not a git repository` になる（本番 2026-09-29）。
pub fn remote_exec_reviewer_instructions(settings: &SshSettings) -> String {
    let base = format!(
        "{head}ワーカーの run の後にクラスタへ同期済みです。\
         `git status` / `git diff` / `git log` などの git の確認と、ビルド・テストなどの検査は手元ではなく \
         {REMOTE_EXEC_USAGE}\
         手元の写しで git を実行しても、クラスタ側の状態は分かりません（`not a git repository` になることがあります）。",
        head = remote_exec_head(settings),
    );
    match remote_worktree_note(settings) {
        None => base,
        Some(note) => format!("{base}\n{note}"),
    }
}

/// tick（同期の文脈）から呼ぶ、多重接続の有無の確認（ADR-0018 D2）。`ssh -O check` は unix ソケットを見るだけで即座に返る。
pub fn control_master_alive_blocking(ssh_command: &[String], host: &str) -> bool {
    let Some((program, rest)) = ssh_command.split_first() else {
        return false;
    };
    std::process::Command::new(program)
        .args(rest)
        .args(["-o", "BatchMode=yes", "-O", "check", host])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// ADR-0062 A（Phase 107）: master 越しの実通信で生存を確定させる（`ssh -o BatchMode=yes <host> -- true`）。
/// `control_master_alive_blocking`（`-O check`）は unix ソケットを見るだけなので、NAT / ファイアウォールの
/// idle timeout で TCP が黙って死んでいても「Master running」を返し続ける。こちらは実際にリモートへ
/// コマンドを 1 つ流すので確定できる。`timeout` を超えたら子を kill して `false`（`std::process::Command`
/// を使う同期の実装。tick ループからは `run_cluster_hooks_off_async` 経由で OS スレッドに逃がして呼ぶ）。
pub fn control_master_command_probe_blocking(
    ssh_command: &[String],
    host: &str,
    timeout: Duration,
) -> bool {
    let Some((program, rest)) = ssh_command.split_first() else {
        return false;
    };
    let mut child = match std::process::Command::new(program)
        .args(rest)
        .args(["-o", "BatchMode=yes", host, "--", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

/// ADR-0090 D2: master 越しに 1 つのコマンドを流した結果（stdout / stderr は先頭から最大
/// [`REMOTE_COMMAND_OUTPUT_MAX`] バイト。qstat の出力を丸ごと読むため末尾ではなく先頭を残す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCommandOutput {
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// [`run_remote_command_blocking`] が残す出力の上限（バイト）。
pub const REMOTE_COMMAND_OUTPUT_MAX: usize = 1024 * 1024;

/// ADR-0090 D2: tick から OS スレッドに逃がして呼ぶ、master 越しの 1 コマンド（`ssh -o BatchMode=yes <host> -- <script>`）。
/// `timeout` を超えたら子を kill して `Err`。ssh 自身の失敗（exit 255）も `Err`（接続の問題。poll の失敗として扱う）。
/// 対話的な認証はしない（`BatchMode=yes`。人が張った ControlMaster を借りるだけ）。
pub fn run_remote_command_blocking(
    ssh_command: &[String],
    host: &str,
    script: &str,
    timeout: Duration,
) -> Result<RemoteCommandOutput, String> {
    use std::io::Read;
    let Some((program, rest)) = ssh_command.split_first() else {
        return Err("empty ssh command".to_string());
    };
    let mut child = std::process::Command::new(program)
        .args(rest)
        .args(["-o", "BatchMode=yes", host, "--", script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start ssh: {e}"))?;
    fn drain(mut r: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let room = REMOTE_COMMAND_OUTPUT_MAX.saturating_sub(kept.len());
                        kept.extend_from_slice(&buf[..n.min(room)]);
                    }
                }
            }
            kept
        })
    }
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "ssh to {host} timed out after {}s",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("cannot wait for ssh: {e}")),
        }
    };
    let collect = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
        h.and_then(|h| h.join().ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    let stdout = collect(out);
    let stderr = collect(err);
    if status.code() == Some(255) {
        return Err(format!(
            "ssh to {host} failed (exit 255): {}",
            stderr.trim()
        ));
    }
    Ok(RemoteCommandOutput {
        exit: status.code(),
        stdout,
        stderr,
    })
}

/// 外部コマンドを 1 つ動かし、末尾の出力と終了コードを返す（`LocalWorkspace::exec` と同じ流儀）。
async fn run_command(args: &[String], timeout: Duration) -> Result<ExecResult, WorkspaceError> {
    let Some((program, rest)) = args.split_first() else {
        return Err(WorkspaceError::Remote("empty command".to_string()));
    };
    let mut command = tokio::process::Command::new(program);
    command.args(rest);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);
    let child = command.output();
    match tokio::time::timeout(timeout, child).await {
        Ok(out) => {
            let out = out?;
            Ok(ExecResult {
                exit: out.status.code(),
                stdout_tail: crate::workspace::tail_utf8_lossy(&out.stdout),
                stderr_tail: crate::workspace::tail_utf8_lossy(&out.stderr),
                timed_out: false,
            })
        }
        Err(_) => Ok(ExecResult {
            exit: None,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            timed_out: true,
        }),
    }
}

#[async_trait]
impl Workspace for SshWorkspace {
    /// **クラスタ側が正**（ADR-0018 D1）。手元の写しを用意し、クラスタの内容を取り込んでから、
    /// run に必要なディレクトリを作る。既存プロジェクトを指していても壊さない。
    async fn prepare(&self, task: &Task) -> Result<PathBuf, WorkspaceError> {
        tokio::fs::create_dir_all(self.local.dir()).await?;
        self.pull().await?;
        self.local.prepare(task).await
    }

    /// コマンドはクラスタで実行する（ADR-0018 D1）。前後に同期して、手元の編集が反映され、
    /// リモートで生まれたファイルが手元に戻るようにする。
    async fn exec(&self, cmd: &str, timeout: Duration) -> Result<ExecResult, WorkspaceError> {
        if self.settings.sync_around_exec {
            self.push().await?;
        } else if self.settings.sync == SyncMode::Worktree {
            // push を挟まない設定でも worktree だけは用意する（無ければ `cd` で落ちる）。
            self.ensure_remote_dir().await?;
        }
        let script = self.remote_script(cmd, timeout);
        // ssh 自体のタイムアウトは、リモートの timeout より少し長くする。
        let result = self
            .run_ssh(&script, timeout + Duration::from_secs(30))
            .await?;
        if self.settings.sync_around_exec {
            self.pull().await?;
        }
        Ok(result)
    }

    /// リモートの結果を取り込んでから、ローカルで sha256 を計算する（真実はローカル。ADR-0018 D4）。
    async fn collect(&self, task: &Task) -> Result<Vec<ArtifactRef>, WorkspaceError> {
        self.pull().await?;
        self.local.collect(task).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ADR-0059 D2: `~` の展開（純粋関数） ----

    #[test]
    fn shell_remote_path_expands_only_a_leading_tilde() {
        assert_eq!(shell_remote_path("~"), "\"$HOME\"");
        assert_eq!(
            shell_remote_path("~/work/project"),
            "\"$HOME\"'/work/project'"
        );
        // `~user` は展開しない（celeris は他ユーザの home を知らない。ADR-0039 D5 と同じ方針）。
        assert_eq!(shell_remote_path("~user/x"), "'~user/x'");
        // それ以外は従来どおり `shq`。
        assert_eq!(shell_remote_path("/work/x"), "'/work/x'");
        assert_eq!(shell_remote_path("relative/x"), "'relative/x'");
        assert_eq!(shell_remote_path("it's/quoted"), "'it'\\''s/quoted'");
    }

    // ---- ADR-0059 D6: `work_dir` からの相対解決（純粋関数） ----

    #[test]
    fn resolve_remote_dir_keeps_absolute_and_tilde_paths_untouched() {
        let work_dir = Some(Path::new("/work/NBB/rmaeda"));
        assert_eq!(
            resolve_remote_dir(Path::new("/scratch/x"), work_dir),
            Some(PathBuf::from("/scratch/x"))
        );
        assert_eq!(
            resolve_remote_dir(Path::new("~"), work_dir),
            Some(PathBuf::from("~"))
        );
        assert_eq!(
            resolve_remote_dir(Path::new("~/work"), work_dir),
            Some(PathBuf::from("~/work"))
        );
        // `work_dir` が無くても絶対・`~` はそのまま解決できる。
        assert_eq!(
            resolve_remote_dir(Path::new("/scratch/x"), None),
            Some(PathBuf::from("/scratch/x"))
        );
    }

    #[test]
    fn resolve_remote_dir_resolves_empty_and_relative_paths_against_work_dir() {
        let work_dir = Some(Path::new("/work/NBB/rmaeda"));
        assert_eq!(
            resolve_remote_dir(Path::new(""), work_dir),
            Some(PathBuf::from("/work/NBB/rmaeda"))
        );
        assert_eq!(
            resolve_remote_dir(Path::new("benchfs"), work_dir),
            Some(PathBuf::from("/work/NBB/rmaeda/benchfs"))
        );
    }

    #[test]
    fn resolve_remote_dir_is_none_when_there_is_no_work_dir_to_resolve_against() {
        assert_eq!(resolve_remote_dir(Path::new(""), None), None);
        assert_eq!(resolve_remote_dir(Path::new("benchfs"), None), None);
    }

    #[test]
    fn remote_dir_is_resolved_matches_absolute_and_tilde_only() {
        assert!(remote_dir_is_resolved(Path::new("/work/x")));
        assert!(remote_dir_is_resolved(Path::new("~")));
        assert!(remote_dir_is_resolved(Path::new("~/work")));
        assert!(!remote_dir_is_resolved(Path::new("")));
        assert!(!remote_dir_is_resolved(Path::new("relative")));
    }

    // ---- ADR-0018 D5 / ADR-0059: `SyncMode::None` は同期を一切しない ----

    #[tokio::test]
    async fn sync_none_push_and_pull_never_run_rsync_or_ssh() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = SshSettings::new("c", "h", PathBuf::from("/work/x"));
        settings.sync = SyncMode::None;
        // 呼ばれたら即座にエラーで分かるように、実在しないプログラムを指す。
        settings.ssh_command = vec!["/nonexistent/ssh-should-not-run".into()];
        settings.rsync_command = vec!["/nonexistent/rsync-should-not-run".into()];
        let ws = SshWorkspace::new(dir.path(), settings);
        ws.push()
            .await
            .expect("push is a no-op under SyncMode::None");
        ws.pull()
            .await
            .expect("pull is a no-op under SyncMode::None");
    }

    // ---- ADR-0059 D3: worktree 準備の exit 65 を型で区別する ----

    #[tokio::test]
    async fn ensure_worktree_maps_exit_65_to_not_a_git_repository_and_exit_66_to_remote() {
        let dir = tempfile::tempdir().unwrap();
        for (exit_code, matches_not_a_git_repo) in [(65, true), (66, false), (1, false)] {
            let stub = dir.path().join(format!("stub-{exit_code}.sh"));
            std::fs::write(&stub, format!("#!/bin/sh\nexit {exit_code}\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&stub).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&stub, perms).unwrap();
            }
            let mut settings = SshSettings::new("c", "h", PathBuf::from("/work/proj"));
            settings.sync = SyncMode::Worktree;
            settings.task_id = "01TESTTASK".into();
            settings.ssh_command = vec![stub.to_string_lossy().into_owned()];
            let mirror = dir.path().join(format!("mirror-{exit_code}"));
            let ws = SshWorkspace::new(&mirror, settings);
            let err = ws.ensure_worktree().await.expect_err("stub always fails");
            assert_eq!(
                matches!(err, WorkspaceError::NotAGitRepository(_)),
                matches_not_a_git_repo,
                "exit {exit_code}: {err:?}"
            );
        }
    }

    // ---- ADR-0059 D5: `.taskd/remote-exec` -> `.celeris/remote-exec` ----

    #[tokio::test]
    async fn write_remote_exec_helper_writes_celeris_and_cleans_up_the_old_taskd_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let mirror = dir.path().join("mirror");
        tokio::fs::create_dir_all(mirror.join(".taskd"))
            .await
            .unwrap();
        tokio::fs::write(mirror.join(".taskd").join("remote-exec"), "old wrapper")
            .await
            .unwrap();

        let mut settings = SshSettings::new("pegasus", "pegasus", PathBuf::from("~/work/proj"));
        settings.sync = SyncMode::None;
        let ws = SshWorkspace::new(&mirror, settings);
        let path = ws.write_remote_exec_helper().await.expect("write helper");
        assert_eq!(path, mirror.join(".celeris").join("remote-exec"));
        assert!(path.is_file());
        assert!(
            !mirror.join(".taskd").join("remote-exec").exists(),
            "the old wrapper must be removed"
        );

        // ADR-0059 D2: `~/work/proj` は `.celeris/remote-exec` の生成スクリプトの中で `"$HOME"` に展開される。
        let script = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(script.contains("\"$HOME\"'/work/proj'"), "{script}");
        // ADR-0062 Phase 108: `$HOME/.ssh/config` があるときだけ `-F` を足す条件分岐を持つ。
        assert!(
            script.contains(r#"if [ -f "$HOME/.ssh/config" ]; then"#),
            "{script}"
        );
        assert!(script.contains(r#"-F "$HOME/.ssh/config""#), "{script}");
    }

    /// ADR-0062 Phase 108: 生成した `.celeris/remote-exec` は実行時に `$HOME/.ssh/config` があれば
    /// `-F` で明示的に読み、無ければ何も足さない（システムの `/etc/ssh/ssh_config` に触れない）。
    /// codex サンドボックス内で `/etc/ssh/ssh_config.d/...` の Include 先が拒否される問題への対応
    /// （本番 2026-09-23）。
    #[tokio::test]
    async fn the_wrapper_only_adds_dash_f_when_the_users_ssh_config_exists() {
        let dir = tempfile::tempdir().unwrap();
        let mirror = dir.path().join("mirror");
        tokio::fs::create_dir_all(&mirror).await.unwrap();

        // 引数をそのままファイルに記録するだけの偽 ssh。
        let log = dir.path().join("argv.log");
        let fake_ssh = dir.path().join("ssh");
        crate::test_support::write_executable(
            &fake_ssh,
            &format!("#!/bin/sh\necho \"$@\" > {log:?}\nexit 0\n"),
        );

        let mut settings = SshSettings::new("pegasus", "pegasus", PathBuf::from("/work/proj"));
        settings.sync = SyncMode::None;
        settings.ssh_command = vec![fake_ssh.to_string_lossy().into_owned()];
        let ws = SshWorkspace::new(&mirror, settings);
        let path = ws.write_remote_exec_helper().await.expect("write helper");

        // `$HOME/.ssh/config` が無ければ `-F` を付けない。
        let home_without = dir.path().join("home-without");
        tokio::fs::create_dir_all(&home_without).await.unwrap();
        let status = tokio::process::Command::new(&path)
            .arg("true")
            .env("HOME", &home_without)
            .status()
            .await
            .expect("run wrapper");
        assert!(status.success());
        let argv = tokio::fs::read_to_string(&log).await.unwrap();
        assert!(!argv.contains("-F"), "{argv}");

        // `$HOME/.ssh/config` があれば `-F "$HOME/.ssh/config"` を付ける。
        let home_with = dir.path().join("home-with");
        tokio::fs::create_dir_all(home_with.join(".ssh"))
            .await
            .unwrap();
        tokio::fs::write(home_with.join(".ssh").join("config"), "Host pegasus\n")
            .await
            .unwrap();
        let status = tokio::process::Command::new(&path)
            .arg("true")
            .env("HOME", &home_with)
            .status()
            .await
            .expect("run wrapper");
        assert!(status.success());
        let argv = tokio::fs::read_to_string(&log).await.unwrap();
        assert!(
            argv.contains(&format!("-F {}/.ssh/config", home_with.display())),
            "{argv}"
        );
    }

    // ---- ADR-0018 D3 / ADR-0059 D5: `.celeris/` は同期から常に除外し、`.taskd/` も後方互換で残す ----

    #[test]
    fn sync_always_excluded_keeps_the_legacy_taskd_alongside_celeris() {
        assert!(SYNC_ALWAYS_EXCLUDED.contains(&".taskd/"));
        assert!(SYNC_ALWAYS_EXCLUDED.contains(&".celeris/"));
    }

    // ---- ADR-0079 R5b-fix2: run 後の push・成果物を pull で消さない・reviewer への指示 ----

    #[test]
    fn pull_protects_artifacts_and_excludes_the_admin_dirs_while_push_still_sends_artifacts() {
        assert!(SYNC_PULL_PROTECTED.contains(&"artifacts/"));
        assert!(SYNC_ALWAYS_EXCLUDED.contains(&".taskd/"));
        let ws = SshWorkspace::new("/tmp/mirror", SshSettings::new("c", "h", "/work/x"));
        let pull = ws.pull_args();
        assert!(pull.contains(&"--delete".to_string()), "{pull:?}");
        assert!(
            pull.contains(&"--filter=P artifacts/".to_string()),
            "{pull:?}"
        );
        for dir in [".taskd/", ".celeris/", "runs/", "inputs/"] {
            assert!(
                pull.windows(2).any(|w| w[0] == "--exclude" && w[1] == dir),
                "pull must exclude {dir}: {pull:?}"
            );
        }
        let push = ws.push_args();
        assert!(!push.contains(&"--delete".to_string()), "{push:?}");
        assert!(
            !push
                .windows(2)
                .any(|w| w[0] == "--exclude" && w[1] == "artifacts/"),
            "artifacts are pushed (cluster-side checks may read them): {push:?}"
        );
        // push は手元 → クラスタ、pull はクラスタ → 手元。
        assert_eq!(push.last().map(String::as_str), Some("h:/work/x/"));
        assert_eq!(pull.last().map(String::as_str), Some("/tmp/mirror/"));
    }

    /// ssh の代わりに、受け取ったリモートコマンドを手元の `sh` で実行するだけの偽物（外部に出ない）。
    /// `ssh -o BatchMode=yes <host> <cmd...>` の形で呼ばれる（rsync の `-e` からも、`run_ssh` からも）。
    fn local_ssh(dir: &Path) -> Vec<String> {
        let path = dir.join("fake-ssh");
        crate::test_support::write_executable(
            &path,
            "#!/bin/sh\nwhile [ \"$1\" = \"-o\" ]; do shift 2; done\nshift\nexec sh -c \"$*\"\n",
        );
        vec![path.to_string_lossy().into_owned()]
    }

    /// 本物の rsync を包み、呼ばれた向き（最後の引数が手元なら pull）を記録する。`fail` なら rsync を呼ばずに 23 で落ちる。
    fn logging_rsync(dir: &Path, name: &str, local: &Path, log: &Path, fail: bool) -> Vec<String> {
        let path = dir.join(name);
        let run = if fail {
            "exit 23".to_string()
        } else {
            "exec rsync \"$@\"".to_string()
        };
        crate::test_support::write_executable(
            &path,
            &format!(
                "#!/bin/sh\nfor a; do last=$a; done\nif [ \"$last\" = {local:?} ]; then echo pull >> {log:?}; else echo push >> {log:?}; fi\n{run}\n",
                local = format!("{}/", local.display()),
            ),
        );
        vec![path.to_string_lossy().into_owned()]
    }

    fn log_lines(log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn rsync_available() -> bool {
        std::process::Command::new("rsync")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// 本番 2026-09-29 の再現: reviewer の検査しか無い task は `exec` で push しないので、次の run の
    /// prepare（`--delete` 付きの pull）が worker の編集と成果物を消していた。run 後の push（1 回）で
    /// 編集はクラスタに届き、成果物は pull でも消えない。
    #[tokio::test]
    async fn push_after_run_runs_once_and_the_next_pull_keeps_edits_and_artifacts() {
        if !rsync_available() {
            eprintln!("rsync is not installed; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote");
        let local = tmp.path().join("mirror");
        std::fs::create_dir_all(remote.join("src")).unwrap();
        std::fs::write(remote.join("src/lib.rs"), "old\n").unwrap();
        let log = tmp.path().join("rsync.log");
        let mut settings = SshSettings::new("c", "h", &remote);
        settings.ssh_command = local_ssh(tmp.path());
        settings.rsync_command = logging_rsync(tmp.path(), "rsync-ok", &local, &log, false);
        let ws = SshWorkspace::new(&local, settings);

        // run の前（prepare）: クラスタの内容を取り込む。
        ws.pull().await.expect("initial pull");
        assert_eq!(
            std::fs::read_to_string(local.join("src/lib.rs")).unwrap(),
            "old\n"
        );
        // worker の run: ソースを直し、成果物を書く（クラスタ側には無い）。
        std::fs::write(local.join("src/lib.rs"), "edited\n").unwrap();
        std::fs::create_dir_all(local.join("artifacts")).unwrap();
        std::fs::write(local.join("artifacts/experiment-plan.md"), "plan\n").unwrap();
        // run の後: push はちょうど 1 回。
        ws.push_after_run().await.expect("push after run");
        assert_eq!(log_lines(&log), vec!["pull", "push"]);
        assert!(!ws.push_pending(), "a successful push clears the marker");
        assert_eq!(
            std::fs::read_to_string(remote.join("src/lib.rs")).unwrap(),
            "edited\n"
        );

        // クラスタ側から成果物が消えても（人が片付けた等）、次の pull は手元の成果物を消さない。
        std::fs::remove_dir_all(remote.join("artifacts")).unwrap();
        ws.pull().await.expect("next prepare pull");
        assert_eq!(log_lines(&log), vec!["pull", "push", "pull"]);
        assert_eq!(
            std::fs::read_to_string(local.join("src/lib.rs")).unwrap(),
            "edited\n"
        );
        assert_eq!(
            std::fs::read_to_string(local.join("artifacts/experiment-plan.md")).unwrap(),
            "plan\n"
        );
    }

    /// push が落ちたら印が残り、エラーとして返る（黙らない）。印がある間の pull は `--delete` の前に push を
    /// やり直し、それも落ちたら pull しない（手元の編集を消さない）。繋がれば push → pull の順で進む。
    #[tokio::test]
    async fn failed_push_is_reported_and_blocks_the_deleting_pull_until_a_push_succeeds() {
        if !rsync_available() {
            eprintln!("rsync is not installed; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote");
        let local = tmp.path().join("mirror");
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("new.rs"), "worker edit\n").unwrap();
        let log = tmp.path().join("rsync.log");
        let mut failing = SshSettings::new("c", "h", &remote);
        failing.ssh_command = local_ssh(tmp.path());
        failing.rsync_command = logging_rsync(tmp.path(), "rsync-fail", &local, &log, true);
        let ws = SshWorkspace::new(&local, failing.clone());

        let err = ws.push_after_run().await.expect_err("push fails");
        assert!(matches!(err, WorkspaceError::Remote(_)), "{err:?}");
        assert!(ws.push_pending());
        ws.pull()
            .await
            .expect_err("pull must not run while the push is pending");
        assert_eq!(
            log_lines(&log),
            vec!["push", "push"],
            "no pull was attempted"
        );
        assert!(local.join("new.rs").is_file(), "the local edit survives");

        let mut ok = failing;
        ok.rsync_command = logging_rsync(tmp.path(), "rsync-ok", &local, &log, false);
        let ws = SshWorkspace::new(&local, ok);
        ws.pull().await.expect("push then pull");
        assert_eq!(log_lines(&log), vec!["push", "push", "push", "pull"]);
        assert!(!ws.push_pending());
        assert_eq!(
            std::fs::read_to_string(remote.join("new.rs")).unwrap(),
            "worker edit\n"
        );
        assert!(local.join("new.rs").is_file());
    }

    #[tokio::test]
    async fn push_after_run_is_a_no_op_under_sync_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = SshSettings::new("c", "h", PathBuf::from("/work/x"));
        settings.sync = SyncMode::None;
        settings.ssh_command = vec!["/nonexistent/ssh-should-not-run".into()];
        settings.rsync_command = vec!["/nonexistent/rsync-should-not-run".into()];
        let ws = SshWorkspace::new(dir.path(), settings);
        ws.push_after_run().await.expect("no-op");
        assert!(!ws.push_pending());
    }

    #[test]
    fn worker_and_reviewer_instructions_share_the_remote_exec_usage() {
        let mut settings = SshSettings::new(
            "sirius",
            "sirius",
            "/work/NBB/rmaeda/workspace/rust/benchfs",
        );
        settings.sync = SyncMode::Worktree;
        settings.task_id = "01TASK".into();
        let worker = remote_exec_instructions(&settings);
        let reviewer = remote_exec_reviewer_instructions(&settings);
        for text in [&worker, &reviewer] {
            assert!(text.contains(REMOTE_EXEC_USAGE), "{text}");
            assert!(text.contains(&remote_exec_head(&settings)), "{text}");
            assert!(text.contains("ブランチ `celeris/01TASK`"), "{text}");
        }
        assert!(worker.contains("run の後にクラスタへ同期され"));
        // ADR-0090 D5: worker の指示文の最後はクラスタ job の wait の段落（reviewer には無い）。
        assert!(
            worker
                .contains("元のリポジトリの作業ツリーは触らないでください。\n長く走るクラスタ job")
        );
        assert!(worker.ends_with(&cluster_job_wait_instructions(&settings)));
        assert!(
            worker
                .contains("\"type\": \"wait\", \"kind\": \"cluster_job\", \"cluster\": \"sirius\"")
        );
        assert!(worker.contains("job が Q / R の間に完了を申告しない"));
        assert!(!reviewer.contains("長く走るクラスタ job"));
        assert!(reviewer.contains("`git status`"));
        assert!(!reviewer.contains("元のリポジトリの作業ツリーは触らないでください"));
    }

    /// ADR-0090 D2: master 越しの 1 コマンドは stdout / stderr を先頭から丸ごと返し、exit 255 は接続の失敗。
    #[test]
    fn remote_command_returns_the_whole_output_and_treats_255_as_a_connection_failure() {
        let dir = tempfile::tempdir().unwrap();
        let ok = fake_ssh_probe(
            dir.path(),
            "#!/bin/sh\nprintf 'Job Id: 1.pbs\\n    job_state = F\\n'\necho 'qstat: Unknown Job Id 2.pbs' >&2\nexit 153\n",
        );
        let out =
            run_remote_command_blocking(&ok, "sirius", "qstat -xf 1 2", Duration::from_secs(10))
                .expect("ran");
        assert_eq!(out.exit, Some(153));
        assert!(out.stdout.contains("job_state = F"), "{out:?}");
        assert!(out.stderr.contains("Unknown Job Id 2.pbs"), "{out:?}");
        let dir = tempfile::tempdir().unwrap();
        let down = fake_ssh_probe(
            dir.path(),
            "#!/bin/sh\necho 'mux_client: no master' >&2\nexit 255\n",
        );
        let err =
            run_remote_command_blocking(&down, "sirius", "qstat -xf 1", Duration::from_secs(10))
                .expect_err("255 is a connection failure");
        assert!(err.contains("exit 255"), "{err}");
    }

    // ---- ADR-0062 A（Phase 107）: 実通信 probe（`ssh -o BatchMode=yes <host> -- true`） ----

    fn fake_ssh_probe(dir: &Path, script: &str) -> Vec<String> {
        let path = dir.join("ssh");
        crate::test_support::write_executable(&path, script);
        vec![path.to_string_lossy().into_owned()]
    }

    #[test]
    fn command_probe_succeeds_when_the_remote_command_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = fake_ssh_probe(dir.path(), "#!/bin/sh\nexit 0\n");
        assert!(control_master_command_probe_blocking(
            &ssh,
            "cluster-host",
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn command_probe_fails_when_the_remote_command_exits_nonzero() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = fake_ssh_probe(dir.path(), "#!/bin/sh\nexit 255\n");
        assert!(!control_master_command_probe_blocking(
            &ssh,
            "cluster-host",
            Duration::from_secs(2)
        ));
    }

    /// NAT の idle timeout で TCP が黙って死んだ状況を模す: ssh が応答せずハングし続ける。
    /// `timeout` を超えたら kill されて `false` になる（ハングしたまま残らない）。
    #[test]
    fn command_probe_times_out_and_kills_a_hanging_ssh() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let script = format!(
            "#!/bin/sh\necho $$ > {state:?}/pid\nwhile true; do sleep 3600; done\n",
            state = state.path()
        );
        let ssh = fake_ssh_probe(dir.path(), &script);
        let start = std::time::Instant::now();
        assert!(!control_master_command_probe_blocking(
            &ssh,
            "cluster-host",
            Duration::from_millis(300)
        ));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "probe must not block past the timeout"
        );
        let pid: u32 = std::fs::read_to_string(state.path().join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        for _ in 0..150 {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("hanging ssh process {pid} was not killed");
    }

    // ---- Phase R6-3: クラスタの worktree は git submodule を初期化する ----

    /// テスト用の git（人の設定・対話的な認証に引きずられない。ローカルパスの submodule を許す）。
    fn test_git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "protocol.file.allow=always",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `sub`（1 ファイル）を submodule `lib/sub` に持つ `proj` を作る。`with_submodule = false` なら submodule 無し。
    fn project_repo(root: &Path, with_submodule: bool) -> PathBuf {
        let sub = root.join("sub");
        let proj = root.join("proj");
        for dir in [&sub, &proj] {
            std::fs::create_dir_all(dir).unwrap();
            test_git(dir, &["init", "-q", "-b", "main"]);
        }
        std::fs::write(sub.join("lib.rs"), "pub fn sub() {}\n").unwrap();
        test_git(&sub, &["add", "-A"]);
        test_git(&sub, &["commit", "-q", "-m", "sub"]);
        std::fs::write(proj.join("README.md"), "proj\n").unwrap();
        test_git(&proj, &["add", "-A"]);
        if with_submodule {
            let url = sub.to_string_lossy().into_owned();
            test_git(&proj, &["submodule", "add", "-q", &url, "lib/sub"]);
        }
        test_git(&proj, &["commit", "-q", "-m", "proj"]);
        proj
    }

    /// `local_ssh` と同じ偽 ssh だが、ローカルパスの submodule の clone を許す（git ≥ 2.38.1 の既定は拒否）。
    fn local_ssh_allowing_file_protocol(dir: &Path) -> Vec<String> {
        let path = dir.join("fake-ssh-git");
        crate::test_support::write_executable(
            &path,
            "#!/bin/sh\nwhile [ \"$1\" = \"-o\" ]; do shift 2; done\nshift\n\
             export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=protocol.file.allow GIT_CONFIG_VALUE_0=always GIT_TERMINAL_PROMPT=0\n\
             exec sh -c \"$*\"\n",
        );
        vec![path.to_string_lossy().into_owned()]
    }

    fn worktree_workspace(tmp: &Path, proj: &Path, ssh: Vec<String>) -> SshWorkspace {
        let mut settings = SshSettings::new("sirius", "h", proj);
        settings.sync = SyncMode::Worktree;
        settings.task_id = "01R63TASK".into();
        settings.ssh_command = ssh;
        SshWorkspace::new(tmp.join("mirror"), settings)
    }

    /// 本番 2026-09-29（task 01M3Q25DSD895DGMGPWD752G3G、sirius の BenchFS）: `git worktree add` の直後は
    /// submodule のディレクトリが空だった。準備で `submodule update --init --recursive` し、進行を 1 行残す。
    /// 2 回目（再利用）は落ちず、初期化済みなので何もしない（行も増えない）。
    #[tokio::test]
    async fn ensure_worktree_initialises_submodules_and_is_idempotent_on_reuse() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project_repo(tmp.path(), true);
        let ws = worktree_workspace(
            tmp.path(),
            &proj,
            local_ssh_allowing_file_protocol(tmp.path()),
        );
        let wt = ws.effective_remote_dir();

        ws.ensure_worktree().await.expect("first prepare");
        assert_eq!(
            std::fs::read_to_string(wt.join("lib/sub/lib.rs")).unwrap(),
            "pub fn sub() {}\n",
            "the submodule is populated in the fresh worktree"
        );
        let notes = ws.take_progress_notes();
        assert_eq!(
            notes,
            vec![format!(
                "initialised 1 submodules in {} on cluster sirius",
                wt.display()
            )]
        );

        ws.ensure_worktree().await.expect("reuse does not fail");
        assert!(wt.join("lib/sub/lib.rs").is_file());
        assert!(
            ws.take_progress_notes().is_empty(),
            "already initialised: nothing to do on reuse"
        );

        // 人が submodule を deinit しても（空のディレクトリに戻る）、次の準備で戻る。
        test_git(&wt, &["submodule", "deinit", "-q", "--all", "--force"]);
        assert!(!wt.join("lib/sub/lib.rs").exists());
        ws.ensure_worktree().await.expect("re-init on reuse");
        assert!(wt.join("lib/sub/lib.rs").is_file());
        assert_eq!(ws.take_progress_notes().len(), 1);
    }

    #[tokio::test]
    async fn ensure_worktree_skips_the_submodule_step_without_gitmodules() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project_repo(tmp.path(), false);
        let ws = worktree_workspace(tmp.path(), &proj, local_ssh(tmp.path()));
        ws.ensure_worktree().await.expect("prepare");
        let wt = ws.effective_remote_dir();
        assert!(wt.join("README.md").is_file());
        assert!(!wt.join(".gitmodules").exists());
        assert!(ws.take_progress_notes().is_empty());
    }

    /// Phase R7-4: submodule の初期化に失敗しても（ここでは submodule の元を消した）、準備は通る。
    /// 失敗は submodule の path とクラスタ・worktree を名指しした警告の進行の行になる（黙らない）。
    #[tokio::test]
    async fn a_failed_submodule_init_is_a_warning_note_not_a_prepare_error() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project_repo(tmp.path(), true);
        std::fs::remove_dir_all(tmp.path().join("sub")).unwrap();
        let ws = worktree_workspace(
            tmp.path(),
            &proj,
            local_ssh_allowing_file_protocol(tmp.path()),
        );
        ws.ensure_worktree()
            .await
            .expect("a broken submodule does not stop the prepare");
        let wt = ws.effective_remote_dir();
        assert!(
            wt.join("README.md").is_file(),
            "the worktree itself is fine"
        );
        let notes = ws.take_progress_notes();
        assert_eq!(notes.len(), 1, "{notes:?}");
        let note = &notes[0];
        assert!(
            note.starts_with("submodule lib/sub could not be initialised: "),
            "{note}"
        );
        assert!(note.contains("sirius"), "{note}");
        assert!(note.contains(&wt.to_string_lossy().into_owned()), "{note}");
    }

    /// 本番 2026-09-30（task 01M3PAZ4XG4QN1T8S98VNA6ABV、sirius の BenchFS）: `ior_integration/ior` の固定 commit が
    /// remote に無く（`not our ref`）、`submodule update --init --recursive` 1 回の失敗で準備ごと落ちた。
    /// R7-4: submodule は 1 つずつ初期化する。良い方は入り、悪い方は `fatal:` の行つきの警告になる。
    #[tokio::test]
    async fn one_unfetchable_submodule_does_not_stop_the_other_or_the_prepare() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = project_repo(tmp.path(), true);
        // 2 つ目の submodule `ior`: remote に無い commit（submodule の中だけの commit）を固定する。
        let bad = tmp.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        test_git(&bad, &["init", "-q", "-b", "main"]);
        std::fs::write(bad.join("ior.c"), "int main(){}\n").unwrap();
        test_git(&bad, &["add", "-A"]);
        test_git(&bad, &["commit", "-q", "-m", "ior"]);
        let url = bad.to_string_lossy().into_owned();
        test_git(&proj, &["submodule", "add", "-q", &url, "ior"]);
        std::fs::write(proj.join("ior/local.c"), "// unpushed\n").unwrap();
        test_git(&proj.join("ior"), &["add", "-A"]);
        test_git(&proj.join("ior"), &["commit", "-q", "-m", "local only"]);
        test_git(&proj, &["add", "-A"]);
        test_git(&proj, &["commit", "-q", "-m", "pin an unpushed ior commit"]);

        let ws = worktree_workspace(
            tmp.path(),
            &proj,
            local_ssh_allowing_file_protocol(tmp.path()),
        );
        let wt = ws.effective_remote_dir();
        ws.ensure_worktree()
            .await
            .expect("one broken submodule does not stop the prepare");
        assert_eq!(
            std::fs::read_to_string(wt.join("lib/sub/lib.rs")).unwrap(),
            "pub fn sub() {}\n",
            "the good submodule is populated"
        );
        assert!(!wt.join("ior/local.c").exists());
        let notes = ws.take_progress_notes();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(
            notes[0],
            format!(
                "initialised 1 submodules in {} on cluster sirius",
                wt.display()
            )
        );
        assert!(
            notes[1].starts_with("submodule ior could not be initialised: fatal:"),
            "{}",
            notes[1]
        );
        assert!(notes[1].contains("sirius"), "{}", notes[1]);

        // 再利用でも落ちない。失敗した ior は clone までは済んで `submodule status` の行頭が `-` でなくなるので、
        // 再利用では試し直さない（R6-3 と同じく、`-` の無い worktree には触らない = 人・agent の submodule の
        // commit を巻き戻さない）。警告は最初の準備の 1 回だけ。
        ws.ensure_worktree().await.expect("reuse");
        assert!(wt.join("lib/sub/lib.rs").is_file());
        assert!(ws.take_progress_notes().is_empty());
    }
}
