//! 作業場所・worktree・container の準備（ADR-0041、ADR-0043、ADR-0059）。ADR-0082 の L1。

use super::*;

/// ADR-0043 D3（Phase 56）: 起動時の `<runtime> info` の上限（届かない docker デーモンで固まらない）。
pub(super) const CONTAINER_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// ADR-0041 D1 / ADR-0043 D2: `<task_dir>/worktree.json` の中身。先頭の 5 つは Phase 49 からある
/// 「1 リポジトリのときの姿」で、複数リポジトリのタスクでは `repos[0]`（cwd になるもの）の写しが入る。
pub(super) fn worktree_marker(
    ws: &task_worker::TaskWorkspaces,
) -> task_ops::workspace::WorktreeMarker {
    let first = ws.repos.first();
    task_ops::workspace::WorktreeMarker {
        repo: first
            .map(|r| r.source.to_string_lossy().into_owned())
            .unwrap_or_default(),
        dir: first
            .map(|r| r.dir.to_string_lossy().into_owned())
            .unwrap_or_default(),
        branch: first
            .and_then(|r| r.branch())
            .unwrap_or_default()
            .to_string(),
        base: first
            .and_then(|r| r.worktree.as_ref())
            .map(|w| w.base.sha.clone())
            .unwrap_or_default(),
        base_kind: first
            .and_then(|r| r.worktree.as_ref())
            .map(|w| w.base.kind.as_str().to_string())
            .unwrap_or_default(),
        repos: ws
            .repos
            .iter()
            .map(|r| task_ops::workspace::WorktreeMarkerRepo {
                name: r.name.clone(),
                kind: if r.is_git() {
                    "git".into()
                } else {
                    "dir".into()
                },
                source: r.source.to_string_lossy().into_owned(),
                dir: r.dir.to_string_lossy().into_owned(),
                branch: r.branch().map(str::to_string),
                base: r.worktree.as_ref().map(|w| w.base.sha.clone()),
                base_kind: r
                    .worktree
                    .as_ref()
                    .map(|w| w.base.kind.as_str().to_string()),
            })
            .collect(),
    }
}

/// ADR-0018 D5: ワークスペースの失敗をアダプタの失敗に写す。`Unreachable`（ssh / rsync 自体の失敗）は
/// 供給側失敗（`Spawn`）にして、attempts を消費せず requeue されるようにする。
pub(super) fn workspace_error_to_adapter(
    e: task_worker::WorkspaceError,
    context: &str,
) -> AdapterError {
    match e {
        task_worker::WorkspaceError::Unreachable(msg) => {
            AdapterError::Spawn(std::io::Error::other(format!("{context}: {msg}")))
        }
        other => AdapterError::Other(format!("{context}: {other}")),
    }
}

/// ADR-0059 D3: `mode` が省略された（明示的に `"worktree"` を選んでいない）Remote workspace か。
/// `Local` は関係ないので `false`。
pub(super) fn remote_mode_omitted(workspace: &WorkspaceSpec) -> bool {
    matches!(workspace, WorkspaceSpec::Remote { mode: None, .. })
}

/// ADR-0059 D3: `Remote` workspace を `mode: Some(Shared)` に書き換えた複製（自動の格下げ）。
/// `Local` は関係ないので `None`。
pub(super) fn downgraded_remote_workspace(workspace: &WorkspaceSpec) -> Option<WorkspaceSpec> {
    match workspace {
        WorkspaceSpec::Remote { cluster, path, .. } => Some(WorkspaceSpec::Remote {
            cluster: cluster.clone(),
            path: path.clone(),
            mode: Some(WorkspaceMode::Shared),
        }),
        WorkspaceSpec::Local { .. } => None,
    }
}

/// ADR 2026-10-10-local-disk-growth-paths D1: 祖先をたどる上限（木の深さ `max_depth` 3 に余裕を持たせる。
/// 親の連鎖が壊れていても止まる）。
const CARGO_TARGET_ANCESTOR_LIMIT: usize = 8;

