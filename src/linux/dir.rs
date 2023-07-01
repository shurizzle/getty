use core::{
    borrow::{Borrow, BorrowMut},
    mem::{ManuallyDrop, MaybeUninit},
    ops::{Deref, DerefMut},
};

use linux_stat::{CStr, RawFd, CURRENT_DIRECTORY};
use linux_syscalls::{syscall, Errno, Sysno};

const O_DIRECTORY: usize = 0o200000;
const O_CLOEXEC: usize = 0o2000000;
const O_RDONLY: usize = 0;

pub struct Dir {
    fd: RawFd,
    tell: u64,
}

impl Dir {
    pub fn open_at(fd: &Dir, file: &CStr) -> Result<Self, Errno> {
        loop {
            match unsafe {
                syscall!([ro] Sysno::openat, fd.as_raw_fd(), file.as_ptr(), O_RDONLY | O_DIRECTORY | O_CLOEXEC, 0)
            } {
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

    #[inline]
    pub fn open(file: &CStr) -> Result<Self, Errno> {
        let dir = ManuallyDrop::new(unsafe { Dir::from_raw_fd(CURRENT_DIRECTORY) });
        Self::open_at(&dir, file)
    }

    #[inline]
    pub const unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self { fd, tell: 0 }
    }

    #[inline]
    pub const fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        _ = unsafe { syscall!([ro] Sysno::close, self.fd) };
    }
}

pub trait DirentBuf:
    Deref<Target = [u8]> + DerefMut + AsRef<[u8]> + AsMut<[u8]> + Borrow<[u8]> + BorrowMut<[u8]>
{
    fn reset(&mut self);

    fn reserve(&mut self, size: usize) -> Result<(), Errno>;

    fn as_ptr(&self) -> *const u8;

    fn as_mut_ptr(&mut self) -> *mut u8;

    #[inline]
    fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.as_ptr(), self.len()) }
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.as_mut_ptr(), self.len()) }
    }

    fn capacity(&self) -> usize;

    fn len(&self) -> usize;

    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    unsafe fn set_len(&mut self, len: usize);

    fn shrink_to_fit(&mut self);

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

    #[inline]
    fn push_c_str(&mut self, s: &CStr) -> Result<(), Errno> {
        self.push_slice(s.to_bytes())
    }
}

#[repr(u8)]
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(dead_code)]
pub enum DirentFileType {
    Unknown = 0,
    Fifo = 1,
    Character = 2,
    Directory = 4,
    Block = 6,
    Regular = 8,
    Link = 10,
    Socket = 12,
    Wht = 14,
}

#[allow(non_camel_case_types)]
#[repr(packed)]
#[allow(dead_code)]
pub struct dirent {
    ino: u64,
    off: u64,
    reclen: core::ffi::c_ushort,
    r#type: DirentFileType,
    name: [u8; 0],
}

pub struct DirIterator<'a, B: DirentBuf> {
    dir: &'a mut Dir,
    buf: &'a mut B,
    offset: usize,
}

impl<'a, B: DirentBuf> DirIterator<'a, B> {
    #[inline]
    pub fn new(dir: &'a mut Dir, buf: &'a mut B) -> Result<Self, Errno> {
        if dir.tell != 0 {
            unsafe { syscall!([ro] Sysno::lseek, dir.fd, dir.tell, 0)? };
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
    type Item = Result<&'a dirent, Errno>;

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

            if buf.len() < core::mem::size_of::<dirent>() {
                self.offset = 0;
                if let Err(err) = getdents64(self.dir.fd, self.buf) {
                    return Some(Err(err));
                }
                buf = self.buffer();
            }

            if buf.len() < core::mem::size_of::<dirent>() {
                None
            } else {
                let res: &'a dirent = &*(buf.as_ptr().cast());
                self.offset += res.len();
                self.dir.tell = res.offset();
                Some(Ok(res))
            }
        }
    }
}

impl dirent {
    #[inline]
    pub const fn inode(&self) -> u64 {
        self.ino
    }

    #[inline]
    const fn offset(&self) -> u64 {
        self.off
    }

    #[inline]
    pub const fn file_type(&self) -> DirentFileType {
        self.r#type
    }

    #[inline]
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.name.as_ptr().cast()) }
    }

    #[inline]
    pub const fn len(&self) -> usize {
        self.reclen as usize
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct ArrayBuffer<const N: usize> {
    mem: MaybeUninit<[u8; N]>,
    len: usize,
}

#[cfg(feature = "std")]
pub struct VecBuffer {
    mem: Vec<u8>,
}

#[cfg(feature = "c")]
pub struct CBuffer {
    mem: *mut u8,
    len: usize,
    capacity: usize,
}

impl<const N: usize> ArrayBuffer<N> {
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
