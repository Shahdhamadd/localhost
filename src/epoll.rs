use std::io;

pub const READ: u32 = libc::EPOLLIN as u32;
pub const WRITE: u32 = libc::EPOLLOUT as u32;

pub fn create() -> io::Result<i32> {
    let epoll = unsafe { libc::epoll_create1(0) };
    if epoll < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(epoll)
}

pub fn add(epoll: i32, fd: i32, events: u32) -> io::Result<()> {
    control(epoll, libc::EPOLL_CTL_ADD, fd, events)
}

pub fn change(epoll: i32, fd: i32, events: u32) -> io::Result<()> {
    control(epoll, libc::EPOLL_CTL_MOD, fd, events)
}

pub fn remove(epoll: i32, fd: i32) {
    let _ = control(epoll, libc::EPOLL_CTL_DEL, fd, 0);
}

fn control(epoll: i32, action: i32, fd: i32, events: u32) -> io::Result<()> {
    let mut event = libc::epoll_event { events, u64: fd as u64 };
    let result = unsafe { libc::epoll_ctl(epoll, action, fd, &mut event) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn wait(epoll: i32, events: &mut [libc::epoll_event]) -> usize {
    let count = unsafe { libc::epoll_wait(epoll, events.as_mut_ptr(), events.len() as i32, 1000) };
    if count < 0 {
        return 0;
    }
    count as usize
}
