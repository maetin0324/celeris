//! コンテナ実行（ADR-0043 D3。Phase 56 = A3）。
//!
//! リポジトリの `run` が `container`（または `auto` で `workspace.toml` の `[run] mode = "container"`）の
//! とき、**そのタスクのワーカー（ハーネスの CLI そのもの）をコンテナの中で起こす**。やることは 1 つだけで、
//! アダプタが組み立てた `(program, args, env, cwd)` を
//!
//! ```text
//! <runtime> run --rm -i --network host {--userns=keep-id | --user <uid>:<gid>} -w <cwd> \
//!   -v <task_dir>:<task_dir> [-v <dir リポジトリの実体>:<同じパス>] [-v <認証情報>:<同じパス>:ro] \
//!   [--env K=V …] [workspace.toml の mounts] --label celeris.task=<task_id> <image> <program> <args…>
//! ```
//!
//! に**包む**（ハーネスの stdio 契約はそのまま使えるので、アダプタごとの変更は無い）。
//!
//! ## 差し込み点（ADR-0043 D3「差し込み点は 1 か所」）
//!
//! 包むのは [`wrap`] の 1 関数だけで、呼ぶ場所は「`Command` を組み立て終えて stdio を付ける直前」である:
//!
//! | ファイル | 関数 | 何を包むか |
//! |---|---|---|
//! | `subprocess.rs` | `run_subprocess` | `fake` など `SubprocessSpec` のワーカー |
//! | `claude_code.rs` | `run_claude_code` | `claude` CLI |
//! | `codex.rs` | `run_codex` | `codex` CLI |
//! | `acp.rs` | `run_acp` | `opencode acp` |
//! | `workspace.rs` | `LocalWorkspace::exec` | `[commands] setup`（ADR-0043 D3「`setup` はその実行環境で走る」） |
//!
//! `paperqa` / `local-deep-research` は**コンテナに入れない**（[`HOST_ONLY_ADAPTERS`]）。道具立てが
//! ホストの venv（`uv` で作った `.venv`、`PAPERQA_*` / LDR の設定）に生えていて、コンテナに持ち込むと
//! 別物になるためである。[`decide`] がこの 2 つを弾く。
//!
//! ## ここに無いもの
//!
//! 判断（どのタスクをコンテナで走らせるか）は [`decide`] という**純粋関数**にあり、それを呼ぶのは
//! ディスパッチャである。LLM 呼び出しはもちろん無い（DESIGN 原則 1）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use task_core::repos::RepoRun;
use task_core::workspace_config::{RunMode, WorkspaceConfig};

/// `--label celeris.task=<task_id>`（後片付け用。ADR-0043 D3）。
pub const TASK_LABEL: &str = "celeris.task";

/// `[containers] image_default` の既定（`deploy/containers/celeris-worker/Dockerfile`）。
pub const DEFAULT_IMAGE: &str = "celeris-worker:latest";

/// `[container] dockerfile` の既定の場所（リポジトリ相対。ADR-0042 D2）。
pub const DEFAULT_DOCKERFILE: &str = ".config/celeris/Dockerfile";

/// `[containers] build_timeout_secs` の既定（30 分）。
pub const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 1800;

/// `workspace.toml` から作ったイメージのタグの接頭辞（`celeris-ws-<sha12>`）。
pub const BUILT_IMAGE_PREFIX: &str = "celeris-ws-";

/// ビルドの記録（タスクの `<task_dir>/` からの相対）。
pub const BUILD_LOG: &str = "runs/container-build.log";

/// **コンテナに入れないアダプタ**（ADR-0043 Phase 56 追記）。道具立てがホストの venv にあるため。
pub const HOST_ONLY_ADAPTERS: [&str; 2] = ["paperqa", "local-deep-research"];

/// 認証情報の置き場を指しているとみなす環境変数（値のパスを**読み取り専用**で同じ場所にマウントする）。
/// `OPENCODE_CONFIG` だけはファイルを指すので、その**親ディレクトリ**を渡す。
pub const CREDENTIAL_ENV_DIRS: [&str; 3] = [
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    "CODEX_HOME",
];
/// 値が**ファイル**の認証情報（親ディレクトリをマウントする）。
pub const CREDENTIAL_ENV_FILES: [&str; 1] = ["OPENCODE_CONFIG"];

