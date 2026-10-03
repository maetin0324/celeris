use super::{ClassifiedPath, ResolveAttempt, ResolveContext};
use std::path::Path;

/// 次の葉が base/ours/theirs の末尾追記だけを結合する。
pub fn resolve(
    _repo: &Path,
    _ctx: &ResolveContext,
    _path: &ClassifiedPath,
) -> Result<ResolveAttempt, String> {
    Ok(ResolveAttempt::NotHandled {
        reason: "追記の結合は未実装".into(),
    })
}
