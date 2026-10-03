use super::{ClassifiedPath, ConflictKind, ResolutionAction, ResolveAttempt, ResolveContext, git};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// base の各行を順に残している側だけを受け入れ、追加行を挿入位置ごとに分ける。
fn additions<'a>(base: &[&str], side: &[&'a str]) -> Option<Vec<Vec<&'a str>>> {
    let mut groups = vec![Vec::new(); base.len() + 1];
    let mut anchor = 0;
    for line in side {
        if anchor < base.len() && *line == base[anchor] {
            anchor += 1;
        } else {
            groups[anchor].push(*line);
        }
    }
    (anchor == base.len()).then_some(groups)
}

fn merge(base: &str, ours: &str, theirs: &str) -> Option<String> {
    let base = base.split_inclusive('\n').collect::<Vec<_>>();
    let ours = ours.split_inclusive('\n').collect::<Vec<_>>();
    let theirs = theirs.split_inclusive('\n').collect::<Vec<_>>();
    let ours = additions(&base, &ours)?;
    let theirs = additions(&base, &theirs)?;
    let mut merged = String::new();
    for anchor in 0..=base.len() {
        let mut shared = BTreeMap::<&str, usize>::new();
        for line in &ours[anchor] {
            merged.push_str(line);
            *shared.entry(line).or_default() += 1;
        }
        for line in &theirs[anchor] {
            // 同じ場所に加えた同じ行は、両側の最大出現回数だけ残す。
            if let Some(remaining) = shared.get_mut(line)
                && *remaining > 0
            {
                *remaining -= 1;
                continue;
            }
            merged.push_str(line);
        }
        if let Some(line) = base.get(anchor) {
            merged.push_str(line);
        }
    }
    Some(merged)
}

/// index の三者を読み、既存行を全て保持した追記だけを結合する。
pub fn resolve(
    repo: &Path,
    _ctx: &ResolveContext,
    path: &ClassifiedPath,
) -> Result<ResolveAttempt, String> {
    if path.kind != ConflictKind::Record {
        return Ok(ResolveAttempt::NotHandled {
            reason: "記録ファイルではない".into(),
        });
    }
    let read_stage = |stage| git(repo, &["show", &format!(":{stage}:{}", path.path)]);
    let (Ok(base), Ok(ours), Ok(theirs)) = (read_stage(1), read_stage(2), read_stage(3)) else {
        return Ok(ResolveAttempt::NotHandled {
            reason: "記録の三者比較ができない".into(),
        });
    };
    let Some(merged) = merge(&base, &ours, &theirs) else {
        return Ok(ResolveAttempt::NotHandled {
            reason: "記録の既存行が両側で変わった".into(),
        });
    };
    fs::write(repo.join(&path.path), merged)
        .map_err(|e| format!("記録 {} の書き込み: {e}", path.path))?;
    git(repo, &["add", "--", &path.path])?;
    Ok(ResolveAttempt::Handled {
        actions: vec![ResolutionAction {
            path: path.path.clone(),
            kind: ConflictKind::Record,
            detail: "両側の追記を ours、theirs の順に結合".into(),
        }],
    })
}

#[cfg(test)]
mod tests;