// ---------------------------------------------------------------------------
// runtime（podman / docker）
// ---------------------------------------------------------------------------

/// 使えるコンテナ runtime（ADR-0043 D3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Runtime {
    Podman,
    Docker,
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Podman => "podman",
            Runtime::Docker => "docker",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "podman" => Some(Runtime::Podman),
            "docker" => Some(Runtime::Docker),
            _ => None,
        }
    }
}

/// `[containers] runtime` の設定値。`auto` は podman を先に試し、駄目なら docker（ADR-0043 D3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimePreference {
    #[default]
    Auto,
    Podman,
    Docker,
}

impl RuntimePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimePreference::Auto => "auto",
            RuntimePreference::Podman => "podman",
            RuntimePreference::Docker => "docker",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(RuntimePreference::Auto),
            "podman" => Some(RuntimePreference::Podman),
            "docker" => Some(RuntimePreference::Docker),
            _ => None,
        }
    }

    /// 試す順番（ADR-0043 D3: podman を優先）。
    pub fn candidates(self) -> Vec<Runtime> {
        match self {
            RuntimePreference::Auto => vec![Runtime::Podman, Runtime::Docker],
            RuntimePreference::Podman => vec![Runtime::Podman],
            RuntimePreference::Docker => vec![Runtime::Docker],
        }
    }
}

/// 起動時の検出結果（**観測値**。`GET /daemon` とログに出す。DB には書かない）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeProbe {
    /// 使える runtime。どれも駄目なら `None`（コンテナが要るタスクは `blocked`）。
    pub runtime: Option<Runtime>,
    /// 設定（`auto` / `podman` / `docker`）。
    pub preference: String,
    /// 試した runtime ごとの `<runtime> info` の結果（`("podman", "ok" | 理由)`。順番は試した順）。
    pub tried: Vec<(String, String)>,
}

impl RuntimeProbe {
    pub fn is_available(&self) -> bool {
        self.runtime.is_some()
    }

    /// 選んだ runtime の実行ファイル名。
    pub fn program(&self) -> Option<&'static str> {
        self.runtime.map(Runtime::as_str)
    }

    /// 人に見せる 1 行（ログと `blocked` の質問文）。
    pub fn summary(&self) -> String {
        match self.runtime {
            Some(rt) => format!("{} が使える", rt.as_str()),
            None if self.tried.is_empty() => "コンテナ runtime を試していない".to_string(),
            None => self
                .tried
                .iter()
                .map(|(name, detail)| format!("{name}: {detail}"))
                .collect::<Vec<_>>()
                .join(" / "),
        }
    }
}

/// `<program> info` を起こして使えるかどうかを見る（能力の確認。ADR-0043 D3）。
/// 成功なら `Ok(())`、失敗なら理由の 1 行。**ネットワークには出ない**（ローカルの runtime に聞くだけ）。
pub fn probe_program(program: &str, timeout: Duration) -> Result<(), String> {
    let mut child = match std::process::Command::new(program)
        .arg("info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("{program} が見つからない"));
        }
        Err(e) => return Err(format!("{program} を起こせない: {e}")),
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "`{program} info` が {} 秒で終わらなかった",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("`{program} info` を待てなかった: {e}")),
        }
    }
    let out = match child.wait_with_output() {
        Ok(out) => out,
        Err(e) => return Err(format!("`{program} info` を待てなかった: {e}")),
    };
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("`info` が失敗した")
        .to_string();
    Err(truncate(&reason, 200))
}

/// 検出（判断だけ。`probe` に `<runtime> info` を渡す）。**純粋**なので試験できる。
pub fn detect_with<F>(preference: RuntimePreference, probe: F) -> RuntimeProbe
where
    F: Fn(Runtime) -> Result<(), String>,
{
    let mut out = RuntimeProbe {
        runtime: None,
        preference: preference.as_str().to_string(),
        tried: Vec::new(),
    };
    for candidate in preference.candidates() {
        match probe(candidate) {
            Ok(()) => {
                out.tried
                    .push((candidate.as_str().to_string(), "ok".to_string()));
                out.runtime = Some(candidate);
                break;
            }
            Err(reason) => out.tried.push((candidate.as_str().to_string(), reason)),
        }
    }
    out
}

