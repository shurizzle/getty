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

pub use core::ffi::CStr;
use core::fmt;

pub use bsd_errnos::Errno;
/// Device id.
pub type Dev = u32;

/// A process' informations useful to get tty informations.
#[derive(Debug, Clone)]
pub struct RawProcessInfo {
    /// The process id.
    pub pid: u32,
    /// The user id owning the process.
    pub uid: u32,
    /// The group id owning the process.
    pub gid: u32,
    /// The session id.
    pub session: u32,
    /// The tty device id if process has one.
    pub tty: Option<Dev>,
}

/// [RawProcessInfo] with `tty` field remapped to [TtyInfo].
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// The process id.
    pub pid: u32,
    /// The user id owning the process.
    pub uid: u32,
    /// The group id owning the process.
    pub gid: u32,
    /// The session id.
    pub session: u32,
    /// The tty device informations if process has one.
    pub tty: Option<TtyInfo>,
}

impl RawProcessInfo {
    /// Returns the informations for the current process.
    #[inline]
    pub fn current() -> Result<Self, Errno> {
        Self::for_process(std::process::id())
    }

    /// Returns the informations for the `pid` process.
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

        let tty = if ki_proc.kp_eproc.e_tdev == -1 {
            None
        } else {
            Some(ki_proc.kp_eproc.e_tdev as Dev)
        };

        Ok(Self {
            pid,
            uid,
            gid,
            session,
            tty,
        })
    }
}

impl ProcessInfo {
    /// Calls [RawProcessInfo::current] and maps `tty` with [TtyInfo::by_device].
    #[inline]
    pub fn current() -> Result<Self, Errno> {
        ProcessInfo::for_process(std::process::id())
    }

    /// Calls [RawProcessInfo::for_process] and maps `tty` with [TtyInfo::by_device].
    #[inline]
    pub fn for_process(pid: u32) -> Result<Self, Errno> {
        let info = RawProcessInfo::for_process(pid)?;

        Ok(Self {
            pid: info.pid,
            uid: info.uid,
            gid: info.gid,
            session: info.session,
            tty: info.tty.map(TtyInfo::by_device).transpose()?,
        })
    }
}

/// A structure that contains informations about a tty.
#[derive(Clone)]
pub struct TtyInfo {
    nr: Dev,
    buf: *mut u8,
}

impl TtyInfo {
    /// Returns the device number.
    #[inline]
    pub const fn device(&self) -> Dev {
        self.nr
    }

    /// Returns the device full path.
    #[inline]
    pub fn path(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.buf.cast()) }
    }

    /// Returns the device full path.
    #[inline]
    pub fn name(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.buf.add(5).cast()) }
    }

    /// Find a tty by its device number.
    pub fn by_device(rdev: Dev) -> Result<TtyInfo, Errno> {
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

            Ok(TtyInfo { nr: rdev, buf })
        }
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

impl Drop for TtyInfo {
    #[inline]
    fn drop(&mut self) {
        unsafe { libc::free(self.buf.cast()) };
    }
}

impl fmt::Debug for TtyInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtyInfo")
            .field("name", &self.name())
            .field("path", &self.path())
            .finish()
    }
}
