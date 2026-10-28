//! ADR 2026-10-28-cos-session-policy-key D1: the policy part of a CoS chat session key.
//!
//! `policy_version=<[cos.triage] policy_version>;<skill>=<sha256 of its tree>;…` (skills sorted by
//! name). The tree hash covers the regular files under the KB's `skills/<name>/` (relative path,
//! byte length and bytes, sorted by path); symlinks and special files are skipped, as delivery
//! does (`task_worker::skills`). No judgment here: equal strings mean "same policy".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The policy key of a run that mounts `skills` (`(name, KB directory)`) under `policy_version`.
pub(crate) fn policy_key(
    policy_version: &str,
    skills: &[(String, PathBuf)],
) -> std::io::Result<String> {
    let mut hashes = BTreeMap::new();
    for (name, dir) in skills {
        hashes.insert(name.as_str(), skill_tree_sha256(dir)?);
    }
    let mut key = format!("policy_version={policy_version}");
    for (name, hash) in hashes {
        key.push(';');
        key.push_str(name);
        key.push('=');
        key.push_str(&hash);
    }
    Ok(key)
}

/// SHA-256 of the regular files under `root` (see the module doc).
pub(crate) fn skill_tree_sha256(root: &Path) -> std::io::Result<String> {
    if !std::fs::symlink_metadata(root)?.is_dir() {
        return Err(std::io::Error::other(format!(
            "skill root {} is not a directory",
            root.display()
        )));
    }
    let mut files = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        for entry in std::fs::read_dir(root.join(&relative))? {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                files.insert(path, ());
            }
        }
    }
    let mut hash = Sha256::new();
    for path in files.into_keys() {
        let bytes = std::fs::read(root.join(&path))?;
        let name = path.as_os_str().as_encoded_bytes();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_key_follows_version_and_every_file_of_each_skill() {
        let dir = tempfile::tempdir().expect("tmp");
        let op = dir.path().join("cos-operator");
        let triage = dir.path().join("cos-inbox-triage");
        std::fs::create_dir_all(op.join("sub")).expect("mkdir");
        std::fs::create_dir_all(&triage).expect("mkdir");
        std::fs::write(op.join("SKILL.md"), "v5").expect("write");
        std::fs::write(op.join("sub/operations.md"), "ops").expect("write");
        std::fs::write(triage.join("SKILL.md"), "v6").expect("write");
        let skills = vec![
            ("cos-operator".to_string(), op.clone()),
            ("cos-inbox-triage".to_string(), triage.clone()),
        ];
        let base = policy_key("6", &skills).expect("key");
        assert!(
            base.starts_with("policy_version=6;cos-inbox-triage="),
            "{base}"
        );
        assert!(base.contains(";cos-operator="), "{base}");
        // Same inputs, same key (order of the mount list does not matter).
        let reversed: Vec<_> = skills.iter().rev().cloned().collect();
        assert_eq!(policy_key("6", &reversed).expect("key"), base);
        assert_ne!(policy_key("7", &skills).expect("key"), base);
        assert_ne!(policy_key("6", &skills[..1]).expect("key"), base);
        std::fs::write(op.join("sub/operations.md"), "ops2").expect("write");
        let changed = policy_key("6", &skills).expect("key");
        assert_ne!(changed, base);
        std::fs::rename(op.join("sub/operations.md"), op.join("sub/ops.md")).expect("mv");
        assert_ne!(policy_key("6", &skills).expect("key"), changed);
        assert!(policy_key("6", &[("x".into(), dir.path().join("missing"))]).is_err());
    }
}