/// 起動時の検出（`podman info` → `docker info`）。
pub fn detect(preference: RuntimePreference, timeout: Duration) -> RuntimeProbe {
    detect_with(preference, |rt| probe_program(rt.as_str(), timeout))
}

// ---------------------------------------------------------------------------
// 判断（どのタスクをコンテナで走らせるか）
// ---------------------------------------------------------------------------

/// 1 リポジトリ分の判断材料（ADR-0043 D3。`project_repos.run` と `workspace.toml`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRunInput {
    /// 案件の中での名前。
    pub name: String,
    /// `project_repos.run`（`auto` / `host` / `container`）。
    pub run: RepoRun,
    /// 案件の主なリポジトリか（イメージを選ぶ順番は primary が先）。
    pub is_primary: bool,
    /// そのリポジトリの `.config/celeris/workspace.toml`（読めなければ既定）。
    pub config: WorkspaceConfig,
    /// `workspace.toml` を読んだディレクトリ（`dockerfile` と `.config/celeris/` の基準）。
    pub config_dir: PathBuf,
}

/// どのイメージで走るか（ADR-0043 D3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// `[container] image` — **そのまま使う**（ビルドしない）。
    Named(String),
    /// `[container] dockerfile` — 内容 + `.config/celeris/` の sha でタグを付けてビルドする。
    Dockerfile {
        /// Dockerfile と `.config/celeris/` の基準になるディレクトリ。
        repo_dir: PathBuf,
        /// リポジトリ相対の Dockerfile のパス。
        dockerfile: String,
    },
    /// どちらも無い → `[containers] image_default`（`celeris-worker:latest`）。
    Default,
}

/// コンテナで走らせるときの中身（ADR-0043 D3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerChoice {
    /// コンテナを要求した最初のリポジトリの名前（primary → `repos[0]` の順）。
    pub repo: String,
    pub image: ImageSource,
    /// `[container] mounts`（`src:dst[:opts]` をそのまま渡す）。
    pub mounts: Vec<String>,
    /// `[container] env`。
    pub env: Vec<(String, String)>,
}

/// そのリポジトリがコンテナを要求しているか（ADR-0043 D3: `container`、または `auto` + `[run] mode`）。
pub fn wants_container(repo: &RepoRunInput) -> bool {
    match repo.run {
        RepoRun::Container => true,
        RepoRun::Host => false,
        RepoRun::Auto => repo.config.run.mode == RunMode::Container,
    }
}

