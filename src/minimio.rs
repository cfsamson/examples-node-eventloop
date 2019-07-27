//! As you'll see the system calls for interacting with Epoll, Kqueue and IOCP is highly
//! platform specific. The Rust community has already abstracted this away in the combination of the
//! `libc` and `mio` crates but since we want to see what really goes on under the hood we implement 
//! a sort of mini-mio library ourselves.
use super::*;

#[cfg(target_os = "macos")]
pub type Event = macos::ffi::Kevent;
#[cfg(target_os = "linux")]
pub type Event = linux::ffi::EpollEvent;


pub fn queue() -> io::Result<i32> {
    if cfg!(target_os = "macos") {
        macos::kqueue()
    } else if cfg!(target_os = "linux") {
        linux::queue()
    } else {
        unimplemented!()
    }
}

pub fn poll(
    queue: i32,
    changelist: &mut [Event],
    timeout: usize,
    max_events: Option<i32>,
) -> io::Result<usize> {
    if cfg!(target_os = "macos") {
        macos::kevent(queue, &[], changelist, timeout)
    } else {
        unimplemented!("Operating system not supported")
    }
}

/// Timeout of 0 means no timeout
pub fn add_event(queue: i32, event_list: &[Event], timeout_ms: usize) -> io::Result<usize> {
    if cfg!(target_os = "macos") {
        macos::kevent(queue, event_list, &mut [], timeout_ms)
    } else if cfg!(target_os = "linux") {
        linux::epoll_add(queue, fd)
    } else {
        unimplemented!("Operating system not supported")
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
        unimplemented!("Operating system not supported")
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use ffi::*;

    // Shamelessly stolen from the libc wrapper found at:
    // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L2447
    pub const EVFILT_READ: i16 = -1;
    pub const EV_ADD: u16 = 0x1;
    pub const EV_ENABLE: u16 = 0x4;
    pub const EV_ONESHOT: u16 = 0x10;

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

    pub mod ffi {

        // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L497
        // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/bsd/apple/mod.rs#L207
        #[derive(Debug, Clone, Default)]
        #[repr(C)]
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
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use ffi;

    // https://github.com/rust-lang/libc/blob/c8aa8ec72d631bc35099bcf5d634cf0a0b841be0/src/unix/linux_like/mod.rs#L855
    pub const EPOLL_CTL_ADD: ::c_int = 1;
    pub const EPOLLIN: ::c_int = 0x1;
    pub const EPOLLONESHOT: ::c_int = 0x40000000;

    pub fn queue() -> io::Result<i32> {
        let fd = unsafe { ffi::epoll_create(1) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }

    pub fn epoll_add(queue: i32, fd: i32) -> io::Result<()> {
        let event = ffi::EpollEvent {
            evens: EPOLLIN | EPOLLONESHOT,
            epoll_data: 0
        };

        let res = ffi::epoll_ctl(queue, EPOLL_CTL_ADD, fd, event);
        if res < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    pub fn epoll_wait(queue: RawFd, events: Vec<EpollEvent>, max_events: i32, timeout: i32) -> io::Result<i32> {
        let res = ffi::epoll_wait(queue, events, max_events, timeout);
        if res < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(res)
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

    pub mod ffi {
        #[repr(C)]
        union EpollData {
            void: *const std::os::raw::c_void,
            fd: i32,
            uint32: u32,
            unit64: u64,
        }

        #[derive(Debug, Clone, Default)]
        #[repr(C)]
        pub struct EpollEvent {
            pub events: u32,
            data: i32,
        }
        #[link(name = "c")]
        extern "C" {
            /// Returns: positive: file descriptor, negative: error
            /// Size has been unused since 2.6.8 but must be greater than zero
            pub(super) fn epoll_create(size: i32) -> i32;

            /// Returns: zero if successful, -1 if there was an error
            pub(super) fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: *mut EpollEvent) -> i32;

            /// Sigmask is actually a type of `sigset_t` which is defined in libc, and for maximum portability
            /// that type should be used. However, an i32 works for us in this case saving making this
            /// a bit easier for us to reason about
            /// Returns: 
            /// - the number of file descriptors ready for the requested I/O
            /// - zero if no file descriptors are ready
            /// - 1 if there was an error
            pub(super) fn epoll_wait(epfd: i32, events: *mut EpollEvent, maxevents: i32, timeout: i32) -> i32;
        }
    }
}
