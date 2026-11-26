//! Functions for handling the unix socket used by CommAsync

use std::os::unix::net::UnixStream;

use log::debug;
use nix::{
    errno::Errno,
    poll::PollTimeout,
    sys::epoll::{Epoll, EpollEvent, EpollFlags},
};

/// Blocks until its `stream` socket is shut down or errors out.
/// Uses the linux-specific epoll(7) api.
pub fn wait_for_socket_shutdown(stream: &UnixStream, epoll: Epoll) -> std::io::Result<()> {
    let watched_event = EpollEvent::new(EpollFlags::EPOLLRDHUP, 1337);
    epoll.add(stream, watched_event)?;

    let mut events = [EpollEvent::empty()];
    loop {
        match epoll.wait(&mut events, PollTimeout::NONE) {
            // We only want 1 since we monitor a single event
            Ok(1) => {
                let trigger_set = events[0].events();
                // Note that epoll_wait always returns on EPOLLHUP and EPOLLERR (see epoll_ctl(2))
                return trigger_set
                    .intersects(
                        EpollFlags::EPOLLRDHUP | EpollFlags::EPOLLHUP | EpollFlags::EPOLLERR,
                    )
                    .ok_or(std::io::Error::other(
                        "shouldn't happen (epoll_wait(2) returned for an unknown/unexpected event)",
                    ));
            }
            Ok(_) => {
                return Err(std::io::Error::other(
                    "shouldn't happen (see epoll_wait(2)'s RETURN VALUE section)",
                ));
            }
            Err(Errno::EINTR) => debug!("epoll was interrupted by a signal, trying to resume"),
            Err(err) => return Err(err.into()),
        }
    }
}