/// **タスクをコンテナで走らせるか**（純粋関数。ADR-0043 D3）。
///
/// - 1 つでもコンテナを要求したリポジトリがあれば、そのタスクの run はコンテナ（環境は混ぜない）。
/// - イメージ・`mounts`・`env` は **primary → `repos[0]` の順で最初に要求したリポジトリ**のものを使う。
/// - `paperqa` / `local-deep-research` は**常にホスト**（[`HOST_ONLY_ADAPTERS`]）。
pub fn decide(repos: &[RepoRunInput], adapter_id: &str) -> Option<ContainerChoice> {
    if HOST_ONLY_ADAPTERS.contains(&adapter_id) {
        return None;
    }
    let chosen = repos
        .iter()
        .find(|r| r.is_primary && wants_container(r))
        .or_else(|| repos.iter().find(|r| wants_container(r)))?;
    let container = &chosen.config.container;
    let image = match (&container.image, &container.dockerfile) {
        (Some(image), _) if !image.trim().is_empty() => {
            ImageSource::Named(image.trim().to_string())
        }
        (_, Some(dockerfile)) if !dockerfile.trim().is_empty() => ImageSource::Dockerfile {
            repo_dir: chosen.config_dir.clone(),
            dockerfile: dockerfile.trim().to_string(),
        },
        _ => ImageSource::Default,
    };
    Some(ContainerChoice {
        repo: chosen.name.clone(),
        image,
        mounts: container.mounts.clone(),
        env: container
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// 計画とコマンドの組み立て
// ---------------------------------------------------------------------------

/// 1 タスク分のコンテナの形（決定的。ADR-0043 D3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPlan {
    pub runtime: Runtime,
    /// 実行ファイル（既定は `runtime.as_str()`。試験では偽物の絶対パスを差す）。
    pub program: String,
    pub image: String,
    /// `<workspace_root>/<task_id>`。**同じパス**で読み書きできるようにマウントする。
    pub task_dir: PathBuf,
    /// `dir` リポジトリの実体（シンボリックリンクの先）。同じパスでマウントする。
    pub dir_repos: Vec<PathBuf>,
    /// 追加で読み取り専用にする認証情報（アカウントプールが選んだディレクトリなど）。
    /// これとは別に、環境変数（[`CREDENTIAL_ENV_DIRS`] / [`CREDENTIAL_ENV_FILES`]）からも拾う。
    pub creds: Vec<PathBuf>,
    /// `workspace.toml` の `[container] mounts`（`src:dst[:opts]`）。
    pub extra_mounts: Vec<String>,
    /// ADR-0047 D3（Phase 61）: 知識ベースの根。**同じパスに読み取り専用**でマウントし、`_inbox` だけ
    /// 書き込み可にする（`celerisctl knowledge` がコンテナの中でもそのまま動く）。`None` ならマウントしない。
    pub knowledge_root: Option<PathBuf>,
    /// `workspace.toml` の `[container] env`（アダプタの環境より**後**に置くので勝つ）。
    pub env: Vec<(String, String)>,
    /// `--label celeris.task=<task_id>`。
    pub task_id: String,
    /// docker のときの `--user <uid>:<gid>`（podman は `--userns=keep-id`）。
    pub uid: u32,
    pub gid: u32,
}

impl ContainerPlan {
    /// `--label celeris.task=<task_id>` の値。
    pub fn label(&self) -> String {
        format!("{TASK_LABEL}={}", self.task_id)
    }
}

/// `(program, args, env, cwd)` → `<runtime> run …` の**引数**（`plan.program` は含まない）。純粋関数。
pub fn argv(
    plan: &ContainerPlan,
    program: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
) -> Vec<String> {
    let mut out: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-i".into(),
        "--network".into(),
        "host".into(),
    ];
    match plan.runtime {
        // rootless の podman はホストと同じ uid に見せる（マウントした worktree にそのまま書ける）。
        Runtime::Podman => out.push("--userns=keep-id".into()),
        // docker はデーモンが root なので、書いたファイルがホストで root にならないよう uid を渡す。
        Runtime::Docker => {
            out.push("--user".into());
            out.push(format!("{}:{}", plan.uid, plan.gid));
        }
    }
    out.push("-w".into());
    out.push(cwd.display().to_string());

    // 読み書きのマウント: タスクのディレクトリ（worktree・artifacts・runs）、`dir` リポジトリの実体、
    // そして task_dir の外に出ている cwd（`mode = shared` のタスク）。同じパスに置く。
    let mut rw: Vec<PathBuf> = vec![plan.task_dir.clone()];
    rw.extend(plan.dir_repos.iter().cloned());
    if !is_under(cwd, &rw) {
        rw.push(cwd.to_path_buf());
    }
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for path in &rw {
        if !path.is_absolute() || !seen.insert(path.clone()) {
            continue;
        }
        out.push("-v".into());
        out.push(format!("{0}:{0}", path.display()));
    }

    // 認証情報は**読み取り専用**で、アダプタが env に書いた場所だけ（ADR-0043 D3）。
    let mut ro: Vec<PathBuf> = plan.creds.clone();
    ro.extend(credential_dirs(env));
    let mut seen_ro: BTreeSet<PathBuf> = BTreeSet::new();
    for path in &ro {
        if !path.is_absolute() || is_under(path, &rw) || !seen_ro.insert(path.clone()) {
            continue;
        }
        out.push("-v".into());
        out.push(format!("{0}:{0}:ro", path.display()));
    }

    // 環境変数: HOME の既定（タスクのディレクトリ。ホームは絶対にマウントしない）→ アダプタが渡すもの →
    // `workspace.toml` の `[container] env`。同じキーは後勝ち。
    out.push("--env".into());
    out.push(format!("HOME={}", plan.task_dir.display()));
    for (k, v) in env.iter().chain(plan.env.iter()) {
        out.push("--env".into());
        out.push(format!("{k}={v}"));
    }

    // ADR-0047 D3（Phase 61）: 知識ベース。正本は読み取り専用、候補の置き場（`_inbox`）だけ書き込み可。
    if let Some(kb) = &plan.knowledge_root
        && kb.is_absolute()
        && !is_under(kb, &rw)
    {
        out.push("-v".into());
        out.push(format!("{0}:{0}:ro", kb.display()));
        let inbox = kb.join(task_core::knowledge::INBOX_DIR);
        out.push("-v".into());
        out.push(format!("{0}:{0}", inbox.display()));
    }

    // `workspace.toml` の追加マウント（`/dev/infiniband:/dev/infiniband` など）。
    for mount in &plan.extra_mounts {
        out.push("-v".into());
        out.push(mount.clone());
    }

    out.push("--label".into());
    out.push(plan.label());
    out.push(plan.image.clone());
    out.push(program.to_string());
    out.extend(args.iter().cloned());
    out
}

/// **差し込み点**（ADR-0043 D3）。組み立て終えた `Command` を読み、コンテナの中で走る `Command` に作り直す。
/// `plan` が `None` なら**そのまま返す**（ホスト実行は従来どおり 1 バイトも変わらない）。
///
/// stdio・`kill_on_drop`・`process_group` は呼び出し側がこの後で付けるので、ここでは触らない。
pub fn wrap(
    command: tokio::process::Command,
    plan: Option<&ContainerPlan>,
) -> tokio::process::Command {
    let Some(plan) = plan else {
        return command;
    };
    let std_command = command.as_std();
    let program = std_command.get_program().to_string_lossy().into_owned();
    let args: Vec<String> = std_command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let env: Vec<(String, String)> = std_command
        .get_envs()
        .filter_map(|(k, v)| {
            v.map(|v| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
        })
        .collect();
    let cwd = std_command
        .get_current_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| plan.task_dir.clone());

    let mut wrapped = tokio::process::Command::new(&plan.program);
    wrapped.args(argv(plan, &program, &args, &env, &cwd));
    // runtime のクライアント自身はホストの環境で動かす（`DOCKER_HOST` / `XDG_RUNTIME_DIR` が要る）。
    // ワーカーに渡す環境は上の `--env` で入る。
    wrapped.current_dir(&plan.task_dir);
    wrapped
}

/// `env` のうち認証情報を指しているものの**ディレクトリ**（ADR-0043 D3）。
fn credential_dirs(env: &[(String, String)]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (k, v) in env {
        if v.is_empty() {
            continue;
        }
        if CREDENTIAL_ENV_DIRS.contains(&k.as_str()) {
            out.push(PathBuf::from(v));
        } else if CREDENTIAL_ENV_FILES.contains(&k.as_str())
            && let Some(parent) = Path::new(v).parent()
            && !parent.as_os_str().is_empty()
        {
            out.push(parent.to_path_buf());
        }
    }
    out
}

/// `path` が `roots` のどれかの下（か同じ）か。
fn is_under(path: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| path == root || path.starts_with(root))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

// ---------------------------------------------------------------------------
// イメージ（タグ・キャッシュ・ビルド）
// ---------------------------------------------------------------------------

/// Dockerfile の内容 + `.config/celeris/` の中身から決まるタグ（ADR-0043 D3。同じ内容なら再ビルドしない）。
pub fn image_tag(dockerfile: &str, context_digest: &str) -> String {
    let mut h = Sha256::new();
    h.update(dockerfile.as_bytes());
    h.update([0u8]);
    h.update(context_digest.as_bytes());
    let digest = h.finalize();
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{BUILT_IMAGE_PREFIX}{}", &hex[..12])
}

/// `.config/celeris/` の中身を決定的に畳んだ sha256（相対パス昇順。無ければ空ディレクトリの sha）。
pub fn context_digest(dir: &Path) -> String {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(dir, dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    for (rel, path) in &files {
        h.update(rel.as_bytes());
        h.update([0u8]);
        match std::fs::read(path) {
            Ok(bytes) => {
                h.update(bytes.len().to_le_bytes());
                h.update(&bytes);
            }
            Err(_) => h.update(b"<unreadable>"),
        }
        h.update([0u8]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            collect_files(root, &path, out);
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push((rel.to_string_lossy().into_owned(), path));
        }
    }
}

/// 使うイメージの名前を決める（ビルドが要るものは `Build`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedImage {
    /// そのまま使う（`[container] image` か `image_default`）。
    Ready(String),
    /// ビルドが要る（`[container] dockerfile`）。
    Build(BuildRequest),
}

impl ResolvedImage {
    pub fn tag(&self) -> &str {
        match self {
            ResolvedImage::Ready(tag) => tag,
            ResolvedImage::Build(req) => &req.tag,
        }
    }
}

/// `[container] dockerfile` のビルド 1 回分（ADR-0043 D3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    /// `celeris-ws-<sha12>`。
    pub tag: String,
    /// 元の Dockerfile（絶対パス）。
    pub dockerfile: PathBuf,
    /// 文脈にする `.config/celeris/`（絶対パス。無ければ Dockerfile の親）。
    pub context_src: PathBuf,
    /// ビルドの作業場所 `<build_dir>/<tag>/`。
    pub build_dir: PathBuf,
}

