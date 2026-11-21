//! `Comm::from_env()` and `RawFd::from_env()` read process-wide state (the
//! environment and the "pipes taken" flag), so they are tested in their own
//! test binary, in a single test that runs alone.

use std::{
    env, io,
    os::fd::{FromRawFd, IntoRawFd, OwnedFd, RawFd},
};
use usbsas_comm::{
    ComRpUsbsas, ComRqUsbsas, Comm, FromEnv, FromFd, ProtoReqUsbsas, ProtoRespUsbsas,
};
use usbsas_proto::usbsas::{RequestUserId, ResponseUserId, request};
use usbsas_utils::{INPUT_PIPE_FD_VAR, OUTPUT_PIPE_FD_VAR};

fn set_fds(input: impl ToString, output: impl ToString) {
    // SAFETY: this is the only test of this binary, no other thread reads the
    // environment.
    unsafe {
        env::set_var(INPUT_PIPE_FD_VAR, input.to_string());
        env::set_var(OUTPUT_PIPE_FD_VAR, output.to_string());
    }
}

fn raw_pipe() -> (RawFd, RawFd) {
    let (r, w) = io::pipe().unwrap();
    (
        OwnedFd::from(r).into_raw_fd(),
        OwnedFd::from(w).into_raw_fd(),
    )
}

#[test]
fn from_env() {
    // RawFd::from_env
    set_fds(12, "twelve");
    assert_eq!(RawFd::from_env(INPUT_PIPE_FD_VAR).unwrap(), 12);
    let err = RawFd::from_env(OUTPUT_PIPE_FD_VAR).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert!(err.to_string().starts_with(OUTPUT_PIPE_FD_VAR));
    let err = RawFd::from_env("USBSAS_COMM_TEST_UNSET_VAR").unwrap_err();
    assert!(err.to_string().starts_with("USBSAS_COMM_TEST_UNSET_VAR"));

    // Missing or invalid values
    unsafe {
        env::remove_var(INPUT_PIPE_FD_VAR);
        env::remove_var(OUTPUT_PIPE_FD_VAR);
    }
    assert!(ComRpUsbsas::from_env().is_err());
    set_fds("", "");
    assert!(ComRpUsbsas::from_env().is_err());
    set_fds(-1, 1);
    assert_eq!(
        ComRpUsbsas::from_env().err().unwrap().kind(),
        io::ErrorKind::InvalidInput
    );

    // Valid pipes in the wrong direction or the same fd twice: rejected, and
    // the failed attempts don't count as taking the pipes.
    let (r, w) = raw_pipe();
    set_fds(w, r);
    assert_eq!(
        ComRpUsbsas::from_env().err().unwrap().kind(),
        io::ErrorKind::InvalidInput
    );
    set_fds(r, r);
    assert_eq!(
        ComRpUsbsas::from_env().err().unwrap().kind(),
        io::ErrorKind::InvalidInput
    );

    // Valid pipes: the child reads requests from `r` and writes responses to
    // `w2`, the parent writes requests to `w` and reads from `r2`.
    let (r2, w2) = raw_pipe();
    set_fds(r, w2);
    let mut child: ComRpUsbsas = Comm::from_env().unwrap();

    // Taken only once
    assert_eq!(
        ComRpUsbsas::from_env().err().unwrap().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        ComRqUsbsas::from_env().err().unwrap().kind(),
        io::ErrorKind::AlreadyExists
    );

    // SAFETY: r2 and w are open and owned by nothing else
    let (r2, w) = unsafe { (OwnedFd::from_raw_fd(r2), OwnedFd::from_raw_fd(w)) };
    let mut parent = ComRqUsbsas::from_fd(r2, w);
    let responder = std::thread::spawn(move || {
        assert_eq!(
            child.recv_req().unwrap(),
            request::Msg::UserId(RequestUserId {})
        );
        child
            .userid(ResponseUserId {
                userid: "user".into(),
            })
            .unwrap();
    });
    assert_eq!(parent.userid(RequestUserId {}).unwrap().userid, "user");
    responder.join().unwrap();
}
