use std::io;
use std::ops::Deref;
use std::os::unix::io::{AsRawFd, RawFd};

use libc::{
    epoll_create1, epoll_ctl, epoll_event, epoll_wait, EPOLL_CLOEXEC,
    EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLL_CTL_MOD,
};

pub const EPOLL_IN: u32 = libc::EPOLLIN as u32;
pub const EPOLL_OUT: u32 = libc::EPOLLOUT as u32;

fn cvt(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

#[repr(i32)]
pub enum ControlOperation {
    /// Add a file descriptor to the interest list.
    Add = EPOLL_CTL_ADD,
    /// Change the settings associated with a file descriptor that is
    /// already in the interest list.
    Modify = EPOLL_CTL_MOD,
    /// Remove a file descriptor from the interest list.
    Delete = EPOLL_CTL_DEL,
}

#[derive(Default)]
pub struct EventSet {
    flags: u32,
}

impl EventSet {
    pub fn new(flags: u32) -> Self {
        Self {
            flags,
        }
    }

    pub fn add(&mut self, flag: u32) -> &Self {
        self.flags = self.flags | flag;
        self
    }

    pub fn remove(&mut self, flag: u32) -> &Self {
        self.flags = self.flags ^ flag;
        self
    }

    pub fn contains(&self, flag: u32) -> bool {
        if self.flags & flag != 0 {
            true
        } else {
            false
        }
    }

    pub fn bits(&self) -> u32 {
        self.flags
    }
}


#[repr(transparent)]
#[derive(Clone)]
pub struct EpollEvent(epoll_event);

impl Deref for EpollEvent {
    type Target = epoll_event;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for EpollEvent {
    fn default() -> Self {
        EpollEvent(epoll_event {
            events: 0u32,
            u64: 0u64,
        })
    }
}

impl EpollEvent {
    /// Create a new epoll_event instance with the following fields: `events`, which contains
    /// an event mask and `data` which represents a user data variable. `data` field can be
    /// a fd on which we want to monitor the events specified by `events`.
    pub fn new(events: EventSet, data: u64) -> Self {
        EpollEvent(epoll_event {
            events: events.bits(),
            u64: data,
        })
    }

    /// Returns the `events` from `libc::epoll_event`.
    pub fn events(&self) -> u32 {
        self.events
    }

    pub fn event_set(&self) -> EventSet {
        EventSet::new(self.events())
    }

    /// Returns the `data` from the `libc::epoll_event`.
    pub fn data(&self) -> u64 {
        self.u64
    }

    /// Converts the `libc::epoll_event` data to a RawFd.
    ///
    /// This conversion is lossy when the data does not correspond to a RawFd
    /// (data does not fit in a i32).
    pub fn fd(&self) -> RawFd {
        self.u64 as i32
    }
}

#[derive(Debug)]
pub struct Epoll {
    epoll_fd: RawFd,
}

impl Epoll {
    /// Create a new epoll file descriptor.
    pub fn new() -> io::Result<Self> {
        let epoll_fd = cvt(unsafe { epoll_create1(EPOLL_CLOEXEC) })? as RawFd;
        Ok(Epoll { epoll_fd })
    }

    pub fn ctl(
        &self,
        operation: ControlOperation,
        fd: RawFd,
        event: &EpollEvent,
    ) -> io::Result<()> {
        cvt(unsafe {
            epoll_ctl(
                self.epoll_fd,
                operation as i32,
                fd,
                event as *const EpollEvent as *mut epoll_event,
            )
        })?;
        Ok(())
    }

    pub fn wait(
        &self,
        max_events: usize,
        events: &mut [EpollEvent],
    ) -> io::Result<usize> {
        // Safe because we give a valid epoll file descriptor and an array of epoll_event structures
        // that will be modified by the kernel to indicate information about the subset of file
        // descriptors in the interest list. We also check the return value.
        let events_count = cvt(unsafe {
            epoll_wait(
                self.epoll_fd,
                events.as_mut_ptr() as *mut epoll_event,
                max_events as i32,
                -1,
            )
        })? as usize;

        Ok(events_count)
    }
}

impl AsRawFd for Epoll {
    fn as_raw_fd(&self) -> RawFd {
        self.epoll_fd
    }
}

impl std::ops::Drop for Epoll {
    fn drop(&mut self) {
        // Safe because this fd is opened with `epoll_create` and we trust
        // the kernel to give us a valid fd.
        unsafe {
            libc::close(self.epoll_fd);
        }
    }
}