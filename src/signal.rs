use std::{
    ffi::c_int,
    io,
    mem::{ManuallyDrop, MaybeUninit},
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    ptr,
    sync::atomic::{AtomicI32, Ordering::Relaxed},
};

use libc::{
    raise, SA_RESETHAND, SA_RESTART, SIG_DFL, SIG_ERR,
    SIGHUP, SIGINT, SIGUSR1, SIGUSR2, SIGTERM,
    sigaction, sigemptyset, signal, sigset_t, write,
};
use rustix::{
    fd::AsFd,
    io::read_uninit,
};

use crate::poll::pipe_cloexec_nonblock;

const TERM_SIGNALS: [c_int; 3] = [SIGHUP, SIGINT, SIGTERM];
const OTHER_SIGNALS: [c_int; 2] = [SIGUSR1, SIGUSR2];

const TERM: u8 = 1 << 0;
const INT: u8 = 1 << 1;
const HUP: u8 = 1 << 2;
const USR1: u8 = 1 << 3;
const USR2: u8 = 1 << 4;

static PIPE_FD: AtomicI32 = AtomicI32::new(-1);

pub struct SignalPipe {
    read_half: OwnedFd,
}

impl SignalPipe {
    pub fn new() -> io::Result<SignalPipe> {
        unsafe {
            let (read_half, write_half) = pipe_cloexec_nonblock()?;
            PIPE_FD.compare_exchange(
                -1,
                write_half.as_raw_fd(),
                Relaxed,
                Relaxed,
            ).unwrap();
            let _ = ManuallyDrop::new(write_half);
            let ret = SignalPipe { read_half };
            let sigset_empty = sigset_empty()?;
            for signum in TERM_SIGNALS {
                sigaction_set_handler(
                    signum,
                    handle_termination_signals,
                    sigset_empty,
                    SA_RESTART | SA_RESETHAND,
                )?;
            }
            for signum in OTHER_SIGNALS {
                sigaction_set_handler(
                    signum,
                    handle_other_signals,
                    sigset_empty,
                    SA_RESTART,
                )?;
            }
            Ok(ret)
        }
    }

    pub fn read(&self) -> io::Result<SignalFlags> {
        let mut buf = [MaybeUninit::<u8>::uninit(); 64];
        let mut flags = 0;
        for byte in read_uninit(&self.read_half, &mut buf)?.0 {
            assert_ne!(*byte, 0);
            flags |= *byte;
        }
        Ok(SignalFlags(flags))
    }
}

impl Drop for SignalPipe {
    fn drop(&mut self) {
        for signum in OTHER_SIGNALS {
            sigaction_reset_default(signum).unwrap();
        }
        for signum in TERM_SIGNALS {
            sigaction_reset_default(signum).unwrap();
        }
        let write_half_fd = PIPE_FD.swap(-1, Relaxed);
        assert_ne!(write_half_fd, -1);
        drop(unsafe { OwnedFd::from_raw_fd(write_half_fd) });
    }
}

impl AsFd for SignalPipe {
    fn as_fd(&self) -> BorrowedFd {
        self.read_half.as_fd()
    }
}

#[derive(Clone, Copy)]
pub struct SignalFlags(u8);

impl SignalFlags {
    pub fn any_termination(self) -> Option<&'static str> {
        if self.0 & TERM != 0 {
            Some("TERM")
        } else if self.0 & INT != 0 {
            Some("INT")
        } else if self.0 & HUP != 0 {
            Some("HUP")
        } else {
            None
        }
    }
    pub fn has_usr1(self) -> bool {
        self.0 & USR1 != 0
    }
    pub fn has_usr2(self) -> bool {
        self.0 & USR2 != 0
    }
}

fn sigset_empty() -> io::Result<sigset_t> {
    unsafe {
        let mut sigset = MaybeUninit::uninit();
        if sigemptyset(sigset.as_mut_ptr()) < 0 {
            return Err(io::Error::last_os_error())
        }
        Ok(sigset.assume_init())
    }
}