/// ADR 2026-10-10-local-disk-growth-paths D1: 自分の作業場所の先頭に git の repo が無い run・検査が、
/// `CARGO_TARGET_DIR`（scratch の lease）を結び付ける Rust の checkout。cargo は `CARGO_TARGET_DIR` が無いと
/// cwd の repo 直下に `target/` を作り、scratch の lease・seed・GC の外になる。
#[derive(Debug, Clone)]
pub(super) enum CargoTargetFallback {
    /// task の作業場所そのもの（`mode = shared`・worktree を切らない Local path、`kind = dir` の repo）が
    /// Rust の checkout。
    SharedPath(task_worker::TaskRepo),
    /// `repos=[]` の子 task: 祖先 task の checkout（objective が指す親の作業場所）。
    Ancestor {
        ancestor: TaskId,
        repo: task_worker::TaskRepo,
    },
    /// 祖先に Rust の checkout の設定はあるが作業ツリーがまだ無い。理由を残す（黙って repo 直下に落とさない）。
    Missing { reason: String },
    /// Rust の checkout が見当たらない（Rust でない task・Remote）。何も与えない。
    NotRust,
}

impl CargoTargetFallback {
    /// target を結び付ける checkout（`SharedPath` / `Ancestor` だけ）。
    pub(super) fn repo(&self) -> Option<&task_worker::TaskRepo> {
        match self {
            Self::SharedPath(repo) | Self::Ancestor { repo, .. } => Some(repo),
            Self::Missing { .. } | Self::NotRust => None,
        }
    }
}

/// `dir` が Rust の checkout か（`Cargo.toml` がある）。
fn is_rust_checkout(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file()
}

impl Dispatcher {
    /// ADR 2026-10-10-local-disk-growth-paths D1: run・検査に与える `CARGO_TARGET_DIR` を結び付ける checkout。
    /// 自分の作業場所の先頭が git の repo ならそれ、無ければ [`Self::cargo_target_fallback`]。
    pub(super) fn cargo_target_repo(&self, task: &Task) -> Option<task_worker::TaskRepo> {
        let own = self
            .task_workspaces_for(task)
            .and_then(|ws| ws.repos.into_iter().next())
            .filter(|r| r.is_git());
        if own.is_some() {
            return own;
        }
        let fallback = self.cargo_target_fallback(task);
        if let CargoTargetFallback::Missing { reason } = &fallback {
            tracing::warn!(task_id = %task.id, %reason, "cargo target: CARGO_TARGET_DIR not set (ADR 2026-10-10-local-disk-growth-paths D1)");
        }
        fallback.repo().cloned()
    }

    /// ADR 2026-10-10-local-disk-growth-paths D1: 自分の作業場所に git の repo が無い task の、決定的な
    /// 代わりの checkout。順に (1) task の Local path・先頭の `kind = dir` repo が Rust の checkout なら
    /// それ（cargo はそこに書く）、(2) 親から祖先へたどり、最初に作業ツリーがある Rust の git checkout
    /// （`repos=[]` の子 task が親の checkout を使う形）。Remote は対象外（`NotRust`）。
    pub(super) fn cargo_target_fallback(&self, task: &Task) -> CargoTargetFallback {
        let WorkspaceSpec::Local { path, .. } = &task.workspace else {
            return CargoTargetFallback::NotRust;
        };
        let dir = if path.is_absolute() {
            path.clone()
        } else {
            self.config.workspace_root.join(path)
        };
        if let Some(repo) = self
            .task_workspaces_for(task)
            .and_then(|ws| ws.repos.into_iter().next())
            .filter(|r| is_rust_checkout(&r.dir))
        {
            return CargoTargetFallback::SharedPath(repo);
        }
        if is_rust_checkout(&dir) {
            let name = task_worker::task_repos::repo_display_name(&dir);
            return CargoTargetFallback::SharedPath(task_worker::TaskRepo::link(
                name,
                dir.clone(),
                dir,
            ));
        }
        let mut missing: Option<String> = None;
        let mut cursor = task.parent_id;
        for _ in 0..CARGO_TARGET_ANCESTOR_LIMIT {
            let Some(id) = cursor else { break };
            let ancestor = match self.store.get(id) {
                Ok(Some(t)) => t,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(task_id = %task.id, ancestor = %id, error = %e, "cargo target: cannot read the ancestor task");
                    break;
                }
            };
            if let Some(repo) = self
                .task_workspaces_for(&ancestor)
                .and_then(|ws| ws.repos.into_iter().next())
                .filter(|r| r.is_git())
            {
                if is_rust_checkout(&repo.dir) {
                    return CargoTargetFallback::Ancestor { ancestor: id, repo };
                }
                if missing.is_none() && !repo.dir.is_dir() && is_rust_checkout(&repo.source) {
                    missing = Some(format!(
                        "the checkout {} of ancestor task {id} does not exist yet",
                        repo.dir.display()
                    ));
                }
            }
            cursor = ancestor.parent_id;
        }
        match missing {
            Some(reason) => CargoTargetFallback::Missing { reason },
            None => CargoTargetFallback::NotRust,
        }
    }
}

