use crate::Errno;

#[macro_export]
macro_rules! prefix {
    ($p:literal) => {
        concat!("/usr/local", $p)
    };
}

use core::{
    borrow::{Borrow, BorrowMut},
    mem,
    ops::{Deref, DerefMut},
    ptr,
};

pub fn proc_info<T>(mibs: &mut [libc::c_int]) -> Result<CBox<T>, Errno> {
    unsafe {
        let mut ki_proc: *mut T = ptr::null_mut();
        let mut size = mem::size_of::<T>();

        let mut rc;
        loop {
            {
                size += size / 10;
                let kp = libc::realloc(ki_proc as *mut libc::c_void, size) as *mut T;
                if kp.is_null() {
                    rc = -1;
                    break;
                }
                ki_proc = kp;
            }

            rc = libc::sysctl(
                mibs.as_mut_ptr(),
                mibs.len() as u32,
                ki_proc as *mut _,
                &mut size,
                ptr::null_mut(),
                0,
            );

            if rc != -1 || Errno::last_os_error() != Errno::ENOMEM {
                break;
            }
        }

        if rc == -1 {
            let err = Errno::last_os_error();
            if !ki_proc.is_null() {
                libc::free(ki_proc as *mut _);
            }
            Err(err)
        } else {
            Ok(CBox::from_raw(ki_proc))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CBox<T>(*mut T);

impl<T> CBox<T> {
    #[inline]
    pub unsafe fn from_raw(raw: *mut T) -> Self {
        Self(raw)
    }
}

impl<T> Deref for CBox<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.0 }
    }
}

impl<T> DerefMut for CBox<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.0 }
    }
}

impl<T> Borrow<T> for CBox<T> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T> BorrowMut<T> for CBox<T> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut T {
        &mut *self
    }
}

impl<T> AsRef<T> for CBox<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T> AsMut<T> for CBox<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        &mut *self
    }
}

impl<T> Drop for CBox<T> {
    fn drop(&mut self) {
        unsafe {
            libc::free(self.0 as *mut libc::c_void);
        }
    }
}

extern "C" {
    pub fn devname(dev: libc::dev_t, r#type: libc::mode_t) -> *const i8;
}
