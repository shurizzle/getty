use core::mem::MaybeUninit;

use atoi::FromRadix10;
use linux_stat::{CStr, Dev, RawFd};
use linux_syscalls::{syscall, Errno, Sysno};

const SELF_INFO_PATH: &[u8] = b"/proc/self/stat\0".as_slice();
const O_CLOEXEC: usize = 0o2000000;
const O_RDONLY: usize = 0;

unsafe fn parse_num<T: FromRadix10>(buf: &[u8]) -> Result<(T, &[u8]), Errno> {
    let (res, len) = T::from_radix_10(buf);
    if len == 0 {
        return Err(Errno::EINVAL);
    }
    let buf = buf.get_unchecked(len..);
    Ok((res, buf))
}

unsafe fn skip_char(buf: &[u8], ch: u8) -> Result<&[u8], Errno> {
    if buf.iter().copied().next().map_or(false, |c| c == ch) {
        Ok(buf.get_unchecked(1..))
    } else {
        Err(Errno::EINVAL)
    }
}

#[inline]
unsafe fn skip_space(buf: &[u8]) -> Result<&[u8], Errno> {
    skip_char(buf, b' ')
}

#[derive(Debug, Clone, Copy, Hash)]
pub struct ProcessInfo {
    pub pid: core::ffi::c_int,
    pub ppid: core::ffi::c_int,
    pub session: core::ffi::c_int,
    pub tty_nr: Option<Dev>,
}

impl ProcessInfo {
    fn parse(path: &CStr) -> Result<Self, Errno> {
        unsafe {
            struct FdHolder(RawFd);
            impl Drop for FdHolder {
                fn drop(&mut self) {
                    _ = unsafe { syscall!(Sysno::close, self.0) };
                }
            }

            let mut buf = MaybeUninit::<[u8; 1024]>::uninit();
            let mut len: usize = 0;
            {
                let fd = syscall!(Sysno::open, path.as_ptr(), O_RDONLY | O_CLOEXEC)? as RawFd;
                let _h = FdHolder(fd);
                let mut b = core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), 1024);
                while !b.is_empty() {
                    match syscall!(Sysno::read, fd, b.as_mut_ptr(), b.len()) {
                        Ok(0) => break,
                        Ok(n) => {
                            len += n;
                            b = b.get_unchecked_mut(n..);
                        }
                        Err(Errno::EAGAIN) | Err(Errno::EINTR) => (),
                        Err(err) => return Err(err),
                    }
                }
            }
            let buf = buf.assume_init();
            let buf = buf.get_unchecked(..len);

            let (pid, buf) = parse_num(buf)?;
            let buf = skip_space(buf)?;

            let buf = skip_char(buf, b'(')?;
            let buf = match memchr::memchr(b')', buf) {
                Some(i) => skip_space(buf.get_unchecked((i + 1)..))?,
                None => return Err(Errno::EINVAL),
            };

            let buf = match memchr::memchr(b' ', buf) {
                Some(1) => buf.get_unchecked(2..),
                Some(_) | None => return Err(Errno::EINVAL),
            };

            let (ppid, buf) = parse_num(buf)?;
            let buf = skip_space(buf)?;
            let buf = match memchr::memchr(b' ', buf) {
                Some(0) | None => return Err(Errno::EINVAL),
                Some(n) => buf.get_unchecked((n + 1)..),
            };
            let (session, buf) = parse_num(buf)?;
            let buf = skip_space(buf)?;
            let tty_nr = parse_num::<u32>(buf)?.0;
            let tty_nr = if tty_nr == core::mem::transmute::<i32, u32>(-1i32) {
                None
            } else {
                Some(tty_nr.into())
            };

            Ok(Self {
                pid,
                ppid,
                session,
                tty_nr,
            })
        }
    }

    #[inline]
    pub fn current() -> Result<Self, Errno> {
        Self::parse(unsafe { CStr::from_ptr(SELF_INFO_PATH.as_ptr().cast()) })
    }

    pub fn for_process(pid: core::ffi::c_int) -> Result<Self, Errno> {
        use itoap::Integer;

        if pid < 0 {
            return Err(Errno::EINVAL);
        }

        let mut uninit_buf = MaybeUninit::<[u8; 11 + core::ffi::c_int::MAX_LEN + 1]>::uninit();
        let path = unsafe {
            let mut buf = uninit_buf.as_mut_ptr().cast::<u8>();
            core::ptr::copy_nonoverlapping(b"/proc/".as_ptr().cast::<u8>(), buf, 6);
            buf = buf.add(6);
            let len = itoap::write_to_ptr(buf, pid);
            buf = buf.add(len);
            core::ptr::copy_nonoverlapping(b"/stat".as_ptr().cast::<u8>(), buf, 5);
            *buf.add(5) = 0;
            CStr::from_ptr((uninit_buf.as_mut_ptr().cast::<u8>() as *const u8).cast())
        };

        Self::parse(path)
    }
}
