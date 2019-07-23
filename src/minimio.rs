//! As you'll see the system calls for interacting with Epoll, Kqueue and IOCP is highly
//! platform specific. The Rust community has already abstracted this away in the `mio` crate
//! but since we want to see what really goes on under the hood we implement a sort of mini-mio
//! library ourselves.
use super::*;
pub fn queue() -> io::Result<i32> {
    if cfg!(target_os = "macos") {
        macos::kqueue()
    } else {
        unimplemented!()
    }
}

#[cfg(target_os = "macos")]
pub type Event = macos::ffi::Kevent;

pub fn poll(
    queue: i32,
    changelist: &mut [Event],
    timeout: usize,
    max_events: Option<i32>,
) -> io::Result<usize> {
    if cfg!(target_os = "macos") {
        macos::kevent(queue, &[], changelist, timeout)
    } else {
        unimplemented!()
    }
}

/// Timeout of 0 means no timeout
pub fn add_event(queue: i32, event_list: &[Event], timeout_ms: usize) -> io::Result<usize> {
    if cfg!(target_os = "macos") {
        macos::kevent(queue, event_list, &mut [], timeout_ms)
    } else {
        unimplemented!()
    }
}

pub fn event_read(fd: RawFd) -> Event {
    if cfg!(target_os = "macos") {
        Event {
            ident: fd as u64,
            filter: macos::EVFILT_READ,
            flags: macos::EV_ADD | macos::EV_ENABLE | macos::EV_ONESHOT,
            fflags: 0,
            data: 0,
            udata: 0,
        }
    } else {
        unimplemented!()
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use ffi::*;

    // Shamelessly stolen from the libc wrapper found at:
    // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L2447
    pub const EVFILT_TIMER: i16 = -7;
    pub const EVFILT_READ: i16 = -1;
    pub const EV_ADD: u16 = 0x1;
    pub const EV_ENABLE: u16 = 0x4;
    pub const EV_ONESHOT: u16 = 0x10;

    pub mod ffi {
        #[derive(Debug, Clone, Default)]
        #[repr(C)]
        // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L497
        // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L207
        pub struct Kevent {
            pub ident: u64,
            pub filter: i16,
            pub flags: u16,
            pub fflags: u32,
            pub data: i64,
            pub udata: u64,
        }
        #[link(name = "c")]
        extern "C" {
            /// Returns: positive: file descriptor, negative: error
            pub(super) fn kqueue() -> i32;
            /// Returns: nothing, all non zero return values is an error
            pub(super) fn kevent(
                kq: i32,
                changelist: *const Kevent,
                nchanges: i32,
                eventlist: *mut Kevent,
                nevents: i32,
                timeout: usize,
            ) -> i32;
        }
    }

    pub fn kqueue() -> io::Result<i32> {
        let fd = unsafe { ffi::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }

    pub fn kevent(
        kq: RawFd,
        cl: &[Kevent],
        el: &mut [Kevent],
        timeout: usize,
    ) -> io::Result<usize> {
        let res = unsafe {
            let kq = kq as i32;
            let cl_len = cl.len() as i32;
            let el_len = el.len() as i32;
            ffi::kevent(kq, cl.as_ptr(), cl_len, el.as_mut_ptr(), el_len, timeout)
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(res as usize)
    }
}
