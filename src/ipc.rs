//! The local connection through which the interface drives mpv and hears what it does:
//! a Unix socket, or a named pipe where Windows has no sockets of that kind.
//!
//! Neither direction makes the interface wait: a read waits for the player to say
//! something, and a write waits for it to listen, each in a thread of its own. The
//! interface takes what has arrived and goes on drawing.
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = std::fs::File;

/// Where mpv is told to listen, in the form that `--input-ipc-server` takes.
pub struct Address(std::ffi::OsString);

impl Address {
    /// Named after `directory`, which belongs to one playback and no other.
    pub fn new(directory: &Path) -> Self {
        #[cfg(unix)]
        let address = directory.join("ipc").into_os_string();
        #[cfg(windows)]
        let address = {
            // Windows keeps its pipes in a namespace of its own, not in a directory.
            let mut address = std::ffi::OsString::from(r"\\.\pipe\");
            address.push(directory.file_name().unwrap_or_default());
            address
        };
        Self(address)
    }

    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

/// A connection to a player that listens already.
pub struct Connection {
    outgoing: Sender<Vec<u8>>,
    incoming: Receiver<io::Result<Vec<u8>>>,
}

impl Connection {
    /// Connects to `address`; an error is also what a player yet to listen gives.
    pub fn connect(address: &Address) -> io::Result<Self> {
        let mut writer = open(address)?;
        let mut reader = writer.try_clone()?;
        let (events, incoming) = mpsc::channel();
        let (outgoing, commands) = mpsc::channel::<Vec<u8>>();
        let failures = events.clone();
        std::thread::spawn(move || {
            // The queue ends with the connection, and so does this thread.
            for bytes in commands {
                if let Err(error) = writer.write_all(&bytes) {
                    let _ = failures.send(Err(error));
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut bytes = [0; 8192];
            loop {
                let message = match reader.read(&mut bytes) {
                    // The player closed its end: nothing more will come.
                    Ok(0) => break,
                    Ok(count) => Ok(bytes[..count].to_vec()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => Err(error),
                };
                // A failure is passed on once, and then there is nothing left to read.
                let failed = message.is_err();
                if events.send(message).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self { outgoing, incoming })
    }

    /// Queues one command, which must end in the newline that mpv reads up to.
    pub fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.outgoing
            .send(bytes.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the player is gone"))
    }

    /// Appends what has arrived, waiting for nothing.
    pub fn receive(&mut self, buffer: &mut Vec<u8>) -> io::Result<()> {
        loop {
            match self.incoming.try_recv() {
                Ok(Ok(bytes)) => buffer.extend_from_slice(&bytes),
                Ok(Err(error)) => return Err(error),
                // Nothing for now, or the player is gone, which its exit status tells.
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }
}

#[cfg(unix)]
fn open(address: &Address) -> io::Result<Stream> {
    Stream::connect(address.as_os_str())
}

#[cfg(windows)]
fn open(address: &Address) -> io::Result<Stream> {
    // Opening the pipe by its name is how a client joins it; it must be there already.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(address.as_os_str())
}
