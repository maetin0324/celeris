use celeris_credentiald::{Broker, Error, ManualProvider, ipc};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::FromRawFd,
        unix::fs::{DirBuilderExt, MetadataExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroize;
fn private_dir(path: &Path) -> Result<(), Error> {
    if !path.exists() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| Error::Io)?;
    }
    let m = fs::symlink_metadata(path).map_err(|_| Error::Permission)?;
    if !m.is_dir()
        || m.is_symlink()
        || m.mode() & 0o777 != 0o700
        || m.uid() != unsafe { libc::geteuid() }
    {
        return Err(Error::Permission);
    }
    Ok(())
}
fn home() -> Result<PathBuf, Error> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(Error::Invalid)
}
/// ADR-0136: vault・audit の置き場。`CELERIS_CREDENTIALD_DATA_DIR`（絶対 path のみ）を書けばそれ、
/// 無ければ従来の `$HOME/.local/celeris/credentiald`。鍵（`~/.config/celeris/credentiald`）は動かさない。
fn data_dir(home: &Path, configured: Option<std::ffi::OsString>) -> Result<PathBuf, Error> {
    match configured.filter(|v| !v.is_empty()) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                Ok(dir)
            } else {
                Err(Error::Invalid)
            }
        }
        None => Ok(home.join(".local/celeris/credentiald")),
    }
}
fn runtime() -> Result<PathBuf, Error> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or(Error::Invalid)
}
fn run() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("init") => {
            if args.next().is_some() {
                return Err(Error::Invalid);
            }
            let h = home()?;
            let config = h.join(".config/celeris/credentiald");
            let data = data_dir(&h, std::env::var_os("CELERIS_CREDENTIALD_DATA_DIR"))?;
            private_dir(&config)?;
            private_dir(&data)?;
            let provider = ManualProvider::open(config.join("keys"), data.join("vault"))?;
            provider.initialize_key()
        }
        Some("serve") => {
            let h = home()?;
            let config = h.join(".config/celeris/credentiald");
            let data = data_dir(&h, std::env::var_os("CELERIS_CREDENTIALD_DATA_DIR"))?;
            private_dir(&config)?;
            private_dir(&data)?;
            let provider = ManualProvider::open(config.join("keys"), data.join("vault"))?;
            let broker = Arc::new(Broker::new(provider, data.join("audit"))?);
            let mut allowed = Vec::new();
            // ADR-0138 D-L: the launcher UID that production `Attested` admission checks
            // proofs against. Without it no session is admitted for injection.
            let mut launcher_uid = None;
            for arg in args {
                if let Some(uid) = arg.strip_prefix("--launcher-uid=") {
                    if launcher_uid.is_some() {
                        return Err(Error::Invalid);
                    }
                    launcher_uid = Some(uid.parse::<u32>().map_err(|_| Error::Invalid)?);
                    continue;
                }
                allowed.push(arg.parse::<u32>().map_err(|_| Error::Invalid)?)
            }
            if allowed.is_empty() {
                return Err(Error::Invalid);
            }
            ipc::serve_attested(broker, &runtime()?, allowed, launcher_uid)
        }
        Some("bridge") => {
            if args.next().is_some() {
                return Err(Error::Invalid);
            } // FD 3 is an inherited, private supervisor channel. It is not a CLI argument or environment secret.
            if unsafe { libc::fcntl(3, libc::F_GETFD) } < 0 {
                return Err(Error::Denied);
            }
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(3, stat.as_mut_ptr()) } != 0 {
                return Err(Error::Denied);
            }
            let stat = unsafe { stat.assume_init() };
            if stat.st_mode & libc::S_IFMT != libc::S_IFIFO
                && stat.st_mode & libc::S_IFMT != libc::S_IFSOCK
            {
                return Err(Error::Denied);
            }
            let mut binding = String::new();
            let fd = unsafe { fs::File::from_raw_fd(3) };
            fd.take(256)
                .read_to_string(&mut binding)
                .map_err(|_| Error::Io)?;
            if binding.len() != 64 || !binding.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Error::Denied);
            }
            let mut input = Vec::new();
            std::io::stdin()
                .take(65537)
                .read_to_end(&mut input)
                .map_err(|_| Error::Io)?;
            if input.len() > 65536 {
                return Err(Error::Invalid);
            }
            let mut out = ipc::bridge_request(&input, &binding, &ipc::resolve_socket(&runtime()?));
            binding.zeroize();
            input.zeroize();
            let result = std::io::stdout().write_all(&out).map_err(|_| Error::Io);
            out.zeroize();
            result
        }
        _ => Err(Error::Invalid),
    }
}
fn main() {
    if let Err(e) = run() {
        // Fixed error codes only: no request, path or secret is reflected.
        eprintln!("{}", e.code());
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_defaults_to_the_home_path_and_follows_the_env_override() {
        let home = Path::new("/home/u");
        assert_eq!(
            data_dir(home, None).expect("default"),
            PathBuf::from("/home/u/.local/celeris/credentiald")
        );
        assert_eq!(
            data_dir(home, Some("".into())).expect("empty means unset"),
            PathBuf::from("/home/u/.local/celeris/credentiald")
        );
        assert_eq!(
            data_dir(home, Some("/local/celeris/state/credentiald".into())).expect("override"),
            PathBuf::from("/local/celeris/state/credentiald")
        );
        assert!(matches!(
            data_dir(home, Some("relative/credentiald".into())),
            Err(Error::Invalid)
        ));
    }
}
