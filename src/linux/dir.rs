use core::{
    borrow::{Borrow, BorrowMut},
    mem::{ManuallyDrop, MaybeUninit},
    ops::{Deref, DerefMut},
};

pub use crate::{CStr, Errno, RawFd};

use linux_defs::{SeekWhence, O};
use linux_stat::CURRENT_DIRECTORY;
use linux_syscalls::{syscall, Sysno};

/// An object providing access to an open directory on the filesystem.
///
/// Dirs are automatically closed when they go out of scope.
/// Errors detected on closing are ignored by the implementation of Drop.
pub struct Dir {
    fd: RawFd,
    tell: u64,
}

impl Dir {
    /// Attempts to open a directory by a `path` relative to `dir`.
    pub fn open_at(dir: &Dir, path: &CStr) -> Result<Self, Errno> {
        let dir = dir.as_raw_fd();
        let path = path.as_ptr();
        let flags = (O::RDONLY | O::DIRECTORY | O::CLOEXEC).bits();

        loop {
            match unsafe { syscall!([ro] Sysno::openat, dir, path, flags, 0o666) } {
                Err(Errno::EINTR) => (),
                Err(err) => return Err(err),
                Ok(x) => {
                    return Ok(Dir {
                        fd: x as RawFd,
                        tell: 0,
                    })
                }
            }
        }
    }

    /// Attempts to open a directory by a `path` relative to
    /// current working directory.
    #[inline]
    pub fn open(file: &CStr) -> Result<Self, Errno> {
        let dir = ManuallyDrop::new(unsafe { Dir::from_raw_fd(CURRENT_DIRECTORY) });
        Self::open_at(&dir, file)
    }

    /// Constructs a new instance of [Dir] from the given raw file descriptor.
    ///
    /// # Safety
    ///
    /// the `fd` passed in must be a valid and open file descriptor.
    #[inline]
    pub const unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self { fd, tell: 0 }
    }

    /// Extract the raw file descriptor.
    #[inline]
    pub const fn as_raw_fd(&self) -> RawFd {
        self.fd
    }

    /// Constructs a new [DirIterator].
    #[inline]
    pub fn iter<'a, B: DirentBuf>(
        &'a mut self,
        buf: &'a mut B,
    ) -> Result<DirIterator<'a, B>, Errno> {
        DirIterator::new(self, buf)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        _ = unsafe { syscall!([ro] Sysno::close, self.fd) };
    }
}

/// A [DirentBuf] is a type of buffer which can handle filesystem paths and
/// [DirEntry] buffers.
pub trait DirentBuf:
    Deref<Target = [u8]> + DerefMut + AsRef<[u8]> + AsMut<[u8]> + Borrow<[u8]> + BorrowMut<[u8]>
{
    /// Clers the buffer, removing all values.
    fn reset(&mut self);

    /// Reserves capacity for at least `size` elements to be inserted in the given buffer.
    /// The buffer may reserve more space to speculatively avoid frequent reallocations. After
    /// calling `reserve`, capacity will be greater than or equal to `size`.
    fn reserve(&mut self, size: usize) -> Result<(), Errno>;

    /// Returns a raw pointer to the buffer, or a dangling raw pointer valid for sized reads if the
    /// buffer didn't allocate.
    fn as_ptr(&self) -> *const u8;

    /// Returns an unsafe mutable pointer to the buffer, or a dangling raw pointer valid for zero
    /// sized reads if the vector didn't allocate.
    fn as_mut_ptr(&mut self) -> *mut u8;

    /// Extracts a slice containing the entire buffer.
    #[inline]
    fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.as_ptr(), self.len()) }
    }

    /// Extracts a mutable slice containing the entire buffer.
    #[inline]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.as_mut_ptr(), self.len()) }
    }

    /// Returns the total number of elements the vector can hold without reallocating.
    fn capacity(&self) -> usize;

    /// Returns the number of elements in the vector, also referred to as its 'length'.
    fn len(&self) -> usize;

    /// Returns `true` if the buffer contains no elements.
    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forces the length of the vector to `new_len`.
    ///
    /// # Safety
    ///
    /// - `new_len` must be less than or equal to [Self::capacity()]
    /// - The elements at `old_len..new_len` must be initialized.
    unsafe fn set_len(&mut self, new_len: usize);

    /// Shrinks the capacity of the buffer as much as possible.
    fn shrink_to_fit(&mut self);

    /// Clones and appends all elements in a slice to the buffer.
    fn push_slice(&mut self, slice: &[u8]) -> Result<(), Errno> {
        let new_len = self.len() + slice.len();
        self.reserve(new_len)?;

        unsafe {
            core::ptr::copy_nonoverlapping(
                slice.as_ptr(),
                self.as_mut_ptr().add(self.len()),
                slice.len(),
            );
            self.set_len(new_len);
        }

        Ok(())
    }

    /// Clones and appends all elements in a [CStr] to the buffer.
    #[inline]
    fn push_c_str(&mut self, s: &CStr) -> Result<(), Errno> {
        self.push_slice(s.to_bytes())
    }
}