unsafe fn sigaction_set_handler(
    signum: c_int,
    handler: extern "C" fn(c_int),
    mask: sigset_t,
    flags: c_int,
) -> io::Result<()> {
    unsafe {
        let mut act: sigaction = MaybeUninit::zeroed().assume_init();
        act.sa_sigaction = handler as _;
        act.sa_mask = mask;
        act.sa_flags = flags;
        if sigaction(
            signum,
            &act,
            ptr::null_mut(),
        ) < 0 {
            return Err(io::Error::last_os_error())
        }
        Ok(())
    }
}

fn sigaction_reset_default(signum: c_int) -> io::Result<()> {
    unsafe {
        if signal(signum, SIG_DFL) == SIG_ERR {
            return Err(io::Error::last_os_error())
        }
        Ok(())
    }
}

extern "C" fn handle_termination_signals(signum: c_int) {
    unsafe {
        let _errno_guard = ErrnoGuard::new();
        let byte: u8 = match signum {
            SIGTERM => TERM,
            SIGINT => INT,
            SIGHUP => HUP,
            _ => 0,
        };
        // In case of an error termination signals will have SA_RESETHAND set
        // so re-raise the signal to invoke the default handler
        if write(
            PIPE_FD.load(Relaxed),
            ptr::from_ref(&byte).cast(),
            1,
        ) != 1 {
            raise(signum);
        }
    }
}

extern "C" fn handle_other_signals(signum: c_int) {
    unsafe {
        let _errno_guard = ErrnoGuard::new();
        let byte: u8 = match signum {
            SIGUSR1 => USR1,
            SIGUSR2 => USR2,
            _ => 0,
        };
        // In case of an error ignore non-termination signals
        let _ = write(
            PIPE_FD.load(Relaxed),
            ptr::from_ref(&byte).cast(),
            1,
        );
    }
}

struct ErrnoGuard(i32);

impl ErrnoGuard {
    unsafe fn new() -> ErrnoGuard {
        ErrnoGuard(errno())
    }
}

impl Drop for ErrnoGuard {
    fn drop(&mut self) {
        set_errno(self.0)
    }
}

// Based on the Rust Standard Library:
// .rustup/toolchains/nightly-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/sys/pal/unix/os.rs
// with changes for stable rust from the errno crate:
// https://github.com/lambda-fairy/rust-errno/blob/main/src/unix.rs
// under licence MIT OR Apache-2.0

#[allow(unexpected_cfgs)]
extern "C" {
    #[cfg_attr(
        any(
            target_os = "linux",
            target_os = "emscripten",
            target_os = "fuchsia",
            target_os = "l4re",
            target_os = "hurd",
            target_os = "dragonfly",
        ),
        link_name = "__errno_location"
    )]
    #[cfg_attr(
        any(
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "cygwin",
            target_os = "android",
            target_os = "redox",
            target_os = "nuttx",
            target_env = "newlib",
            target_os = "vxworks",
        ),
        link_name = "__errno"
    )]
    #[cfg_attr(
        any(target_os = "solaris", target_os = "illumos"),
        link_name = "___errno"
    )]
    #[cfg_attr(target_os = "nto", link_name = "__get_errno_ptr")]
    #[cfg_attr(
        any(target_os = "freebsd", target_vendor = "apple"),
        link_name = "__error"
    )]
    #[cfg_attr(target_os = "haiku", link_name = "_errnop")]
    #[cfg_attr(target_os = "aix", link_name = "_Errno")]
    // SAFETY: this will always return the same pointer on a given thread.
    fn errno_location() -> *mut c_int;
}

/// Returns the platform-specific value of errno
#[inline]
fn errno() -> i32 {
    unsafe { (*errno_location()) as i32 }
}

/// Sets the platform-specific value of errno
// needed for readdir and syscall!
#[inline]
fn set_errno(e: i32) {
    unsafe { *errno_location() = e as c_int }
}
