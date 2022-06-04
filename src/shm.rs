// MIT/Apache2 License

// unsafe code is quarantined into this module
#![allow(unsafe_code, unused_unsafe)]

use libc::{c_int, c_uint};
use std::{
    borrow::{Borrow, BorrowMut},
    io::{Error, Result},
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    ptr::{null_mut, slice_from_raw_parts_mut, NonNull},
};

macro_rules! syscall {
    ($expr: expr) => {{
        match ($expr) {
            -1 => return Err(Error::last_os_error()),
            a => a,
        }
    }};
    ($expr: expr, null) => {{
        match ($expr) {
            a if a.is_null() => return Err(Error::last_os_error()),
            a => a,
        }
    }};
}

/// An SHM segment allocated to be used in X11.
#[derive(Debug)]
pub(crate) struct ShmBlock {
    /// The ID associated with the SHM segment.
    shm_id: c_int,
    /// A pointer to the slice of memory associated with the SHM segment.
    ///
    /// Is a slice, so includes the size of the segment.
    ///
    /// # Safety
    ///
    /// While this struct is active, this will always point to a valid
    /// region of memory.
    ptr: NonNull<[u8]>,
    /// This segment is currently being read by the X server, so it
    /// cannot be modified.
    shared: bool,
}

/// A block of memory that uses SHM as a transport.
pub struct ShmTransport {
    /// The user-accessible block of memory.
    block: Box<[u8]>,
    /// The SHM segment associated with this block.
    segment: ShmBlock,
}

impl AsRef<[u8]> for ShmBlock {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: ptr is always a valid pointer to a slice of memory
        unsafe { self.ptr.as_ref() }
    }
}

impl AsMut<[u8]> for ShmBlock {
    fn as_mut(&mut self) -> &mut [u8] {
        if self.shared {
            panic!("Cannot modify shared segment");
        }

        // SAFETY: ptr isn't being read by the X server (or if it is,
        // we don't really care), so it is safe to modify it
        unsafe { self.ptr.as_mut() }
    }
}

impl Borrow<[u8]> for ShmBlock {
    fn borrow(&self) -> &[u8] {
        self.as_ref()
    }
}

impl BorrowMut<[u8]> for ShmBlock {
    fn borrow_mut(&mut self) -> &mut [u8] {
        self.as_mut()
    }
}

impl Deref for ShmBlock {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl DerefMut for ShmBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

impl Drop for ShmBlock {
    fn drop(&mut self) {
        // try to detach the process and them delete the segment
        unsafe {
            libc::shmdt(self.ptr.as_ptr() as *mut _);
            libc::shmctl(self.shm_id, libc::IPC_RMID, null_mut());
        }
    }
}

impl AsRef<[u8]> for ShmTransport {
    fn as_ref(&self) -> &[u8] {
        &self.block
    }
}

impl AsMut<[u8]> for ShmTransport {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.block
    }
}

impl Borrow<[u8]> for ShmTransport {
    fn borrow(&self) -> &[u8] {
        &self.block
    }
}

impl BorrowMut<[u8]> for ShmTransport {
    fn borrow_mut(&mut self) -> &mut [u8] {
        &mut self.block
    }
}

impl Deref for ShmTransport {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.block
    }
}

impl DerefMut for ShmTransport {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.block
    }
}

impl ShmBlock {
    /// Create a new SHM segment with the given length.
    pub fn new(len: usize) -> Result<ShmBlock> {
        // SAFETY: using flags 0744 (creator read/write, other read only)
        //         ensures that the rest of the safety guarantees we make
        //         are upheld
        unsafe { Self::with_flags(len, libc::S_IRWXU | libc::S_IRGRP | libc::S_IROTH) }
    }

    /// ## Safety
    ///
    /// This function requires that other guarantees are upheld if the
    /// `flags` are used beyond 0744.
    unsafe fn with_flags(len: usize, flags: c_uint) -> Result<ShmBlock> {
        // create the SHM ID
        let shm_id = syscall!(unsafe {
            libc::shmget(
                libc::IPC_PRIVATE,
                len as _,
                flags as c_int | libc::IPC_PRIVATE,
            )
        });

        // attach the SHM segment to an address
        let ptr = syscall!(unsafe { libc::shmat(shm_id, null_mut(), 0,) }, null);

        // let's create the end result
        Ok(ShmBlock {
            ptr: unsafe { NonNull::new_unchecked(slice_from_raw_parts_mut(ptr.cast(), len)) },
            shm_id,
            shared: false,
        })
    }

    /// Get the SHM ID associated with this segment.
    pub fn shm_id(&self) -> c_int {
        self.shm_id
    }

    /// Get the pointer to the memory associated with this segment.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr() as *const u8
    }

    /// Get the pointer to the memory associated with this segment.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr() as *mut u8
    }

    /// Get the length of the memory associated with this segment.
    pub fn len(&self) -> usize {
        self.as_ref().len()
    }

    /// Tell whether this item is empty.
    pub fn is_empty(&self) -> bool {
        self.as_ref().is_empty()
    }

    /// Tell whether this item is shared.
    pub fn is_shared(&self) -> bool {
        self.shared
    }

    /// Indicate that this segment is being used by the server.
    pub fn make_shared(&mut self) {
        self.shared = true;
    }

    /// Indicate that this segment is no longer being used by the server.
    ///
    /// # Safety
    ///
    /// The user must be entirely sure that the server is no longer using
    /// this segment.
    pub unsafe fn make_unshared(&mut self) {
        self.shared = false;
    }
}

impl ShmTransport {
    /// Create a new available SHM transport of the specified size.
    pub fn new(len: usize) -> Result<ShmTransport> {
        let block = vec![0; len].into_boxed_slice();

        // SAFETY: SHM transport is not exposed to the user, so we can make it
        //         server-writable
        let transport =
            unsafe { ShmBlock::with_flags(len, libc::S_IRWXU | libc::S_IRWXG | libc::S_IRWXO) }?;

        Ok(Self {
            block,
            segment: transport,
        })
    }

    /// Get the block of memory backing this transport.
    pub fn into_inner(self) -> Box<[u8]> {
        self.block
    }

    /// Get the ID of the segment associated with this transport.
    pub fn shm_id(&self) -> c_int {
        self.segment.shm_id()
    }

    pub(crate) unsafe fn segment(&self) -> &ShmBlock {
        &self.segment
    }

    pub(crate) unsafe fn segment_mut(&mut self) -> &mut ShmBlock {
        &mut self.segment
    }

    /// Repopulate the data in the block.
    ///
    /// # Safety
    ///
    /// SHM block must not be in use by the server.
    pub unsafe fn repopulate(&mut self) {
        self.block.copy_from_slice(&self.segment);
    }
}