/// File type for the [DirEntry] structure.
#[repr(u8)]
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(dead_code)]
pub enum DirentFileType {
    Unknown = 0,
    /// FIFO pipe.
    Fifo = 1,
    /// Character device.
    Character = 2,
    /// Directory.
    Directory = 4,
    /// Block device.
    Block = 6,
    /// Regular file.
    Regular = 8,
    /// Link file.
    Link = 10,
    /// Unix socket.
    Socket = 12,
    Wht = 14,
}

impl From<DirentFileType> for linux_stat::FileType {
    fn from(value: DirentFileType) -> Self {
        match value {
            DirentFileType::Character => Self::Character,
            DirentFileType::Directory => Self::Directory,
            DirentFileType::Fifo => Self::Fifo,
            DirentFileType::Block => Self::Block,
            DirentFileType::Regular => Self::Regular,
            DirentFileType::Link => Self::Link,
            DirentFileType::Socket => Self::Socket,
            _ => Self::Unknown,
        }
    }
}

/// A type representing a directory entry, returned by [DirIterator].
#[repr(packed)]
#[allow(dead_code)]
pub struct DirEntry {
    ino: u64,
    off: u64,
    reclen: core::ffi::c_ushort,
    r#type: DirentFileType,
    name: [u8; 0],
}

/// An iterator over a filesystem directory.
pub struct DirIterator<'a, B: DirentBuf> {
    dir: &'a mut Dir,
    buf: &'a mut B,
    offset: usize,
}

impl<'a, B: DirentBuf> DirIterator<'a, B> {
    /// Creates a new iterator over directory `dir` using `buf` as a buffer.
    #[inline]
    pub fn new(dir: &'a mut Dir, buf: &'a mut B) -> Result<Self, Errno> {
        if dir.tell != 0 {
            loop {
                match unsafe { syscall!([ro] Sysno::lseek, dir.fd, dir.tell, SeekWhence::SET) } {
                    Err(Errno::EINTR) => (),
                    Err(err) => return Err(err),
                    Ok(_) => break,
                }
            }
        }

        Ok(Self {
            dir,
            buf,
            offset: 0,
        })
    }

    fn buffer(&self) -> &[u8] {
        let len = self.buf.len() - self.offset;
        unsafe { core::slice::from_raw_parts(self.buf.as_ptr().add(self.offset), len) }
    }
}

impl<'a, B: DirentBuf> Iterator for DirIterator<'a, B> {
    type Item = Result<&'a DirEntry, Errno>;

    fn next(&mut self) -> Option<Self::Item> {
        #[inline(always)]
        unsafe fn getdents64<B: DirentBuf>(fd: RawFd, buf: &mut B) -> Result<(), Errno> {
            buf.reset();
            loop {
                match syscall!(Sysno::getdents64, fd, buf.as_mut_ptr(), buf.capacity()) {
                    Err(Errno::EINVAL) => {
                        buf.reserve(buf.capacity() * 3 / 2)?;
                    }
                    Err(Errno::EINTR) => (),
                    Err(err) => return Err(err),
                    Ok(len) => {
                        buf.set_len(len);
                        return Ok(());
                    }
                }
            }
        }

        unsafe {
            let mut buf = self.buffer();

            if buf.len() < core::mem::size_of::<DirEntry>() {
                self.offset = 0;
                if let Err(err) = getdents64(self.dir.fd, self.buf) {
                    return Some(Err(err));
                }
                buf = self.buffer();
            }

            if buf.len() < core::mem::size_of::<DirEntry>() {
                None
            } else {
                let res: &'a DirEntry = &*(buf.as_ptr().cast());
                self.offset += res.len();
                self.dir.tell = res.offset();
                Some(Ok(res))
            }
        }
    }
}

impl DirEntry {
    /// Returns the inode for the entry.
    #[inline]
    pub const fn inode(&self) -> u64 {
        self.ino
    }

    /// Returns the offset to the next directory entry.
    #[inline]
    const fn offset(&self) -> u64 {
        self.off
    }

    /// Returns the file type for the entry.
    #[inline]
    pub const fn file_type(&self) -> DirentFileType {
        self.r#type
    }

    /// Returns the file name for the entry.
    #[inline]
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.name.as_ptr().cast()) }
    }

    /// Returns the total size of the entry.
    #[inline]
    pub const fn len(&self) -> usize {
        self.reclen as usize
    }

    /// Returns true if the total size of the entry is `0`.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A [DirentBuf] backed by a [u8] array.
pub struct ArrayBuffer<const N: usize> {
    mem: MaybeUninit<[u8; N]>,
    len: usize,
}

/// A [DirentBuf] backed by a [`Vec<u8>`].
#[cfg(feature = "std")]
pub struct VecBuffer {
    mem: Vec<u8>,
}

/// A [DirentBuf] backed by a `malloc`ated [u8] array.
#[cfg(feature = "c")]
pub struct CBuffer {
    mem: *mut u8,
    len: usize,
    capacity: usize,
}

