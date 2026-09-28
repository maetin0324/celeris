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
fn runtime() -> Result<PathBuf, Error> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or(Error::Invalid)
}
fn run() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("serve") => {
            let h = home()?;
            let config = h.join(".config/celeris/credentiald");
            let data = h.join(".local/celeris/credentiald");
            private_dir(&config)?;
            private_dir(&data)?;
            let provider = ManualProvider::open(config.join("keys"), data.join("vault"))?;
            let broker = Arc::new(Broker::new(provider, data.join("audit"))?);
            let mut allowed = Vec::new();
            for arg in args {
                allowed.push(arg.parse::<u32>().map_err(|_| Error::Invalid)?)
            }
            if allowed.is_empty() {
                return Err(Error::Invalid);
            }
            ipc::serve(broker, &runtime()?, allowed)
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
