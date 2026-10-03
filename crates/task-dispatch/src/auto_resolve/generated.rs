use super::{ClassifiedPath, ResolveAttempt, ResolveContext};
use std::path::Path;

/// 次の葉が設定由来のコマンドで再生成する。
pub fn resolve(
    _repo: &Path,
    _ctx: &ResolveContext,
    _path: &ClassifiedPath,
) -> Result<ResolveAttempt, String> {
    Ok(ResolveAttempt::NotHandled {
        reason: "生成物の再作成は未実装".into(),
    })
}