/// `ImageSource` を実際のタグに落とす（I/O は `context_digest` の読み取りだけ）。
pub fn resolve_image(
    source: &ImageSource,
    image_default: &str,
    build_root: &Path,
) -> Result<ResolvedImage, String> {
    match source {
        ImageSource::Named(image) => Ok(ResolvedImage::Ready(image.clone())),
        ImageSource::Default => Ok(ResolvedImage::Ready(image_default.to_string())),
        ImageSource::Dockerfile {
            repo_dir,
            dockerfile,
        } => {
            let path = repo_dir.join(dockerfile);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("Dockerfile `{}` が読めない: {e}", path.display()))?;
            let context_src = repo_dir.join(".config/celeris");
            let digest = if context_src.is_dir() {
                context_digest(&context_src)
            } else {
                String::new()
            };
            let tag = image_tag(&text, &digest);
            let build_dir = build_root.join(&tag);
            Ok(ResolvedImage::Build(BuildRequest {
                tag,
                dockerfile: path,
                context_src,
                build_dir,
            }))
        }
    }
}

/// そのイメージが手元にあるか（`<program> image inspect <tag>`）。
pub fn image_exists(program: &str, tag: &str) -> bool {
    std::process::Command::new(program)
        .args(["image", "inspect", tag])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// イメージを用意する（あれば**ビルドしない**。ADR-0043 D3「同じ内容なら再ビルドしない」）。
///
/// - `exists` は試験のために差し替えられる（キャッシュに当たったかどうかを見る）
/// - ビルドの記録は `log_path`（`<task_dir>/runs/container-build.log`）
/// - `timeout` を超えたら kill して失敗（呼び出し側はタスクを `blocked` にして人に聞く）
pub async fn ensure_image(
    program: &str,
    request: &BuildRequest,
    timeout: Duration,
    log_path: &Path,
    exists: &(dyn Fn(&str) -> bool + Sync),
) -> Result<(), String> {
    if exists(&request.tag) {
        return Ok(());
    }
    // 文脈は `<build_dir>/context/`（`.config/celeris/` の写し）。リポジトリ全体は送らない。
    let context = request.build_dir.join("context");
    if let Err(e) = tokio::fs::create_dir_all(&context).await {
        return Err(format!(
            "ビルドの作業場所 `{}` を作れない: {e}",
            context.display()
        ));
    }
    if request.context_src.is_dir()
        && let Err(e) = copy_tree(&request.context_src, &context)
    {
        return Err(format!(
            "`{}` を写せない: {e}",
            request.context_src.display()
        ));
    }
    // Dockerfile が `.config/celeris/` の外にある場合も、写しの中に 1 つ置く。
    let dockerfile_in_context = match request.dockerfile.strip_prefix(&request.context_src) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => {
            let dest = context.join("Dockerfile");
            if let Err(e) = std::fs::copy(&request.dockerfile, &dest) {
                return Err(format!("Dockerfile を写せない: {e}"));
            }
            PathBuf::from("Dockerfile")
        }
    };

    let mut command = tokio::process::Command::new(program);
    command
        .arg("build")
        // run と同じネットワーク（ADR-0043 D3）。加えて、入れ子のコンテナ（この LXC）では
        // ビルド中の `RUN` がブリッジを張れず `OCI runtime create failed: recvfrom(PF_NETLINK)` で
        // 落ちるので、ホストのネットワークで動かす（Phase 56 の実機確認）。
        .arg("--network")
        .arg("host")
        .arg("-t")
        .arg(&request.tag)
        .arg("-f")
        .arg(context.join(&dockerfile_in_context))
        .arg(".")
        .current_dir(&context)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let started = format!(
        "$ {program} build --network host -t {} -f {} .\n",
        request.tag,
        dockerfile_in_context.display()
    );
    let output = match tokio::time::timeout(timeout, command.output()).await {
        Err(_) => {
            append_log(
                log_path,
                &format!(
                    "{started}=> 失敗: {} 秒で終わらなかった\n",
                    timeout.as_secs()
                ),
            )
            .await;
            return Err(format!(
                "イメージ `{}` のビルドが {} 秒で終わらなかった",
                request.tag,
                timeout.as_secs()
            ));
        }
        Ok(Err(e)) => {
            append_log(log_path, &format!("{started}=> 失敗: {e}\n")).await;
            return Err(format!("`{program} build` を起こせない: {e}"));
        }
        Ok(Ok(output)) => output,
    };
    let mut log = started;
    log.push_str(&String::from_utf8_lossy(&output.stdout));
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        log.push_str("[stderr] ");
        log.push_str(&stderr);
        if !stderr.ends_with('\n') {
            log.push('\n');
        }
    }
    if output.status.success() {
        log.push_str("=> exit 0\n");
        append_log(log_path, &log).await;
        return Ok(());
    }
    log.push_str(&format!("=> 失敗: exit {:?}\n", output.status.code()));
    append_log(log_path, &log).await;
    let tail = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("理由は記録を見よ");
    Err(format!(
        "イメージ `{}` のビルドが失敗した（exit {:?}）: {}",
        request.tag,
        output.status.code(),
        truncate(tail, 200)
    ))
}

