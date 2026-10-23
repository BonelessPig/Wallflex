//! SSD temperatures from the storage driver's temperature query. Hard
//! drives are skipped so polling never wakes a sleeping disk.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_ID,
    STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
    StorageDeviceSeekPenaltyProperty, StorageDeviceTemperatureProperty,
};
use windows::core::PCWSTR;

use super::super::struct_size;
use crate::sensors::{
    DiskTemp, composite_temperature, has_seek_penalty, product_name,
};

/// Temperatures change slowly; polling more often wastes work.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Windows numbers physical drives from 0; 16 covers any normal desktop.
const MAX_PHYSICAL_DRIVES: u32 = 16;
/// Big enough for every descriptor queried here.
const DESCRIPTOR_BUFFER_BYTES: u32 = 1024;
/// Opening with no access rights is enough for these queries, and
/// doesn't need admin.
const NO_ACCESS: u32 = 0;

/// Polls SSD temperatures every [`POLL_INTERVAL`] and caches them.
#[derive(Default)]
pub struct DiskReader {
    /// The last results.
    cached: Vec<DiskTemp>,
    /// When they were read.
    last_poll: Option<Instant>,
}

impl DiskReader {
    /// SSD temperatures, at most [`POLL_INTERVAL`] old.
    pub fn read(&mut self) -> Vec<DiskTemp> {
        let fresh = self
            .last_poll
            .is_some_and(|then| then.elapsed() < POLL_INTERVAL);
        if !fresh {
            self.last_poll = Some(Instant::now());
            self.cached =
                (0..MAX_PHYSICAL_DRIVES).filter_map(read_drive).collect();
        }
        self.cached.clone()
    }
}

/// Closes a handle on drop.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// One drive's temperature, or `None` for a missing drive, a hard drive
/// or a drive that doesn't report it.
fn read_drive(number: u32) -> Option<DiskTemp> {
    let path: Vec<u16> = format!(r"\\.\PhysicalDrive{number}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `path` is null-terminated and outlives the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            NO_ACCESS,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .ok()?;
    let handle = OwnedHandle(handle);

    // Only SSDs: asking a hard drive could spin it up from sleep.
    let seek = query(handle.0, StorageDeviceSeekPenaltyProperty)?;
    if has_seek_penalty(&seek) != Some(false) {
        return None;
    }
    let temp_c = composite_temperature(&query(
        handle.0,
        StorageDeviceTemperatureProperty,
    )?)?;
    let name = query(handle.0, StorageDeviceProperty)
        .as_deref()
        .and_then(product_name);
    Some(DiskTemp {
        disk: number,
        name,
        temp_c,
    })
}

/// Runs a standard storage property query; returns the descriptor bytes.
fn query(handle: HANDLE, property: STORAGE_PROPERTY_ID) -> Option<Vec<u8>> {
    let request = STORAGE_PROPERTY_QUERY {
        PropertyId: property,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut buffer = vec![0_u8; usize::try_from(DESCRIPTOR_BUFFER_BYTES).ok()?];
    let mut returned = 0_u32;
    // SAFETY: the in and out pointers and sizes describe live buffers,
    // and the call is synchronous (no OVERLAPPED).
    unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(std::ptr::from_ref(&request).cast()),
            struct_size::<STORAGE_PROPERTY_QUERY>(),
            Some(buffer.as_mut_ptr().cast()),
            DESCRIPTOR_BUFFER_BYTES,
            Some(&raw mut returned),
            None,
        )
    }
    .ok()?;
    buffer.truncate(usize::try_from(returned).ok()?);
    Some(buffer)
}
