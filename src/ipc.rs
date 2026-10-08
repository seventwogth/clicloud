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
use windows_pipe::Stream;

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
    Stream::open(address.as_os_str())
}

#[cfg(windows)]
mod windows_pipe {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, ERROR_IO_PENDING, HANDLE},
        Storage::FileSystem::{FILE_FLAG_OVERLAPPED, ReadFile, WriteFile},
        System::{
            IO::{GetOverlappedResult, OVERLAPPED},
            Threading::CreateEventW,
        },
    };

    pub struct Stream(std::fs::File);

    impl Stream {
        pub fn open(address: &OsStr) -> io::Result<Self> {
            // Synchronous Windows handles serialize reads and writes even across
            // duplicates: an idle read would prevent sending mpv its next command.
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(FILE_FLAG_OVERLAPPED)
                .open(address)
                .map(Self)
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            self.0.try_clone().map(Self)
        }

        fn transfer(
            &self,
            start: impl FnOnce(HANDLE, *mut OVERLAPPED, *mut u32) -> i32,
        ) -> io::Result<usize> {
            // SAFETY: no security attributes or name; the returned handle is owned
            // here and kept alive until this operation has completed.
            let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: CreateEventW succeeded and ownership has not been transferred.
            let event = unsafe { OwnedHandle::from_raw_handle(event) };
            let mut overlapped = OVERLAPPED {
                hEvent: event.as_raw_handle(),
                ..Default::default()
            };
            let mut count = 0;
            let handle = self.0.as_raw_handle();
            if start(handle, &mut overlapped, &mut count) == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                    return Err(error);
                }
                // SAFETY: handle, buffer (borrowed by the caller), OVERLAPPED and
                // event all remain alive. Wait for completion before releasing any
                // of them. Each direction uses its own event and OVERLAPPED.
                if unsafe { GetOverlappedResult(handle, &overlapped, &mut count, 1) } == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(count as usize)
        }
    }

    impl Read for Stream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let length = buffer.len().min(u32::MAX as usize) as u32;
            let result = self.transfer(|handle, overlapped, count| {
                // SAFETY: writable buffer spans length bytes and stays borrowed
                // until transfer has waited for this read to finish.
                unsafe { ReadFile(handle, buffer.as_mut_ptr(), length, count, overlapped) }
            });
            match result {
                Err(error) if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
                other => other,
            }
        }
    }

    impl Write for Stream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let length = buffer.len().min(u32::MAX as usize) as u32;
            self.transfer(|handle, overlapped, count| {
                // SAFETY: readable buffer spans length bytes and stays borrowed
                // until transfer has waited for this write to finish.
                unsafe { WriteFile(handle, buffer.as_ptr(), length, count, overlapped) }
            })
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