async fn append_log(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    use tokio::io::AsyncWriteExt as _;
    match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        Ok(mut file) => {
            let _ = file.write_all(text.as_bytes()).await;
            // Tokio may still have a blocking write queued after write_all returns.
            // Build callers inspect this log as soon as ensure_image completes.
            let _ = file.flush().await;
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not write the container build log")
        }
    }
}

fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let meta = entry.metadata()?;
        if meta.is_dir() {
            copy_tree(&from, &to)?;
        } else if meta.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 後片付け（kill の経路から呼ぶ）
// ---------------------------------------------------------------------------

/// kill の経路から呼ぶ後始末の口（ADR-0044 B2 のプロセスグループ kill helper からも呼べるようにしておく）。
///
/// `--rm` を付けているので `<runtime> run` のクライアントに SIGTERM が届けばコンテナも止まるが、
/// **クライアントの pid への `killpg` はコンテナの中には届かない**（別の PID 名前空間。ADR-0044 P55-4）。
/// そこで「ラベルで止める」道を 2 段に分けて用意する:
///
/// 1. [`ContainerStopper::terminate_blocking`] — 中の PID 1 へ SIGTERM（`<runtime> kill --signal TERM`）。
///    ホストのプロセスグループへ送る SIGTERM と**同じ瞬間**に送る。片付けの猶予を与えるのが目的。
/// 2. [`ContainerStopper::stop_blocking`] — `grace` 後の `rm -f`（ホストの SIGKILL と同じ瞬間）。
pub trait ContainerStopper: Send + Sync {
    /// `--label celeris.task=<task_id>` の付いたコンテナの中へ SIGTERM を送る（同期）。
    fn terminate_blocking(&self);
    /// `--label celeris.task=<task_id>` の付いたコンテナを消す（同期。`grace` の後に呼ぶ）。
    fn stop_blocking(&self);
}

