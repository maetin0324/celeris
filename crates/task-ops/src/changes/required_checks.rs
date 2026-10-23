//! Read required checks from the immutable candidate, failing closed on unavailable definitions.
use super::{GIT_TIMEOUT, git};
use std::path::Path;

pub fn candidate_required_checks(repo: &Path, candidate: &str) -> Result<Vec<String>, String> {
    read_definition(candidate, |args| {
        let out = git(repo, args, GIT_TIMEOUT).ok_or("cannot start git")?;
        if out.ok {
            Ok(out.stdout)
        } else {
            Err(format!("git {}: {}", args.join(" "), out.why()))
        }
    })
}

fn read_definition(
    candidate: &str,
    mut read: impl FnMut(&[&str]) -> Result<String, String>,
) -> Result<Vec<String>, String> {
    let path = task_core::workspace_config::WORKSPACE_CONFIG_PATH;
    let object = format!("{candidate}:{path}");
    let text = match read(&["show", &object]) {
        Ok(text) => text,
        Err(error) => {
            // A failed cat-file alone cannot distinguish absence from repository corruption.
            // Confirm that the candidate tree can be listed and has no entry for this path.
            if read(&["cat-file", "-e", &object]).is_err()
                && read(&["ls-tree", "--name-only", candidate, "--", path])?
                    .trim()
                    .is_empty()
            {
                return Ok(Vec::new());
            }
            return Err(format!("cannot read required checks: {error}"));
        }
    };
    task_core::workspace_config::parse(&text)
        .map(|cfg| cfg.commands.check)
        .map_err(|e| format!("invalid required checks definition at {object}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_confirmed_absence_means_no_required_checks() {
        assert_eq!(
            read_definition("c", |args| match args[0] {
                "ls-tree" => Ok(String::new()),
                _ => Err("missing".into()),
            }),
            Ok(vec![])
        );
        assert!(read_definition("c", |_| Err("git unavailable".into())).is_err());
        assert!(
            read_definition("c", |args| match args[0] {
                "show" => Err("read failed".into()),
                _ => Ok("exists".into()),
            })
            .unwrap_err()
            .contains("read failed")
        );
        assert!(
            read_definition("c", |args| match args[0] {
                "ls-tree" => Ok(task_core::workspace_config::WORKSPACE_CONFIG_PATH.into()),
                _ => Err("object unavailable".into()),
            })
            .is_err()
        );
    }

    #[test]
    fn malformed_definition_is_rejected() {
        assert!(
            read_definition("c", |_| Ok("[commands\n".into()))
                .unwrap_err()
                .contains("invalid required checks")
        );
        assert_eq!(
            read_definition("c", |_| Ok("[commands]\ncheck = ['a', 'b']".into())),
            Ok(vec!["a".into(), "b".into()])
        );
    }
}