impl Dispatcher {
    /// ADR-0005 D3: `Local{path}` がそのタスクの作業ディレクトリ。相対なら `workspace_root` 基準。
    ///
    /// ADR-0041 D1: ただし worktree を切るタスク（`mode = worktree` かつ `path` が git リポジトリ）では、
    /// **`runs/` `inputs/` `artifacts/` を置く場所**は `workspace_root/<task_id>` になる
    /// （作業ツリーそのものは `<そこ>/tree`。`git status --porcelain` を汚さないため外に出す）。
    pub(super) fn task_dir(&self, task: &Task) -> Option<PathBuf> {
        match &task.workspace {
            WorkspaceSpec::Local { path, .. } => Some(match self.task_workspaces_for(task) {
                Some(ws) => ws.task_dir,
                None if path.is_absolute() => path.clone(),
                None => self.config.workspace_root.join(path),
            }),
            // ADR-0018 D1: クラスタ側が正で、手元は写し（`workspace_root/<task_id>`）。
            WorkspaceSpec::Remote { .. } => {
                Some(self.config.workspace_root.join(task.id.to_string()))
            }
        }
    }

    /// ADR-0041 D1 / ADR-0043 D2: そのタスクに用意する（用意した）ローカルの作業場所。
    ///
    /// 決め方は決定的（LLM は使わない）:
    ///
    /// 1. **タスクがリポジトリを選んでいる**（`task.repos`、ADR-0043 D2）→ `<task_dir>/repos/<name>/` に
    ///    1 つずつ並べる。`kind = git` で実際に git リポジトリなら worktree、そうでなければ実体への
    ///    シンボリックリンク（`kind = dir`、`mode = shared`、git でなかった場合）。
    ///    **リモートのリポジトリを含むタスクは `None`**（従来の ADR-0018 / 0019 の経路に倒す。
    ///    ローカルと混ぜたタスクは作成時に 422 で弾いてある）。
    /// 2. **選んでいない**（案件にリポジトリが無い・案件に属さない）→ Phase 49 と同じ 1 つだけの worktree
    ///    （`<task_dir>/tree`）。条件は 3 つ: `Local`、`mode = worktree`（既定）、`path` が git リポジトリ。
    ///
    /// base の決め方は `task_worker::resolve_base`（`main` / 本番の `current` / `HEAD`。ADR-0041 D1）。
    pub(super) fn task_workspaces_for(&self, task: &Task) -> Option<task_worker::TaskWorkspaces> {
        let task_dir = self.config.workspace_root.join(task.id.to_string());
        if task.repos.is_empty() {
            let worktree = self.legacy_worktree_for(task, &task_dir)?;
            let name = task_worker::task_repos::repo_display_name(&worktree.repo);
            return Some(task_worker::TaskWorkspaces {
                task_dir,
                repos: vec![task_worker::TaskRepo::git(name, worktree)],
            });
        }
        let mut repos = Vec::with_capacity(task.repos.len());
        for reference in &task.repos {
            let row = match self.store.repo_get(reference.repo_id) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    tracing::warn!(task_id = %task.id, repo = %reference.name, "the project repo is gone; falling back to the plain workspace");
                    return None;
                }
                Err(e) => {
                    tracing::error!(task_id = %task.id, repo = %reference.name, error = %e, "cannot read the project repo");
                    return None;
                }
            };
            let task_core::WorkspaceSpec::Local { path, mode } = &row.location else {
                // ADR-0043 D2 / D7: リモートは従来の経路（手元の写し + rsync）。
                return None;
            };
            let source = if path.is_absolute() {
                path.clone()
            } else {
                self.config.workspace_root.join(path)
            };
            let dir = task_dir.join(task_worker::REPOS_DIR_NAME).join(&row.name);
            let wants_worktree = row.kind == task_core::RepoKind::Git
                && mode.unwrap_or_default() == task_core::WorkspaceMode::Worktree
                && task_worker::is_git_repo(&source);
            let base = if wants_worktree {
                self.worktree_base_for(task, &source)
            } else {
                None
            };
            // A human-approved documentation reconciliation arrives with a committed worktree.
            // Preserve that exact branch for the subsequent ordinary verification/review task.
            if task_dir
                .join("artifacts/reconciliation-plan.json")
                .is_file()
                && let Some(marker) = task_ops::workspace::read_marker(&task_dir)
                && let Some(existing) = marker.repos.iter().find(|r| r.name == row.name)
                && std::path::Path::new(&existing.source) == source
                && std::path::Path::new(&existing.dir) == dir
                && let (Some(branch), Some(sha)) = (&existing.branch, &existing.base)
                && branch.starts_with("docs-reconcile/")
                && task_ops::changes::current_branch(&dir).as_ref() == Some(branch)
            {
                repos.push(task_worker::TaskRepo::git(
                    row.name.clone(),
                    task_worker::LocalWorktree {
                        dir,
                        task_dir: task_dir.clone(),
                        repo: source,
                        branch: branch.clone(),
                        base: task_worker::BaseRef {
                            kind: task_worker::BaseKind::Main,
                            sha: sha.clone(),
                        },
                    },
                ));
                continue;
            }
            match base {
                Some(base) => repos.push(task_worker::TaskRepo::git(
                    row.name.clone(),
                    task_worker::LocalWorktree {
                        dir,
                        task_dir: task_dir.clone(),
                        repo: source,
                        branch: format!("{}{}", self.config.worktree_branch_prefix, task.id),
                        base,
                    },
                )),
                None => repos.push(task_worker::TaskRepo::link(row.name.clone(), source, dir)),
            }
        }
        if repos.is_empty() {
            return None;
        }
        Some(task_worker::TaskWorkspaces { task_dir, repos })
    }

    /// ADR-0043 D3（Phase 56）: **このタスクをコンテナで走らせるか**。ストアと `workspace.toml` を
    /// 読むだけの決定的な判断で、LLM は使わない（DESIGN 原則 1）。
    ///
    /// - リモート（`WorkspaceSpec::Remote`）と作業場所の無いタスクはホスト（ADR-0043 D7 は後続）
    /// - `paperqa` / `local-deep-research` はホスト（道具立てがホストの venv にある。`container::decide`）
    /// - 1 つでも `run = container`（か `auto` + `[run] mode = "container"`）なら**コンテナ**
    /// - runtime が使えなければ `Unavailable`（run を始めず `blocked` にして人に聞く）
    pub(super) fn container_decision(
        &self,
        task: &Task,
        worktree: Option<&task_worker::TaskWorkspaces>,
        adapter_id: &str,
        remote: bool,
    ) -> ContainerDecision {
        let Some(ws) = worktree else {
            return ContainerDecision::Host;
        };
        if remote {
            return ContainerDecision::Host;
        }
        // リポジトリごとの `run` と `is_primary`。Phase 49 の 1 リポジトリのタスクは
        // `project_repos` の行を持たないので `auto` + primary として扱う。
        let mut inputs: Vec<task_worker::RepoRunInput> = Vec::with_capacity(ws.repos.len());
        for repo in &ws.repos {
            let row = task
                .repos
                .iter()
                .find(|r| r.name == repo.name)
                .and_then(|r| self.store.repo_get(r.repo_id).ok().flatten());
            // `workspace.toml` は作業ツリーがあればそこ、無ければ元のリポジトリ（`repo_notes` と同じ規則）。
            let from = if repo.dir.is_dir() {
                &repo.dir
            } else {
                &repo.source
            };
            let (config, warning) = task_core::workspace_config::load_or_default(from);
            if let Some(warning) = warning {
                tracing::warn!(repo = %repo.name, %warning, "cannot read workspace.toml; using the defaults");
            }
            inputs.push(task_worker::RepoRunInput {
                name: repo.name.clone(),
                run: row
                    .as_ref()
                    .map(|r| r.run)
                    .unwrap_or(task_core::RepoRun::Auto),
                is_primary: row.as_ref().map(|r| r.is_primary).unwrap_or(true),
                config,
                config_dir: from.clone(),
            });
        }
        let Some(choice) = task_worker::container::decide(&inputs, adapter_id) else {
            return ContainerDecision::Host;
        };
        let Some(runtime) = self.container_probe.runtime else {
            return ContainerDecision::Unavailable {
                question: task_worker::container::unavailable_question(
                    &self.container_probe,
                    &choice.repo,
                ),
            };
        };
        // `dir` のリポジトリ（シンボリックリンク）は**実体**を同じパスでマウントする。
        let dir_repos: Vec<PathBuf> = ws
            .repos
            .iter()
            .filter(|r| !r.is_git())
            .map(|r| r.source.clone())
            .collect();
        let (uid, gid) = task_worker::container::host_ids();
        let plan = task_worker::ContainerPlan {
            runtime,
            program: runtime.as_str().to_string(),
            // イメージは `run_worker` が run の直前に決める（ビルドが要ることがある）。
            image: self.config.containers.image_default.clone(),
            task_dir: ws.task_dir.clone(),
            dir_repos,
            creds: Vec::new(),
            extra_mounts: choice.mounts.clone(),
            // ADR-0047 D3（Phase 61）: 知識ベースがあれば同じパスで見せる（`_inbox` だけ書き込み可）。
            knowledge_root: Some(self.config.knowledge.root.clone()).filter(|r| r.is_dir()),
            env: choice.env.clone(),
            task_id: task.id.to_string(),
            uid,
            gid,
        };
        ContainerDecision::Container(Box::new(ContainerRun {
            plan,
            image: choice.image,
            image_default: self.config.containers.image_default.clone(),
            build_root: self.config.containers.build_dir.clone(),
            build_timeout: self.config.containers.build_timeout,
            repo: choice.repo,
        }))
    }

    /// Phase 49（ADR-0041 D1）の 1 リポジトリだけの worktree（`<task_dir>/tree`）。
    pub(super) fn legacy_worktree_for(
        &self,
        task: &Task,
        task_dir: &Path,
    ) -> Option<task_worker::LocalWorktree> {
        let WorkspaceSpec::Local { path, .. } = &task.workspace else {
            return None;
        };
        if task.workspace.local_mode() != task_core::WorkspaceMode::Worktree {
            return None;
        }
        let repo = if path.is_absolute() {
            path.clone()
        } else {
            self.config.workspace_root.join(path)
        };
        if !task_worker::is_git_repo(&repo) {
            return None;
        }
        let base = self.worktree_base_for(task, &repo)?;
        Some(task_worker::LocalWorktree {
            dir: task_dir.join(task_worker::WORKTREE_DIR_NAME),
            task_dir: task_dir.to_path_buf(),
            repo,
            branch: format!("{}{}", self.config.worktree_branch_prefix, task.id),
            base,
        })
    }

    /// ADR-0079 D6（Phase R1c）: task の worktree の base。木の子 task は既定ブランチではなく
    /// `Task.tree.base_commit`（親の段階の基点、または同じ段階の依存先の HEAD。子を作るときに
    /// [`Self::child_base_commit`] が決めた値）から切る（`BaseKind::Parent`）。この値が `repo` で解決できない
    /// （子の repos のうち先頭以外のリポジトリ。基点は先頭のリポジトリの sha だけを持つ）ときは、そのリポジトリの
    /// 親のブランチ `celeris/<parent_id>` の HEAD（親のブランチは段階の途中では動かない。D6）。どちらも
    /// 無ければ従来の規則（`main`）に倒す。木の子でない task は従来どおり [`Self::worktree_base`]。
    pub(super) fn worktree_base_for(
        &self,
        task: &Task,
        repo: &Path,
    ) -> Option<task_worker::BaseRef> {
        if let Some(parent_branch) =
            task_core::tree::parent_branch(task, &self.config.worktree_branch_prefix)
        {
            let sha = task_core::tree::child_base_commit(task)
                .and_then(|sha| crate::integration::commit_in(repo, sha))
                .or_else(|| {
                    crate::integration::rev_parse(repo, &format!("refs/heads/{parent_branch}"))
                });
            if let Some(sha) = sha {
                return Some(task_worker::BaseRef {
                    kind: task_worker::BaseKind::Parent,
                    sha,
                });
            }
            tracing::warn!(task_id = %task.id, repo = %repo.display(), %parent_branch, "neither the child's base_commit nor the parent's branch exists in this repository; the child's worktree falls back to the default base (ADR-0079 D6)");
        }
        self.worktree_base(repo)
    }

    /// ADR-0041 D1 の base の規則（`main` / 本番の `current` / `HEAD`）。
    pub(super) fn worktree_base(&self, repo: &Path) -> Option<task_worker::BaseRef> {
        let current = self
            .config
            .releases_dir
            .as_deref()
            .and_then(task_worker::current_release_sha);
        task_worker::resolve_base(repo, current.as_deref())
    }

    /// ADR-0043 D2 / D4 / D8: 前置きの「作業場所」に出すリポジトリ一覧（決定的。`workspace.toml` を
    /// 読むだけで、判断も LLM も無い）。`description` / `check` / `outputs` は
    /// `.config/celeris/workspace.toml` に**書いてあることだけ**を使う（ADR-0043 §3）。
    ///
    /// 読む場所は、作業ツリーが既にあればそこ、無ければ元のリポジトリ（初回の dispatch では
    /// worktree をまだ切っていないため）。
    pub(super) fn repo_notes(
        &self,
        ws: &task_worker::TaskWorkspaces,
    ) -> Vec<task_worker::preamble::RepoNote> {
        ws.repos
            .iter()
            .map(|repo| {
                let from = if repo.dir.is_dir() { &repo.dir } else { &repo.source };
                let (config, warning) = task_core::workspace_config::load_or_default(from);
                if let Some(warning) = warning {
                    tracing::warn!(repo = %repo.name, %warning, "cannot read workspace.toml; using the defaults");
                }
                task_worker::preamble::RepoNote {
                    name: repo.name.clone(),
                    dir: repo.dir.to_string_lossy().into_owned(),
                    git: repo.is_git(),
                    branch: repo.branch().map(str::to_string),
                    base: repo.worktree.as_ref().map(|w| w.base.sha12()),
                    base_kind: repo.worktree.as_ref().map(|w| w.base.kind.as_str().to_string()),
                    description: config.workspace.description.clone(),
                    check: config.commands.check.clone(),
                    docs: config.outputs.docs.clone(),
                    deliverables: config.outputs.deliverables.clone(),
                }
            })
            .collect()
    }

    /// ADR-0043 D2 / D4: 計画 run に渡す「この案件のリポジトリ」（名前 / 種類 / 置き場 / `workspace.toml` の
    /// `description`）。ストアと `workspace.toml` を読むだけで、判断も LLM も無い。
    pub(super) fn project_repo_notes(
        &self,
        task: &Task,
    ) -> Result<Vec<task_worker::preamble::ProjectRepoNote>, DispatchError> {
        let repos =
            task_ops::delegate::project_repos(self.store.as_ref(), task).map_err(ops_to_store)?;
        Ok(repos
            .into_iter()
            .map(|repo| {
                let (location, local) = match &repo.location {
                    WorkspaceSpec::Local { path, .. } => {
                        (path.display().to_string(), Some(path.clone()))
                    }
                    WorkspaceSpec::Remote { cluster, path, .. } => {
                        (format!("{cluster}:{}", path.display()), None)
                    }
                };
                let description = local.as_deref().and_then(|dir| {
                    task_core::workspace_config::load_or_default(dir)
                        .0
                        .workspace
                        .description
                });
                task_worker::preamble::ProjectRepoNote {
                    name: repo.name,
                    kind: repo.kind.as_str().to_string(),
                    location,
                    description,
                    is_primary: repo.is_primary,
                }
            })
            .collect())
    }

    /// テスト用: Phase 49 のときの「1 つだけの worktree」の姿（`task_workspaces_for` の先頭）。
    #[cfg(test)]
    pub(super) fn local_worktree_for(&self, task: &Task) -> Option<task_worker::LocalWorktree> {
        self.task_workspaces_for(task)
            .and_then(|ws| ws.repos.into_iter().next())
            .and_then(|r| r.worktree)
    }

    pub(super) fn work_dir_for(&self, task: &Task) -> Option<PathBuf> {
        self.task_workspaces_for(task)
            .and_then(|ws| ws.cwd().map(Path::to_path_buf))
    }

    /// ADR-0036 D1: そのタスクの成果物ディレクトリ（`<dir>/artifacts` か `<dir>/.taskd/artifacts/<task_id>`）。
    /// 判定は `task_core::artifacts`（純粋関数）。アダプタ・レビュー・記憶の読み取りはすべてこれを使う。
    pub(super) fn artifacts_dir(&self, task: &Task, workspace_dir: &Path) -> PathBuf {
        task_core::artifacts::artifacts_dir_for(task, workspace_dir)
    }
}
