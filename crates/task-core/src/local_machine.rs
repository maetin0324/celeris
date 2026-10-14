//! ADR 2026-10-14-local-machine-settings D3/D5: ローカル設定の解決（CLI 上書き > store > 設定ファイル）と
//! task ごとの足回りの root の固定。
//!
//! - `workspace_root`: CLI の `--workspace-root` があれば store の値と `task_workspace_roots` の固定を
//!   **どちらも無視**して CLI の root を使う（staging の DB は本番の写しなので固定を信じない）。無ければ
//!   `local_settings.workspace_root`、それも無ければ設定ファイルの値。
//! - task の root（`WorkspaceRoots::task_root`）は固定があればそれ、無ければ設定ファイル（CLI があれば CLI）
//!   の root。固定は足回りを初めて用意するときに `WorkspaceRoots::pin_task_root` で 1 回だけ書く。
//!   root task は実効の root、子 task は親の root を継ぐ。
//! - `max_concurrency`: store の値があれば store、無ければ設定ファイル（CLI の上書きは無い）。

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::model::TaskId;
use crate::store::{LocalSettings, StoreError, TaskStore};

/// 実効値がどこから来たか（応答の `source`。`work_dir_source` の語彙に `cli` を足したもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SettingSource {
    Cli,
    Settings,
    Config,
}

/// 解決した値とその出所。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: SettingSource,
}

/// `workspace_root` の resolver。設定ファイルの root と CLI の上書き（あれば）を持ち、store の
/// 上書きと task ごとの固定は呼ぶたびに store から読む（daemon の再起動なしに効かせるため）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoots {
    config_root: PathBuf,
    cli_root: Option<PathBuf>,
}

impl WorkspaceRoots {
    pub fn new(config_root: PathBuf, cli_root: Option<PathBuf>) -> Self {
        Self {
            config_root,
            cli_root,
        }
    }

    /// 設定ファイルの root（上書きを消したときに戻る値。応答の `config_value`）。
    pub fn config_root(&self) -> &Path {
        &self.config_root
    }

    /// CLI の `--workspace-root`（あれば）。
    pub fn cli_root(&self) -> Option<&Path> {
        self.cli_root.as_deref()
    }

    /// 固定を持たない task の root（CLI があれば CLI、無ければ設定ファイル）。store の上書きは使わない
    /// （この ADR 以前の task は設定ファイルの root にあるため）。
    pub fn unpinned_root(&self) -> &Path {
        self.cli_root.as_deref().unwrap_or(&self.config_root)
    }

    /// 実効の root（CLI > store > 設定ファイル）。新しく足回りを用意する root task に使う。
    pub fn effective_root(&self, settings: Option<&LocalSettings>) -> Resolved<PathBuf> {
        if let Some(cli) = &self.cli_root {
            return Resolved {
                value: cli.clone(),
                source: SettingSource::Cli,
            };
        }
        match settings.and_then(|s| s.workspace_root.as_deref()) {
            Some(root) => Resolved {
                value: PathBuf::from(root),
                source: SettingSource::Settings,
            },
            None => Resolved {
                value: self.config_root.clone(),
                source: SettingSource::Config,
            },
        }
    }

    /// `effective_root` を store の今の上書きで解く。
    pub fn effective_root_in<S: TaskStore + ?Sized>(
        &self,
        store: &S,
    ) -> Result<Resolved<PathBuf>, StoreError> {
        if self.cli_root.is_some() {
            return Ok(self.effective_root(None));
        }
        Ok(self.effective_root(store.local_settings_get()?.as_ref()))
    }

    /// task の足回りの root（固定があればそれ、無ければ `unpinned_root`）。CLI の上書きがあれば
    /// 固定を無視して CLI の root。
    pub fn task_root<S: TaskStore + ?Sized>(
        &self,
        store: &S,
        task_id: &TaskId,
    ) -> Result<PathBuf, StoreError> {
        if let Some(cli) = &self.cli_root {
            return Ok(cli.clone());
        }
        Ok(match store.task_workspace_root_get(task_id)? {
            Some(root) => PathBuf::from(root),
            None => self.config_root.clone(),
        })
    }

