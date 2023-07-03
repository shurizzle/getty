mod dir;
mod pinfo;

pub use dir::*;
pub use pinfo::*;

use core::{
    fmt,
    mem::{ManuallyDrop, MaybeUninit},
};

use linux_stat::{fstatat_cstr, StatAtFlags};

pub use linux_stat::{CStr, Dev, RawFd};
pub use linux_syscalls::Errno;

const TTY_MAJOR: u32 = 4;
const PTS_MAJOR: u32 = 136;
const TTY_ACM_MAJOR: u32 = 166;
const TTY_USB_MAJOR: u32 = 188;
const NR_CONSOLES: u32 = 64;
const MAX_U32_LENGTH: usize = 10;

/// A structure that contains informations about a tty.
#[derive(Clone)]
pub struct TtyInfo<B: DirentBuf> {
    dev: Dev,
    buf: B,
    offset: usize,
}

impl<B: DirentBuf> TtyInfo<B> {
    /// Returns the device number.
    #[inline]
    pub fn device(&self) -> Dev {
        self.dev
    }

    /// Returns the device full path.
    #[inline]
    pub fn path(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.buf.as_ptr().cast()) }
    }

    /// Returns the device full path.
    #[inline]
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.buf.as_ptr().add(self.offset).cast()) }
    }
}

impl<B: DirentBuf> fmt::Debug for TtyInfo<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtyInfo")
            .field("device", &self.device())
            .field("name", &self.name())
            .field("path", &self.path())
            .finish()
    }
}

fn try_path_guessing<B: DirentBuf>(
    dirfd: &Dir,
    file: &CStr,
    ttynr: Dev,
    path: &mut B,
) -> Result<Option<()>, Errno> {
    loop {
        unsafe {
            match fstatat_cstr(dirfd.as_raw_fd(), file, StatAtFlags::SYMLINK_NOFOLLOW) {
                Err(Errno::ENOENT) => return Ok(None),
                Err(Errno::EINTR) => (),
                Err(err) => return Err(err),
                Ok(md) => {
                    return if md.is_char() && md.rdev() == ttynr {
                        let file = file.to_bytes();

                        path.reserve(path.len() + file.len() + 2)?;

                        path.push_slice(b"/")?;
                        path.push_slice(file)?;
                        path.push_slice(b"\0")?;

                        Ok(Some(()))
                    } else {
                        Ok(None)
                    };
                }
            }
        }
    }
}

#[inline]
fn statat(dirfd: &Dir, file: &CStr) -> Result<linux_stat::Stat, Errno> {
    loop {
        match unsafe { fstatat_cstr(dirfd.as_raw_fd(), file, StatAtFlags::SYMLINK_NOFOLLOW) } {
            Err(Errno::EINTR) => (),
            other => return other,
        }
    }
}

#[inline(always)]
fn try_path<B: DirentBuf>(
    md: linux_stat::Stat,
    file: &CStr,
    ttynr: Dev,
    path: &mut B,
) -> Result<Option<()>, Errno> {
    if md.rdev() == ttynr {
        let file = file.to_bytes();

        path.reserve(path.len() + file.len() + 2)?;

        path.push_slice(b"/")?;
        path.push_slice(file)?;
        path.push_slice(b"\0")?;

        Ok(Some(()))
    } else {
        Ok(None)
    }
}

fn scandir<B1: DirentBuf, B2: DirentBuf>(
    mut dirfd: Dir,
    ttynr: Dev,
    buf: &mut B1,
    path: &mut B2,
) -> Result<Option<()>, Errno> {
    let dupfd = ManuallyDrop::new(unsafe { Dir::from_raw_fd(dirfd.as_raw_fd()) });

    let mut dirit = dirfd.iter(buf)?;
    while let Some(entry) = dirit.next() {
        let entry = entry?;
        let name_cstr = entry.name();
        let name = name_cstr.to_bytes();

        if name == b"." || name == b".." {
            continue;
        }

        let (ft, md) = match entry.file_type().into() {
            linux_stat::FileType::Unknown => {
                let md = statat(&dupfd, name_cstr)?;
                (md.file_type(), Some(md))
            }
            ft => (ft, None),
        };

        match ft {
            linux_stat::FileType::Character => {
                let md = if let Some(md) = md {
                    md
                } else {
                    statat(&dupfd, name_cstr)?
                };

                if Some(()) == try_path(md, name_cstr, ttynr, path)? {
                    return Ok(Some(()));
                }
            }
            linux_stat::FileType::Directory => {
                _ = dirit;
                {
                    let new_dirfd = Dir::open_at(&dupfd, name_cstr)?;
                    let old_len = path.len();
                    if Some(()) == scandir(new_dirfd, ttynr, buf, path)? {
                        return Ok(Some(()));
                    }
                    unsafe { path.set_len(old_len) };
                }

                dirit = dirfd.iter(buf)?;
            }
            _ => (),
        }
    }

    Ok(None)
}