impl ContainerStopper for ContainerPlan {
    fn terminate_blocking(&self) {
        terminate_by_label(&self.program, &self.task_id);
    }

    fn stop_blocking(&self) {
        stop_by_label(&self.program, &self.task_id);
    }
}

/// ラベルでコンテナを止めるのに要る最小限（runtime の実行ファイルとタスク id）。
///
/// `ContainerPlan` そのものを kill の経路へ持ち回すと、イメージやマウントまで抱えることになる。
/// ディスパッチャが `RunEntry` に置くのはこれだけでよい（ADR-0044 §5 / ADR-0043 P56-7）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerStop {
    pub program: String,
    pub task_id: String,
}

impl ContainerStop {
    /// 計画から作る（`program` と `task_id` だけを借りる）。
    pub fn of(plan: &ContainerPlan) -> Self {
        Self {
            program: plan.program.clone(),
            task_id: plan.task_id.clone(),
        }
    }
}

impl ContainerStopper for ContainerStop {
    fn terminate_blocking(&self) {
        terminate_by_label(&self.program, &self.task_id);
    }

    fn stop_blocking(&self) {
        stop_by_label(&self.program, &self.task_id);
    }
}

/// `--label celeris.task=<task_id>` の付いたコンテナの id（`ps -aq --filter label=…`）。
/// runtime が起動できない・落ちたなら空（kill の経路では黙って諦める）。
fn ids_by_label(program: &str, task_id: &str) -> Vec<String> {
    let filter = format!("label={TASK_LABEL}={task_id}");
    let listed = std::process::Command::new(program)
        .args(["ps", "-aq", "--filter", &filter])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(listed) = listed else {
        return Vec::new();
    };
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// `--label celeris.task=<task_id>` の付いたコンテナの中の PID 1 へ SIGTERM を送る
/// （`ps -aq --filter` → `kill --signal TERM`）。ホストのプロセスグループへの `killpg` は
/// 別の PID 名前空間には届かないので、コンテナで走る run にはこちらが要る（ADR-0044 P55-4）。
/// 失敗しても warn するだけ（run の結果には影響させない）。
pub fn terminate_by_label(program: &str, task_id: &str) {
    let ids = ids_by_label(program, task_id);
    if ids.is_empty() {
        return;
    }
    let mut command = std::process::Command::new(program);
    command.arg("kill").arg("--signal").arg("TERM").args(&ids);
    if let Err(e) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        tracing::warn!(%task_id, error = %e, "could not signal the task container");
    }
}

