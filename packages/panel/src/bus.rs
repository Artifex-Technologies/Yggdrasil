//! The panel's D-Bus connections, on sockets it connects itself. zbus would connect them through the `blocking` crate's thread
//! pool, whose idle threads then wake twice a second for as long as the panel runs (blocking 1.7 keeps them); a socket handed to
//! zbus ready-made needs no pool. An address that is not a plain socket path is left to zbus.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use zbus::blocking::connection::Builder;

/// The session bus (DBUS_SESSION_BUS_ADDRESS).
pub fn session() -> zbus::Result<Builder<'static>> {
    match std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().as_deref().and_then(socket_path) {
        Some(p) => on(p),
        None => Builder::session(),
    }
}

/// The system bus (DBUS_SYSTEM_BUS_ADDRESS, or its usual socket).
pub fn system() -> zbus::Result<Builder<'static>> {
    match std::env::var("DBUS_SYSTEM_BUS_ADDRESS") {
        Ok(a) => match socket_path(&a) {
            Some(p) => on(p),
            None => Builder::system(),
        },
        Err(_) => on(PathBuf::from("/run/dbus/system_bus_socket")),
    }
}

fn on(path: PathBuf) -> zbus::Result<Builder<'static>> {
    let s = UnixStream::connect(path).map_err(|e| zbus::Error::InputOutput(std::sync::Arc::new(e)))?;
    Ok(Builder::async_io_unix_stream(s))
}

/// The socket of a D-Bus address when its first entry is `unix:path=...` (the value unescaped: %XX).
pub fn socket_path(address: &str) -> Option<PathBuf> {
    let first = address.split(';').next()?;
    let rest = first.strip_prefix("unix:")?;
    let value = rest.split(',').find_map(|kv| kv.strip_prefix("path="))?;
    let b = value.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    (!out.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_socket_addresses_are_read() {
        assert_eq!(socket_path("unix:path=/run/user/1000/bus"), Some("/run/user/1000/bus".into()));
        assert_eq!(socket_path("unix:path=/tmp/a%20b,guid=0123"), Some("/tmp/a b".into()));
        assert_eq!(socket_path("unix:guid=1,path=/x;tcp:host=h"), Some("/x".into()));
        assert_eq!(socket_path("unix:abstract=/tmp/dbus-x"), None, "left to zbus");
        assert_eq!(socket_path("tcp:host=localhost,port=1"), None);
        assert_eq!(socket_path("unix:path=%zz"), None);
    }
}