#[cfg(feature = "std")]
type DirBuf = VecBuffer;
#[cfg(all(feature = "c", not(feature = "std")))]
type DirBuf = CBuffer;
#[cfg(all(not(feature = "c"), not(feature = "std")))]
type DirBuf = ArrayBuffer<2048>;

#[cfg(feature = "std")]
type PathBuf = VecBuffer;
#[cfg(all(feature = "c", not(feature = "std")))]
type PathBuf = CBuffer;
#[cfg(all(not(feature = "c"), not(feature = "std")))]
type PathBuf = ArrayBuffer<4096>;

#[inline(always)]
fn find_in_dir<B1: DirentBuf, B2: DirentBuf>(
    dir: &CStr,
    guessing: &CStr,
    ttynr: Dev,
    buf: &mut B1,
    path: &mut B2,
) -> Result<Option<()>, Errno> {
    path.push_c_str(dir)?;

    let dirfd = Dir::open(dir)?;

    if Some(()) == try_path_guessing(&dirfd, guessing, ttynr, path)? {
        return Ok(Some(()));
    }

    scandir(dirfd, ttynr, buf, path)
}

fn concat_cstr_number<const N: usize>(
    buf: &mut MaybeUninit<[u8; 6 + MAX_U32_LENGTH + 1]>,
    cstr: &[u8; N],
    n: u32,
) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            cstr.as_ptr().cast::<u8>(),
            buf.as_mut_ptr().cast::<u8>(),
            cstr.len(),
        );
        let ptr = buf.as_mut_ptr().cast::<u8>().add(cstr.len());
        let len = itoap::write_to_ptr(ptr, n);
        *ptr.add(len) = 0;
    }
}

#[inline(always)]
pub(crate) fn with_default_paths<'a, T, F: FnOnce([&'a CStr; 1]) -> T>(f: F) -> T {
    f([unsafe { CStr::from_bytes_with_nul_unchecked(b"/dev\0") }])
}

