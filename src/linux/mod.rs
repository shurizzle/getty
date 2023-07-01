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

#[derive(Clone)]
pub struct TtyInfo<B: DirentBuf> {
    nr: Dev,
    buf: B,
    offset: usize,
}

impl<B: DirentBuf> TtyInfo<B> {
    #[inline]
    pub fn number(&self) -> Dev {
        self.nr
    }

    #[inline]
    pub fn path(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.buf.as_ptr().cast()) }
    }

    #[inline]
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.buf.as_ptr().add(self.offset).cast()) }
    }
}

impl<B: DirentBuf> fmt::Debug for TtyInfo<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtyInfo")
            .field("number", &self.number())
            .field("path", &self.path())
            .field("name", &self.name())
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

fn try_path<B: DirentBuf>(
    dirfd: &Dir,
    file: &CStr,
    ttynr: Dev,
    path: &mut B,
) -> Result<Option<()>, Errno> {
    loop {
        unsafe {
            match fstatat_cstr(dirfd.as_raw_fd(), file, StatAtFlags::SYMLINK_NOFOLLOW) {
                Err(Errno::EINTR) => (),
                Err(err) => return Err(err),
                Ok(md) => {
                    return if md.rdev() == ttynr {
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

fn scandir<B1: DirentBuf, B2: DirentBuf>(
    mut dirfd: Dir,
    ttynr: Dev,
    buf: &mut B1,
    path: &mut B2,
) -> Result<Option<()>, Errno> {
    let dupfd = ManuallyDrop::new(unsafe { Dir::from_raw_fd(dirfd.as_raw_fd()) });

    let mut dirit = DirIterator::new(&mut dirfd, buf)?;
    while let Some(entry) = dirit.next() {
        let entry = entry?;
        let name_cstr = entry.name();
        let name = name_cstr.to_bytes();

        if name == b"." || name == b".." {
            continue;
        }

        match entry.file_type() {
            DirentFileType::Character => {
                if Some(()) == try_path(&dupfd, name_cstr, ttynr, path)? {
                    return Ok(Some(()));
                }
            }
            DirentFileType::Directory => {
                _ = dirit;
                {
                    let new_dirfd = Dir::open_at(&dupfd, name_cstr)?;
                    let old_len = path.len();
                    if Some(()) == scandir(new_dirfd, ttynr, buf, path)? {
                        return Ok(Some(()));
                    }
                    unsafe { path.set_len(old_len) };
                }

                dirit = DirIterator::new(&mut dirfd, buf)?;
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

impl<B: DirentBuf> TtyInfo<B> {
    pub fn by_number_with_buffers_in<'a, I, B1>(
        rdev: Dev,
        dirs: I,
        buf: &mut B1,
        mut path: B,
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
            if Some(()) == find_in_dir(dir, guessing, rdev, buf, &mut path)? {
                path.shrink_to_fit();
                return Ok(TtyInfo {
                    nr: rdev,
                    buf: path,
                    offset: dir.to_bytes().len() + 1,
                });
            }
        }

        Err(Errno::ENOENT)
    }

    pub fn by_number_with_buffers<B1: DirentBuf>(
        rdev: Dev,
        buf: &mut B1,
        path: B,
    ) -> Result<Self, Errno> {
        Self::by_number_with_buffers_in(
            rdev,
            [unsafe { CStr::from_bytes_with_nul_unchecked(b"/dev\0") }],
            buf,
            path,
        )
    }
}

impl TtyInfo<PathBuf> {
    #[inline]
    pub fn by_number_in<'a, I>(rdev: Dev, dirs: I) -> Result<Self, Errno>
    where
        I: IntoIterator<Item = &'a CStr>,
    {
        Self::by_number_with_buffers_in(rdev, dirs, &mut DirBuf::new(), PathBuf::new())
    }

    #[inline]
    pub fn by_number(rdev: Dev) -> Result<Self, Errno> {
        Self::by_number_with_buffers(rdev, &mut DirBuf::new(), PathBuf::new())
    }
}
