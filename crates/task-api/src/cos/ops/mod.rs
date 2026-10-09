//! Domain registries. Each domain unit edits only its own file; this list stays fixed.
pub(crate) mod admin;
pub(crate) mod decisions;
pub(crate) mod projects;
pub(crate) mod surface;
pub(crate) mod tasks;

use super::operations::{DispatchEnv, Matched, OperationAudit};
use crate::problem::ApiProblem;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

type Dispatcher = fn(
    &SqliteStore,
    &DispatchEnv,
    &OperationAudit,
    Matched,
    &str,
    Value,
) -> Result<CosOperation, ApiProblem>;

pub(crate) struct Registry {
    /// Domain label matched against the ADR headings by the registry tests.
    #[cfg(test)]
    pub(crate) name: &'static str,
    pub(crate) allowed: &'static [(&'static str, &'static str, &'static str)],
    pub(crate) excluded: &'static [(&'static str, &'static str, &'static str)],
    pub(crate) dispatch: Dispatcher,
}

pub(crate) const REGISTRIES: &[Registry] = &[
    Registry {
        #[cfg(test)]
        name: "tasks",
        allowed: tasks::ALLOWED,
        excluded: tasks::EXCLUDED,
        dispatch: tasks::dispatch,
    },
    Registry {
        #[cfg(test)]
        name: "decisions",
        allowed: decisions::ALLOWED,
        excluded: decisions::EXCLUDED,
        dispatch: decisions::dispatch,
    },
    Registry {
        #[cfg(test)]
        name: "projects",
        allowed: projects::ALLOWED,
        excluded: projects::EXCLUDED,
        dispatch: projects::dispatch,
    },
    Registry {
        #[cfg(test)]
        name: "admin",
        allowed: admin::ALLOWED,
        excluded: admin::EXCLUDED,
        dispatch: admin::dispatch,
    },
    Registry {
        #[cfg(test)]
        name: "surface",
        allowed: surface::ALLOWED,
        excluded: surface::EXCLUDED,
        dispatch: surface::dispatch,
    },
];

#[cfg(test)]
mod tests;