impl<B: DirentBuf> TtyInfo<B> {
    /// Find a tty by its device number in `dir` using `dirent_buf` as dirent
    /// buffer and `path_buf` as filesystem path buffer.
    ///
    /// # Errors
    ///
    /// Returns [Errno::ENOTTY] if major device number is not a valid tty and
    /// [Errno::ENOENT] if it is not present. Other [Errno]s can be returned
    /// due to `open`, `getdents64`, `lseek` and `fstatat` syscalls
    /// or memory allocations.
    pub fn by_device_with_buffers_in<'a, I, B1>(
        rdev: Dev,
        dirs: I,
        dirent_buf: &mut B1,
        mut path_buf: B,
    ) -> Result<Self, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
        B1: DirentBuf,
    {
        let mut guess_buf = MaybeUninit::<[u8; 6 + MAX_U32_LENGTH + 1]>::uninit();
        match rdev.major() {
            TTY_MAJOR => {
                let min = rdev.minor();
                if min < NR_CONSOLES {
                    concat_cstr_number(&mut guess_buf, b"tty", min);
                } else {
                    concat_cstr_number(&mut guess_buf, b"ttyS", min - NR_CONSOLES);
                }
            }
            PTS_MAJOR => {
                concat_cstr_number(&mut guess_buf, b"pts/", rdev.minor());
            }
            TTY_ACM_MAJOR => {
                concat_cstr_number(&mut guess_buf, b"ttyACM", rdev.minor());
            }
            TTY_USB_MAJOR => {
                concat_cstr_number(&mut guess_buf, b"ttyUSB", rdev.minor());
            }
            _ => return Err(Errno::ENOTTY),
        }
        let guess_buf = unsafe { guess_buf.assume_init() };
        let guessing = unsafe { CStr::from_ptr(guess_buf.as_slice().as_ptr().cast()) };

        for dir in dirs {
            if Some(()) == find_in_dir(dir, guessing, rdev, dirent_buf, &mut path_buf)? {
                path_buf.shrink_to_fit();
                return Ok(TtyInfo {
                    dev: rdev,
                    buf: path_buf,
                    offset: dir.to_bytes().len() + 1,
                });
            }
        }

        Err(Errno::ENOENT)
    }

    /// Shortcut for [RawProcessInfo::current] + [Self::by_device_with_buffers_in].
    #[inline]
    pub fn current_with_buffers_in<'a, I, B1>(
        dirs: I,
        dirent_buf: &mut B1,
        path_buf: B,
    ) -> Result<Option<Self>, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
        B1: DirentBuf,
    {
        RawProcessInfo::current()?
            .tty
            .map(|rdev| Self::by_device_with_buffers_in(rdev, dirs, dirent_buf, path_buf))
            .transpose()
    }

    /// Shortcut for [RawProcessInfo::for_process] + [Self::by_device_with_buffers_in].
    #[inline]
    pub fn for_process_with_buffers_in<'a, I, B1>(
        pid: u32,
        dirs: I,
        dirent_buf: &mut B1,
        path_buf: B,
    ) -> Result<Option<Self>, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
        B1: DirentBuf,
    {
        RawProcessInfo::for_process(pid)?
            .tty
            .map(|rdev| Self::by_device_with_buffers_in(rdev, dirs, dirent_buf, path_buf))
            .transpose()
    }

    /// Same as [Self::by_device_with_buffers_in] but with default
    /// `dirs` ('/dev').
    pub fn by_device_with_buffers<B1: DirentBuf>(
        rdev: Dev,
        buf: &mut B1,
        path: B,
    ) -> Result<Self, Errno> {
        with_default_paths(|dirs| Self::by_device_with_buffers_in(rdev, dirs, buf, path))
    }

    /// Shortcut for [RawProcessInfo::current] + [Self::by_device_with_buffers].
    #[inline]
    pub fn current_with_buffers<B1: DirentBuf>(
        dirent_buf: &mut B1,
        path_buf: B,
    ) -> Result<Option<Self>, Errno> {
        RawProcessInfo::current()?
            .tty
            .map(|rdev| Self::by_device_with_buffers(rdev, dirent_buf, path_buf))
            .transpose()
    }

    /// Shortcut for [RawProcessInfo::for_process] + [Self::by_device_with_buffers].
    #[inline]
    pub fn for_process_with_buffers<B1: DirentBuf>(
        pid: u32,
        dirent_buf: &mut B1,
        path_buf: B,
    ) -> Result<Option<Self>, Errno> {
        RawProcessInfo::for_process(pid)?
            .tty
            .map(|rdev| Self::by_device_with_buffers(rdev, dirent_buf, path_buf))
            .transpose()
    }
}

impl TtyInfo<PathBuf> {
    /// Same as [Self::by_device_with_buffers_in] but with default buffers.
    #[inline]
    pub fn by_device_in<'a, I>(rdev: Dev, dirs: I) -> Result<Self, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
    {
        Self::by_device_with_buffers_in(rdev, dirs, &mut DirBuf::new(), PathBuf::new())
    }

    /// Shortcut for [RawProcessInfo::current] + [Self::by_device_in].
    #[inline]
    pub fn current_in<'a, I>(dirs: I) -> Result<Option<Self>, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
    {
        RawProcessInfo::current()?
            .tty
            .map(|rdev| Self::by_device_in(rdev, dirs))
            .transpose()
    }

    /// Shortcut for [RawProcessInfo::for_process] + [Self::by_device_in].
    #[inline]
    pub fn for_process_in<'a, I>(pid: u32, dirs: I) -> Result<Option<Self>, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
    {
        RawProcessInfo::for_process(pid)?
            .tty
            .map(|rdev| Self::by_device_in(rdev, dirs))
            .transpose()
    }

    /// Same as [Self::by_device_with_buffers_in] but
    /// with default buffers and dirs.
    #[inline]
    pub fn by_device(rdev: Dev) -> Result<Self, Errno> {
        Self::by_device_with_buffers(rdev, &mut DirBuf::new(), PathBuf::new())
    }

    /// Shortcut for [RawProcessInfo::current] + [Self::by_device].
    #[inline]
    pub fn current() -> Result<Option<Self>, Errno> {
        RawProcessInfo::current()?
            .tty
            .map(Self::by_device)
            .transpose()
    }

    /// Shortcut for [RawProcessInfo::for_process] + [Self::by_device].
    #[inline]
    pub fn for_process(pid: u32) -> Result<Option<Self>, Errno> {
        RawProcessInfo::for_process(pid)?
            .tty
            .map(Self::by_device)
            .transpose()
    }
}