impl<const N: usize> ArrayBuffer<N> {
    /// Creates a new instance of [Self].
    #[inline]
    pub const fn new() -> Self {
        Self {
            mem: MaybeUninit::<[u8; N]>::uninit(),
            len: 0,
        }
    }
}

impl<const N: usize> DirentBuf for ArrayBuffer<N> {
    #[inline]
    fn reset(&mut self) {
        self.len = 0;
    }

    #[inline]
    fn reserve(&mut self, size: usize) -> Result<(), Errno> {
        if size > N {
            Err(Errno::ENOMEM)
        } else {
            Ok(())
        }
    }

    #[inline]
    fn as_ptr(&self) -> *const u8 {
        self.mem.as_ptr() as *const u8
    }

    #[inline]
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.mem.as_mut_ptr() as *mut u8
    }

    #[inline]
    fn capacity(&self) -> usize {
        N
    }

    #[inline]
    fn len(&self) -> usize {
        self.len
    }

    #[inline]
    unsafe fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    #[inline(always)]
    fn shrink_to_fit(&mut self) {}
}

impl<const N: usize> Deref for ArrayBuffer<N> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<const N: usize> DerefMut for ArrayBuffer<N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl<const N: usize> AsRef<[u8]> for ArrayBuffer<N> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl<const N: usize> AsMut<[u8]> for ArrayBuffer<N> {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl<const N: usize> Borrow<[u8]> for ArrayBuffer<N> {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

impl<const N: usize> BorrowMut<[u8]> for ArrayBuffer<N> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

#[cfg(feature = "std")]
impl VecBuffer {
    /// Creates a new instance of [Self].
    #[inline]
    pub const fn new() -> Self {
        Self { mem: Vec::new() }
    }
}

#[cfg(feature = "std")]
impl DirentBuf for VecBuffer {
    #[inline]
    fn reset(&mut self) {
        unsafe { self.mem.set_len(0) };
    }

    #[inline]
    fn reserve(&mut self, size: usize) -> Result<(), Errno> {
        if let Some(additional) = size.checked_sub(self.len()) {
            self.mem.reserve_exact(additional);
        }
        Ok(())
    }

    #[inline]
    fn as_ptr(&self) -> *const u8 {
        self.mem.as_ptr() as *const u8
    }

    #[inline]
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.mem.as_mut_ptr() as *mut u8
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.mem.capacity()
    }

    #[inline]
    fn len(&self) -> usize {
        self.mem.len()
    }

    #[inline]
    unsafe fn set_len(&mut self, len: usize) {
        self.mem.set_len(len);
    }

    #[inline]
    fn shrink_to_fit(&mut self) {
        self.mem.shrink_to_fit();
    }
}

#[cfg(feature = "std")]
impl Deref for VecBuffer {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

#[cfg(feature = "std")]
impl DerefMut for VecBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

#[cfg(feature = "std")]
impl AsRef<[u8]> for VecBuffer {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(feature = "std")]
impl AsMut<[u8]> for VecBuffer {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

#[cfg(feature = "std")]
impl Borrow<[u8]> for VecBuffer {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(feature = "std")]
impl BorrowMut<[u8]> for VecBuffer {
    #[inline]
    fn borrow_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

#[cfg(feature = "c")]
impl CBuffer {
    /// Creates a new instance of [Self].
    pub fn new() -> Self {
        Self {
            mem: core::ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }
}

#[cfg(feature = "c")]
impl Default for CBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "c")]
impl Drop for CBuffer {
    fn drop(&mut self) {
        if !self.mem.is_null() {
            unsafe { libc::free(self.mem.cast()) };
        }
    }
}

#[cfg(feature = "c")]
impl DirentBuf for CBuffer {
    #[inline]
    fn reset(&mut self) {
        unsafe { self.set_len(0) };
    }

    fn reserve(&mut self, size: usize) -> Result<(), Errno> {
        if size > self.capacity {
            let mem = if self.mem.is_null() {
                unsafe { libc::malloc(size) }
            } else {
                unsafe { libc::realloc(self.mem.cast(), size) }
            };

            if mem.is_null() {
                return Err(Errno::ENOMEM);
            }

            self.mem = mem.cast();
            self.capacity = size;
        }

        Ok(())
    }

    #[inline]
    fn as_ptr(&self) -> *const u8 {
        self.mem
    }

    #[inline]
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.mem
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    fn len(&self) -> usize {
        self.len
    }

    #[inline]
    unsafe fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    fn shrink_to_fit(&mut self) {
        let mem = unsafe { libc::realloc(self.mem.cast(), self.len) };

        if !mem.is_null() {
            self.mem = mem.cast();
            self.capacity = self.len;
        }
    }
}

#[cfg(feature = "c")]
impl Deref for CBuffer {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

#[cfg(feature = "c")]
impl DerefMut for CBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

#[cfg(feature = "c")]
impl AsRef<[u8]> for CBuffer {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(feature = "c")]
impl AsMut<[u8]> for CBuffer {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

#[cfg(feature = "c")]
impl Borrow<[u8]> for CBuffer {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

#[cfg(feature = "c")]
impl BorrowMut<[u8]> for CBuffer {
    #[inline]
    fn borrow_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}
