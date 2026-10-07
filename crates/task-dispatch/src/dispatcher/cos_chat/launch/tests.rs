//! The env every CoS chat/triage run gets (`cos_run_env`, passed to `with_env` at launch).

use super::{COS_API_URL_ENV, COS_RUN_CREDENTIAL_ENV, cos_run_env, path_with_first};
use std::path::{Path, PathBuf};

fn value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[test]
fn cos_chat_run_launch_sets_api_url_env_and_path() {
    let env = cos_run_env("http://127.0.0.1:7700/api/v1", "celeris-cos-run.x");
    assert_eq!(
        value(&env, COS_API_URL_ENV),
        Some("http://127.0.0.1:7700/api/v1")
    );
    assert_eq!(
        value(&env, COS_RUN_CREDENTIAL_ENV),
        Some("celeris-cos-run.x")
    );

    // The daemon's own executable dir (where its release's celerisctl lives) comes first, once.
    let exe_dir: PathBuf = std::env::current_exe()
        .expect("exe")
        .parent()
        .expect("exe dir")
        .to_path_buf();
    let path = value(&env, "PATH").expect("PATH set");
    let entries: Vec<_> = std::env::split_paths(path).collect();
    assert_eq!(entries.first(), Some(&exe_dir), "PATH={path}");
    assert_eq!(entries.iter().filter(|p| **p == exe_dir).count(), 1);
    // The inherited PATH stays behind it, so `sh` and the harness CLIs still resolve.
    if let Some(inherited) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&inherited).filter(|p| *p != exe_dir) {
            assert!(entries.contains(&dir), "{dir:?} dropped from PATH={path}");
        }
    }

    // No `[api] listen`: no CELERIS_API_URL; the prompt's own explanation applies.
    let env = cos_run_env("", "celeris-cos-run.x");
    assert!(value(&env, COS_API_URL_ENV).is_none());
    assert_eq!(
        value(&env, COS_RUN_CREDENTIAL_ENV),
        Some("celeris-cos-run.x")
    );

    let already = path_with_first(
        Some(Path::new("/rel/bin")),
        Some("/rel/bin:/usr/bin:/rel/bin".into()),
    );
    assert_eq!(already.as_deref(), Some("/rel/bin:/usr/bin"));
    assert_eq!(path_with_first(None, Some("/usr/bin".into())), None);
}
