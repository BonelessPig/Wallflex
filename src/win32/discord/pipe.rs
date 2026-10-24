//! The Discord client's local pipe, `\\.\pipe\discord-ipc-N`. Opened for
//! overlapped I/O so a reader thread can wait on it while the session
//! thread writes.

use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING,
    ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::CreateEventW;
use windows::core::{HSTRING, PCWSTR};

/// Discord listens on the first free of these.
const PIPE_COUNT: u32 = 10;

/// An open pipe connection, closed on drop. The handle may be used from
/// several threads (kernel handles aren't tied to a thread, and each
/// operation has its own OVERLAPPED and event).
pub struct Pipe(HANDLE);

// SAFETY: a kernel handle can be used from any thread; every operation
// below uses its own OVERLAPPED structure and event, so concurrent reads
// and writes don't share state.
unsafe impl Send for Pipe {}
// SAFETY: as above; `&Pipe` only exposes thread-safe operations.
unsafe impl Sync for Pipe {}

impl Pipe {
    /// Connects to the first Discord pipe that answers.
    ///
    /// # Errors
    ///
    /// A message when Discord isn't running.
    pub fn connect() -> Result<Self, String> {
        let mut last = String::from("no pipe found");
        for index in 0..PIPE_COUNT {
            let name = HSTRING::from(format!(r"\\.\pipe\discord-ipc-{index}"));
            // SAFETY: `name` is null-terminated and outlives the call.
            let opened = unsafe {
                CreateFileW(
                    &name,
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            };
            match opened {
                Ok(handle) => return Ok(Self(handle)),
                Err(error) => last = error.message(),
            }
        }
        Err(format!("Discord isn't running ({last})"))
    }

    /// Writes all of `bytes`.
    ///
    /// # Errors
    ///
    /// A message when the pipe is closed.
    pub fn write_all(&self, bytes: &[u8]) -> Result<(), String> {
        let mut done = 0;
        while let Some(rest) = bytes.get(done..).filter(|r| !r.is_empty()) {
            let written = self.transfer(Direction::Write(rest))?;
            done = done.saturating_add(written);
        }
        Ok(())
    }

    /// Fills `buffer`, waiting as long as it takes.
    ///
    /// # Errors
    ///
    /// A message when the pipe closes or the read is cancelled.
    pub fn read_exact(&self, buffer: &mut [u8]) -> Result<(), String> {
        let mut done = 0;
        while let Some(rest) = buffer.get_mut(done..).filter(|r| !r.is_empty())
        {
            let read = self.transfer(Direction::Read(rest))?;
            if read == 0 {
                return Err("pipe closed".into());
            }
            done = done.saturating_add(read);
        }
        Ok(())
    }

    /// Cancels pending reads and writes, so a blocked reader returns.
    pub fn cancel(&self) {
        // SAFETY: the handle is valid until drop. Failure means nothing
        // was pending.
        let _ = unsafe { CancelIoEx(self.0, None) };
    }

    /// One overlapped read or write, waited for. Returns the bytes moved.
    fn transfer(&self, direction: Direction<'_>) -> Result<usize, String> {
        // SAFETY: a manual-reset event, unnamed, initially clear.
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map_err(|e| e.to_string())?;
        let event = Event(event);
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        // SAFETY: the buffer and OVERLAPPED stay alive until
        // GetOverlappedResult below has waited for the operation.
        let started = unsafe {
            match direction {
                Direction::Read(buffer) => ReadFile(
                    self.0,
                    Some(buffer),
                    None,
                    Some(&raw mut overlapped),
                ),
                Direction::Write(bytes) => WriteFile(
                    self.0,
                    Some(bytes),
                    None,
                    Some(&raw mut overlapped),
                ),
            }
        };
        if let Err(error) = started
            && error.code() != ERROR_IO_PENDING.to_hresult()
        {
            return Err(error.message());
        }
        let mut moved = 0_u32;
        // SAFETY: waits for the operation started above to finish.
        unsafe {
            GetOverlappedResult(
                self.0,
                &raw const overlapped,
                &raw mut moved,
                true,
            )
        }
        .map_err(|e| e.message())?;
        usize::try_from(moved).map_err(|e| e.to_string())
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A read into, or a write from, a buffer.
enum Direction<'a> {
    /// Read into this buffer.
    Read(&'a mut [u8]),
    /// Write these bytes.
    Write(&'a [u8]),
}

/// An event handle, closed on drop.
struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateEventW and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}