    /// task の足回り `<root>/<task_id>`。
    pub fn task_dir<S: TaskStore + ?Sized>(
        &self,
        store: &S,
        task_id: &TaskId,
    ) -> Result<PathBuf, StoreError> {
        Ok(self.task_root(store, task_id)?.join(task_id.to_string()))
    }

    /// 足回りを初めて用意するときに root を固定する（`INSERT OR IGNORE`、既存の固定は変えない）。
    /// root task は実効の root、子 task（`parent_id` あり）は親の root（親に固定が無ければ親が解決される
    /// root）を継ぐ。CLI の上書きがあるときは固定を書かず CLI の root を返す（staging は本番の写しの DB に
    /// 書かない）。固定された root を返す。
    pub fn pin_task_root<S: TaskStore + ?Sized>(
        &self,
        store: &S,
        task_id: &TaskId,
        parent_id: Option<&TaskId>,
        now: OffsetDateTime,
    ) -> Result<PathBuf, StoreError> {
        if let Some(cli) = &self.cli_root {
            return Ok(cli.clone());
        }
        if let Some(existing) = store.task_workspace_root_get(task_id)? {
            return Ok(PathBuf::from(existing));
        }
        let root = match parent_id {
            Some(parent) => self.task_root(store, parent)?,
            None => self.effective_root_in(store)?.value,
        };
        let root = root.to_str().ok_or_else(|| {
            StoreError::Invalid(format!("workspace root is not UTF-8: {}", root.display()))
        })?;
        Ok(PathBuf::from(
            store.task_workspace_root_pin(task_id, root, now)?,
        ))
    }
}

/// `max_concurrency` の実効値（store > 設定ファイル）。
pub fn resolve_max_concurrency(
    config_value: usize,
    settings: Option<&LocalSettings>,
) -> Resolved<usize> {
    match settings.and_then(|s| s.max_concurrency) {
        Some(n) => Resolved {
            value: n as usize,
            source: SettingSource::Settings,
        },
        None => Resolved {
            value: config_value,
            source: SettingSource::Config,
        },
    }
}

/// ローカルマシンの行の予約 id（`GET /clusters` の `local.id`、`PUT /clusters/local/settings`）。
pub const LOCAL_CLUSTER_ID: &str = "local";

/// `max_concurrency` の許す範囲（ADR 2026-10-14-local-machine-settings D5 の 4）。API の検証と store が共有する。
pub const MAX_CONCURRENCY_RANGE: std::ops::RangeInclusive<u32> = 1..=256;

/// ADR-0095 付記 D-a の 3 / ADR-0126: `$HOME` 下の host の設定。db_guard が worker の namespace で読み取り専用に
/// し、`workspace_root` の検証（D5 の 3）が重なりを拒否する。両者はこの 1 つの定義を使う。
pub const HOST_PROTECTED_HOME_DIRS: [&str; 3] = [
    ".config/systemd",
    ".local/celeris/releases",
    ".config/celeris",
];

/// ADR-0136: host の release の置き場（`$HOME` の外の固定 path）。
pub const HOST_PROTECTED_ABS_DIRS: [&str; 1] = ["/local/celeris/state/releases"];

/// D5 の 3: `workspace_root` が指してはいけない場所。`overlap` は「同じ・その中・それを含む」を、`exact`
/// （`/` と `$HOME`）は「同じ」だけを拒否する。照合は canonicalize（末尾が未作成なら最も近い祖先）後の path。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtectedPaths {
    overlap: Vec<PathBuf>,
    exact: Vec<PathBuf>,
}

