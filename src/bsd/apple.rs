#[path = "mod.rs"]
mod bsd;

mod sysctl {
    #![allow(
        non_upper_case_globals,
        non_camel_case_types,
        non_snake_case,
        deref_nullptr,
        dead_code
    )]

    include!(concat!(env!("OUT_DIR"), "/bindings/sysctl.rs"));
}

use core::{ffi::CStr, fmt};

pub use apple_errnos::Errno;
pub type Dev = u32;

#[derive(Debug, Clone, Copy, Hash)]
pub struct ProcessInfo {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
    pub session: u32,
    pub tty_nr: Option<Dev>,
}

impl ProcessInfo {
    #[inline]
    pub fn current() -> Result<Self, Errno> {
        Self::for_process(std::process::id())
    }

    pub fn for_process(pid: u32) -> Result<Self, Errno> {
        let session = unsafe { libc::getsid(pid as i32) as u32 };

        let ki_proc = bsd::proc_info::<sysctl::kinfo_proc>(
            [
                libc::CTL_KERN,
                libc::KERN_PROC,
                libc::KERN_PROC_PID,
                pid as libc::c_int,
            ]
            .as_mut_slice(),
        )?;

        let uid = ki_proc.kp_eproc.e_pcred.p_ruid;
        let gid = ki_proc.kp_eproc.e_pcred.p_rgid;

        let tty_nr = if ki_proc.kp_eproc.e_tdev == -1 {
            None
        } else {
            Some(ki_proc.kp_eproc.e_tdev as Dev)
        };

        Ok(Self {
            pid,
            uid,
            gid,
            session,
            tty_nr,
        })
    }
}

#[derive(Clone)]
pub struct TtyInfo {
    buf: *mut u8,
}

impl TtyInfo {
    #[inline]
    pub fn path(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.buf.cast()) }
    }

    #[inline]
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.buf.add(5).cast()) }
    }
}

impl Drop for TtyInfo {
    fn drop(&mut self) {
        unsafe { libc::free(self.buf.cast()) };
    }
}

impl fmt::Debug for TtyInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtyInfo")
            .field("path", &self.path())
            .field("name", &self.name())
            .finish()
    }
}

pub fn find_by_number(rdev: Dev) -> Result<TtyInfo, Errno> {
    unsafe {
        let name = bsd::devname(rdev as _, libc::S_IFCHR);
        if name.is_null() {
            return Err(Errno::ENOENT);
        }

        let name = CStr::from_ptr(name).to_bytes();

        let buf = libc::malloc(5 + name.len() + 1) as *mut u8;
        if buf.is_null() {
            return Err(Errno::ENOMEM);
        }
        core::ptr::copy_nonoverlapping(b"/dev/".as_ptr().cast(), buf, 5);
        let ptr = buf.add(5);
        core::ptr::copy_nonoverlapping(name.as_ptr(), ptr, name.len());
        *ptr.add(name.len()) = 0;

        Ok(TtyInfo { buf })
    }
}

#[inline]
pub fn get_tty() -> Result<Option<TtyInfo>, Errno> {
    let info = ProcessInfo::current()?;

    if let Some(ttynr) = info.tty_nr {
        find_by_number(ttynr).map(Some)
    } else {
        Ok(None)
    }
}
