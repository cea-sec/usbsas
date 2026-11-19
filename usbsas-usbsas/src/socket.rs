//! Functions for handling the unix socket used by CommAsync

use std::os::unix::net::UnixStream;

use nix::{
    poll::PollTimeout,
    sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags},
};

/// Generic function that blocks until its `stream` is shutdown or errors out.
/// Uses the linux-specific epoll(7) api.
pub fn wait_for_socket_shutdown(stream: &UnixStream) -> Result<(), std::io::Error> {
    let epoll = Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?;
    let watched_event = EpollEvent::new(EpollFlags::EPOLLRDHUP, 1337);
    epoll.add(stream, watched_event)?;

    let mut events = [EpollEvent::empty()];
    match epoll.wait(&mut events, PollTimeout::NONE) {
        Ok(1) => {
            let trigger_set = events[0].events();
            // Note that epoll_wait always returns on EPOLLHUP and EPOLLERR (see epoll_ctl(2))
            trigger_set
                .intersects(EpollFlags::EPOLLRDHUP | EpollFlags::EPOLLHUP | EpollFlags::EPOLLERR)
                .ok_or(std::io::Error::other(
                    "shouldn't happen (epoll_wait(2) returned for an unknown/unexpected event)",
                ))
        }
        Ok(_) => Err(std::io::Error::other(
            "shouldn't happen (see epoll_wait(2)'s RETURN VALUE section)",
        )),
        Err(err) => Err(err.into()),
    }
}
