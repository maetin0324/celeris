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

#[cfg(test)]
mod tests {
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
    fn workspace_root_priority_cli_store_config() {
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