/// `--label celeris.task=<task_id>` の付いたコンテナを強制的に消す（`ps -aq --filter` → `rm -f`）。
/// 失敗しても warn するだけ（run の結果には影響させない）。
pub fn stop_by_label(program: &str, task_id: &str) {
    let ids = ids_by_label(program, task_id);
    if ids.is_empty() {
        return;
    }
    let mut command = std::process::Command::new(program);
    command.arg("rm").arg("-f").args(&ids);
    if let Err(e) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        tracing::warn!(%task_id, error = %e, "could not remove the task container");
    }
}

/// 人に見せる質問文（ADR-0043 D3「runtime が使えません」）。`setup` の失敗と同じ経路で `blocked` になる。
pub fn unavailable_question(probe: &RuntimeProbe, repo: &str) -> String {
    format!(
        "コンテナ runtime が使えません（`{repo}` が `run = container` を要求しています）: {}。\
         `[containers] runtime` の設定か、podman / docker の用意を見てください。\
         ホストで走らせてよければ、そのリポジトリの `run` を `host` にしてください。",
        probe.summary()
    )
}

/// ビルドやイメージの失敗を人に聞く文面（同じく `blocked`）。
pub fn image_question(repo: &str, reason: &str, log_path: &Path) -> String {
    format!(
        "コンテナのイメージを用意できませんでした（`{repo}`）: {reason}。記録は `{}` にあります。\
         直し方を教えてください（Dockerfile を直す／`[container] image` を指す／`run = host` にする）。",
        log_path.display()
    )
}

/// ホストの uid / gid（docker の `--user` に渡す。podman は `--userns=keep-id` なので要らないが、
/// どちらでも同じ計画を作れるように常に取る）。
pub fn host_ids() -> (u32, u32) {
    (
        nix::unistd::getuid().as_raw(),
        nix::unistd::getgid().as_raw(),
    )
}

/// `ContainerPlan` を `Arc` で持ち回すときの別名（アダプタの設定に入る）。
pub type SharedPlan = Arc<ContainerPlan>;

#[cfg(test)]
mod tests;