impl ProtectedPaths {
    /// daemon の保護 path 一式: DB の file とその dir、`[selfdeploy] releases_dir`、`HOST_PROTECTED_HOME_DIRS`、
    /// `HOST_PROTECTED_ABS_DIRS`、設定ファイルのある dir。`/` と `$HOME` は同じ path だけ拒否する。
    pub fn for_daemon(
        home: Option<&Path>,
        db: &Path,
        config_path: Option<&Path>,
        releases_dir: Option<&Path>,
    ) -> Self {
        let mut overlap = vec![db.to_path_buf()];
        overlap.extend(db.parent().map(Path::to_path_buf));
        overlap.extend(releases_dir.map(Path::to_path_buf));
        if let Some(home) = home {
            overlap.extend(HOST_PROTECTED_HOME_DIRS.iter().map(|rel| home.join(rel)));
        }
        overlap.extend(HOST_PROTECTED_ABS_DIRS.iter().map(PathBuf::from));
        overlap.extend(config_path.and_then(Path::parent).map(Path::to_path_buf));
        let mut exact = vec![PathBuf::from("/")];
        exact.extend(home.map(Path::to_path_buf));
        Self {
            overlap: overlap.iter().map(|p| resolve_lenient(p)).collect(),
            exact: exact.iter().map(|p| resolve_lenient(p)).collect(),
        }
    }

    /// 「同じ・中・含む」で拒否する path（canonicalize 済み）。
    pub fn overlap(&self) -> &[PathBuf] {
        &self.overlap
    }

    /// `path`（canonicalize 済み）が保護 path に当たれば、その保護 path を返す。
    fn hit(&self, path: &Path) -> Option<&Path> {
        self.exact
            .iter()
            .find(|p| p.as_path() == path)
            .or_else(|| {
                self.overlap
                    .iter()
                    .find(|p| path.starts_with(p) || p.starts_with(path))
            })
            .map(PathBuf::as_path)
    }
}

/// 存在する最も近い祖先を canonicalize し、残りの名前を足す（symlink 経由のすり抜けを防ぐ）。
/// 読めなければ字面のまま返す（検証側は字面でも重なりを見る）。
fn resolve_lenient(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            resolve_lenient(parent).join(name)
        }
        _ => path.to_path_buf(),
    }
}

