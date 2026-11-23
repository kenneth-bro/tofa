//! Local transport between the macOS application and TOFA's stdio MCP process.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_MESSAGE: u64 = 4096;

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub account: String,
    pub expected_vault: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub enum Reply {
    Code {
        code: String,
        expires_in_seconds: u64,
    },
    Error(String),
}

fn runtime_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("tofa").join("mcp-runtime")
}

fn check_private(metadata: &fs::Metadata, expected_type: bool) -> io::Result<()> {
    // geteuid has no pointer arguments and only reads the process user identity.
    if !expected_type
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "untrusted TOFA MCP socket location",
        ));
    }
    Ok(())
}

pub fn listen(config_dir: &Path) -> io::Result<(UnixListener, File)> {
    let dir = runtime_dir(config_dir);
    fs::create_dir_all(dir.parent().expect("runtime directory has a parent"))?;
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(&dir)?;
    check_private(&metadata, metadata.is_dir())?;

    // Keep one live listener per user and replace stale sockets only while holding the lock.
    let guard = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("listener.lock"))?;
    guard.try_lock()?;
    let path = dir.join("bridge.sock");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&path)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "unexpected TOFA MCP socket entry",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, Permissions::from_mode(0o600))?;
    Ok((listener, guard))
}

pub fn request(config_dir: &Path, request: &Request) -> io::Result<Reply> {
    let dir = runtime_dir(config_dir);
    let metadata = fs::symlink_metadata(&dir)?;
    check_private(&metadata, metadata.is_dir())?;
    let path = dir.join("bridge.sock");
    let metadata = fs::symlink_metadata(&path)?;
    check_private(&metadata, metadata.file_type().is_socket())?;
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    write_message(&stream, request)?;
    read_message(&stream)
}

pub fn respond(stream: &UnixStream, handler: impl FnOnce(Request) -> Reply) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let request = read_message(stream)?;
    write_message(stream, &handler(request))
}

fn read_message<T: DeserializeOwned>(stream: &UnixStream) -> io::Result<T> {
    let mut data = Vec::new();
    BufReader::new(stream.take(MAX_MESSAGE + 1)).read_until(b'\n', &mut data)?;
    if data.len() as u64 > MAX_MESSAGE || data.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid TOFA MCP frame",
        ));
    }
    Ok(serde_json::from_slice(&data)?)
}

fn write_message(mut stream: &UnixStream, value: &impl Serialize) -> io::Result<()> {
    let mut data = serde_json::to_vec(value)?;
    data.push(b'\n');
    stream.write_all(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_socket_allows_one_listener_and_reopens_after_exit() {
        let temp = tempfile::Builder::new()
            .prefix("temp-mcp-")
            .tempdir()
            .unwrap();
        let config = temp.path();
        let (listener, guard) = listen(config).unwrap();
        assert!(listen(config).is_err());
        let socket = runtime_dir(config).join("bridge.sock");
        assert_eq!(fs::symlink_metadata(&socket).unwrap().mode() & 0o777, 0o600);

        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            respond(&stream, |request| Reply::Error(request.account)).unwrap();
        });
        let call = Request {
            account: "fixture".into(),
            expected_vault: None,
        };
        assert!(matches!(request(config, &call).unwrap(), Reply::Error(text) if text == "fixture"));
        server.join().unwrap();
        assert!(request(config, &call).is_err());
        drop(guard);
        assert!(listen(config).is_ok());
    }
}
