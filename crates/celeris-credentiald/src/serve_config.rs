//! The daemon's config.toml is the sole source of the launcher UID.
//! Read only the broker's projection; never reflect TOML errors or values in logs.
use celeris_credentiald::Error;
use serde::Deserialize;

#[derive(Default, Deserialize)]
struct Browser {
    launcher_uid: Option<u32>,
}

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    browser: Browser,
}

pub(crate) struct Settings {
    pub allowed: Vec<u32>,
    pub launcher_uid: Option<u32>,
}

/// `serve --config <path> <daemon-pid>...`; no implicit config or UID override.
pub(crate) fn load(
    mut args: impl Iterator<Item = String>,
    broker_uid: u32,
) -> Result<Settings, Error> {
    if args.next().as_deref() != Some("--config") {
        return Err(Error::Invalid);
    }
    let path = args.next().ok_or(Error::Invalid)?;
    let allowed = args
        .map(|arg| arg.parse::<u32>().map_err(|_| Error::Invalid))
        .collect::<Result<Vec<_>, _>>()?;
    if allowed.is_empty() {
        return Err(Error::Invalid);
    }
    let raw = zeroize::Zeroizing::new(std::fs::read_to_string(path).map_err(|_| Error::Io)?);
    let config: Config = toml::from_str(&raw).map_err(|_| Error::Invalid)?;
    let launcher_uid = config.browser.launcher_uid;
    if launcher_uid.is_some_and(|uid| uid == 0 || uid == broker_uid) {
        return Err(Error::Invalid);
    }
    Ok(Settings {
        allowed,
        launcher_uid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(path: &std::path::Path, tail: &[&str]) -> std::vec::IntoIter<String> {
        ["--config".to_owned(), path.to_str().unwrap().to_owned()]
            .into_iter()
            .chain(tail.iter().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn launcher_uid_comes_from_daemon_config_and_tracks_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for uid in [995, 996] {
            std::fs::write(&path, format!(
                "[api]\nbind = '127.0.0.1:7710'\n[browser]\nruntime = 'launcher'\nlauncher_socket = '/run/launcher.sock'\nlauncher_uid = {uid}\n[browser.egress]\nmode = 'direct'\n"
            )).unwrap();
            let settings = load(args(&path, &["123", "456"]), 1000).unwrap();
            assert_eq!(settings.launcher_uid, Some(uid));
            assert_eq!(settings.allowed, [123, 456]);
        }
    }

    #[test]
    fn missing_uid_keeps_injection_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for raw in ["", "[browser]\nruntime = 'daemon'\n"] {
            std::fs::write(&path, raw).unwrap();
            assert_eq!(
                load(args(&path, &["123"]), 1000).unwrap().launcher_uid,
                None
            );
        }
    }

    #[test]
    fn unreadable_invalid_or_unsafe_config_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(matches!(load(args(&path, &["123"]), 1000), Err(Error::Io)));
        for raw in [
            "[browser",
            "[browser]\nlauncher_uid = '995'",
            "[browser]\nlauncher_uid = -1",
            "[browser]\nlauncher_uid = 4294967296",
            "[browser]\nlauncher_uid = 0",
            "[browser]\nlauncher_uid = 1000",
            "[browser]\nlauncher_uid = 995\nlauncher_uid = 996",
        ] {
            std::fs::write(&path, raw).unwrap();
            assert!(
                matches!(load(args(&path, &["123"]), 1000), Err(Error::Invalid)),
                "{raw}"
            );
        }
    }

    #[test]
    fn serve_requires_config_and_pids_and_rejects_removed_uid_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[browser]\nlauncher_uid = 995").unwrap();
        for tail in [
            vec![],
            vec!["123", "--launcher-uid=996"],
            vec!["--launcher-uid=995", "123"],
            vec!["--config", "other", "123"],
        ] {
            assert!(matches!(
                load(args(&path, &tail), 1000),
                Err(Error::Invalid)
            ));
        }
        for raw in [
            vec!["123"],
            vec!["--launcher-uid=995", "123"],
            vec!["--config"],
        ] {
            assert!(matches!(
                load(raw.into_iter().map(str::to_owned), 1000),
                Err(Error::Invalid)
            ));
        }
    }
}
