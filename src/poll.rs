use std::{
    io,
    marker::PhantomData,
    mem::MaybeUninit,
    os::fd::{BorrowedFd, OwnedFd},
};

use rustix::{
    event::{PollFd, PollFlags, poll},
    fd::AsFd,
    fs::{fcntl_setfl, OFlags},
    io::{Errno, fcntl_setfd, FdFlags, read_uninit, retry_on_intr, write},
    pipe::pipe,
};

pub struct Poll<'fd> {
    poll_fds: Vec<PollFd<'fd>>,
}

impl<'fd> Poll<'fd> {
    pub fn with_capacity(capacity: usize) -> Self {
        Poll { poll_fds: Vec::with_capacity(capacity) }
    }

    pub fn add_readable(&mut self, fd: &'fd impl AsFd) -> Token<'fd> {
        let index = self.poll_fds.len();
        self.poll_fds.push(PollFd::new(fd, PollFlags::IN));
        Token { index, marker: PhantomData }
    }

    pub fn poll(&mut self) -> io::Result<()> {
        let events_count = retry_on_intr(|| poll(&mut self.poll_fds, -1))?;
        assert_ne!(events_count, 0);
        Ok(())
    }

    pub fn ready(&mut self, token: Token) -> bool {
        let revents = self.poll_fds[token.index].revents();
        assert!(!revents.intersects(PollFlags::NVAL));
        !revents.is_empty()
    }
}

#[derive(Clone, Copy)]
pub struct Token<'a> {
    index: usize,
    marker: PhantomData<BorrowedFd<'a>>
}

pub enum Waker {
    Eventfd { fd: OwnedFd },
    Pipe { read_half: OwnedFd, write_half: OwnedFd },
}

impl Waker {
    pub fn new() -> io::Result<Waker> {
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd",
            target_os = "illumos",
        ))] {
            use rustix::event::{EventfdFlags, eventfd};
            if let Ok(fd) = eventfd(
                0,
                EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK
            ) {
                return Ok(Waker::Eventfd { fd });
            }
        }
        let (read_half, write_half) = pipe_cloexec_nonblock()?;
        Ok(Waker::Pipe { read_half, write_half })
    }

    pub fn wake(&self) {
        match self {
            Waker::Eventfd { fd } => assert_ok_or_wouldblock(
                write(fd, &1u64.to_ne_bytes())
            ),
            Waker::Pipe { write_half, .. } => assert_ok_or_wouldblock(
                write(write_half, &[0u8])
            ),
        }
    }

    pub fn read(&self) {
        match self {
            Waker::Eventfd { fd } => assert_ok_or_wouldblock(
                read_uninit(fd, &mut [MaybeUninit::<u8>::uninit(); 8])
            ),
            Waker::Pipe { read_half, .. } => assert_ok_or_wouldblock(
                clear_pipe(read_half)
            ),
        }
    }
}

impl AsFd for Waker {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Waker::Eventfd { fd } => fd.as_fd(),
            Waker::Pipe { read_half, .. } => read_half.as_fd(),
        }
    }
}

pub fn pipe_cloexec_nonblock() -> io::Result<(OwnedFd, OwnedFd)> {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        target_os = "illumos",
        target_os = "redox",
    ))] {
        use rustix::pipe::{PipeFlags, pipe_with};
        if let Ok(ret) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK) {
            return Ok(ret)
        }
    }
    let (read_half, write_half) = pipe()?;
    fcntl_setfd(&read_half, FdFlags::CLOEXEC)?;
    fcntl_setfd(&write_half, FdFlags::CLOEXEC)?;
    fcntl_setfl(&read_half, OFlags::NONBLOCK)?;
    fcntl_setfl(&write_half, OFlags::NONBLOCK)?;
    Ok((read_half, write_half))
}

fn clear_pipe(read_half: impl AsFd) -> Result<(), Errno> {
    const LEN: usize = 256;
    let mut buf = [MaybeUninit::<u8>::uninit(); LEN];
    loop {
        match read_uninit(&read_half, &mut buf) {
            Ok((slice, _)) => if slice.len() < LEN { return Ok(()) },
            Err(e) => return Err(e),
        }
    }
}

#[track_caller]
fn assert_ok_or_wouldblock<T>(result: Result<T, Errno>) {
    match result {
        #[allow(unreachable_patterns)]
        Ok(_) | Err(Errno::AGAIN) | Err(Errno::WOULDBLOCK) => (),
        Err(e) => panic!("{e}"),
    }
}
