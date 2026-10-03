use super::{ClassifiedPath, ResolveAttempt, ResolveContext};
use std::path::Path;

/// 次の葉が main 保護、全 branch 採番と参照追従を実装する。
pub fn resolve(
    _repo: &Path,
    _ctx: &ResolveContext,
    _path: &ClassifiedPath,
) -> Result<ResolveAttempt, String> {
    Ok(ResolveAttempt::NotHandled {
        reason: "番号の振り直しは未実装".into(),
    })
}