/// D5 の 1〜3: PUT された `workspace_root` を検証し、保存する正規化済みの絶対 path を返す。
/// 1. `~` / `~/…` を `home` で展開し、絶対 path で `..` を含まないこと。
/// 2. 保護 path（`protected`）と重ならないこと（作る前と、作った後の canonicalize の両方で見る）。
/// 3. `create_dir_all` でき、一時 file を作って消せること。
///
/// 失敗は人が読む 1 行（API が 422 `validation`・`field: "workspace_root"` に写す）。
pub fn validate_workspace_root(
    input: &str,
    home: Option<&Path>,
    protected: &ProtectedPaths,
) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("workspace_root must not be empty".into());
    }
    let expanded = if trimmed == "~" || trimmed.starts_with("~/") {
        let home = home.ok_or("workspace_root uses ~ but the daemon has no $HOME")?;
        home.join(trimmed.trim_start_matches('~').trim_start_matches('/'))
    } else {
        PathBuf::from(trimmed)
    };
    if !expanded.is_absolute() {
        return Err("workspace_root must be an absolute path or ~ / ~/…".into());
    }
    let mut normalized = PathBuf::new();
    for component in expanded.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err("workspace_root must not contain `..`".into());
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    let reject = |hit: &Path| {
        format!(
            "workspace_root must not be, contain or be inside a protected path ({})",
            hit.display()
        )
    };
    if let Some(hit) = protected.hit(&resolve_lenient(&normalized)) {
        return Err(reject(hit));
    }
    std::fs::create_dir_all(&normalized).map_err(|e| {
        format!(
            "workspace_root {} cannot be created: {e}",
            normalized.display()
        )
    })?;
    let canonical = std::fs::canonicalize(&normalized).map_err(|e| {
        format!(
            "workspace_root {} cannot be resolved: {e}",
            normalized.display()
        )
    })?;
    if let Some(hit) = protected.hit(&canonical) {
        return Err(reject(hit));
    }
    let nanos = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let probe = canonical.join(format!(
        ".celeris-write-probe-{}-{nanos}",
        std::process::id()
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|_| std::fs::remove_file(&probe))
        .map_err(|e| {
            format!(
                "workspace_root {} is not writable: {e}",
                normalized.display()
            )
        })?;
    Ok(normalized)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::{LocalSettingsOverrides, SqliteStore};

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_791_939_600).unwrap()
    }

    fn set(store: &SqliteStore, root: Option<&str>, conc: Option<u32>) {
        store
            .local_settings_set(
                &LocalSettingsOverrides {
                    workspace_root: root.map(str::to_string),
                    max_concurrency: conc,
                },
                "admin",
                now(),
            )
            .unwrap();
    }

    #[test]
    pub(crate) fn workspace_root_priority_cli_store_config() {
        let store = SqliteStore::open_in_memory().unwrap();
        let config = WorkspaceRoots::new(PathBuf::from("/cfg/ws"), None);
        let cli = WorkspaceRoots::new(PathBuf::from("/cfg/ws"), Some(PathBuf::from("/cli/ws")));

        // 3. 設定ファイル（store に上書きなし）。
        let r = config.effective_root_in(&store).unwrap();
        assert_eq!(
            (r.value, r.source),
            (PathBuf::from("/cfg/ws"), SettingSource::Config)
        );
        let m = resolve_max_concurrency(4, store.local_settings_get().unwrap().as_ref());
        assert_eq!((m.value, m.source), (4, SettingSource::Config));

        // 2. store が設定ファイルに勝つ（欄ごと）。
        set(&store, Some("/store/ws"), None);
        let settings = store.local_settings_get().unwrap();
        let r = config.effective_root_in(&store).unwrap();
        assert_eq!(
            (r.value, r.source),
            (PathBuf::from("/store/ws"), SettingSource::Settings)
        );
        let m = resolve_max_concurrency(4, settings.as_ref());
        assert_eq!((m.value, m.source), (4, SettingSource::Config));
        set(&store, Some("/store/ws"), Some(9));
        let m = resolve_max_concurrency(4, store.local_settings_get().unwrap().as_ref());
        assert_eq!((m.value, m.source), (9, SettingSource::Settings));

        // 1. CLI が store に勝つ。
        let r = cli.effective_root_in(&store).unwrap();
        assert_eq!(
            (r.value, r.source),
            (PathBuf::from("/cli/ws"), SettingSource::Cli)
        );

        // task ごとの固定: 固定の無い既存 task は設定ファイルの root（store の値ではない）。
        let legacy = TaskId::new();
        assert_eq!(
            config.task_root(&store, &legacy).unwrap(),
            PathBuf::from("/cfg/ws")
        );

        // root task は固定の時点の実効の root、子 task は親の固定を継ぐ。
        let root_task = TaskId::new();
        let child = TaskId::new();
        assert_eq!(
            config
                .pin_task_root(&store, &root_task, None, now())
                .unwrap(),
            PathBuf::from("/store/ws")
        );
        // store を変えても既存の固定は動かず、子は親の root を継ぐ（新しい store 値ではない）。
        set(&store, Some("/store/ws2"), None);
        assert_eq!(
            config
                .pin_task_root(&store, &child, Some(&root_task), now())
                .unwrap(),
            PathBuf::from("/store/ws")
        );
        assert_eq!(
            config.task_root(&store, &root_task).unwrap(),
            PathBuf::from("/store/ws")
        );
        assert_eq!(
            config.task_dir(&store, &child).unwrap(),
            PathBuf::from("/store/ws").join(child.to_string())
        );
        // 固定の無い親の子は、親が解決される root（設定ファイル）を継ぐ。
        let legacy_child = TaskId::new();
        assert_eq!(
            config
                .pin_task_root(&store, &legacy_child, Some(&legacy), now())
                .unwrap(),
            PathBuf::from("/cfg/ws")
        );
        // 新しい root task は新しい store 値で固定される。
        let fresh = TaskId::new();
        assert_eq!(
            config.pin_task_root(&store, &fresh, None, now()).unwrap(),
            PathBuf::from("/store/ws2")
        );

        // CLI の上書き時は固定を無視し、固定も書かない。
        assert_eq!(
            cli.task_root(&store, &root_task).unwrap(),
            PathBuf::from("/cli/ws")
        );
        let staged = TaskId::new();
        assert_eq!(
            cli.pin_task_root(&store, &staged, None, now()).unwrap(),
            PathBuf::from("/cli/ws")
        );
        assert_eq!(store.task_workspace_root_get(&staged).unwrap(), None);
    }
}
