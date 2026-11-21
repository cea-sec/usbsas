use super::*;
use prost::Message;
use std::{io::Cursor, os::fd::IntoRawFd};
use usbsas_proto::{
    common::{FileInfo, ResponseEnd, ResponseError, ResponseStatus, Status},
    usbsas::{
        Request, RequestEnd, RequestReadDir, RequestUserId, Response, ResponseReadDir,
        ResponseUserId, request, response,
    },
};

/// In memory channel: reads from `input`, writes to a `Vec`.
type MemComm<U> = Comm<U, Cursor<Vec<u8>>, Vec<u8>>;

fn mem_comm<U>(input: Vec<u8>) -> MemComm<U> {
    Comm {
        input: Cursor::new(input),
        output: Vec::new(),
        req: PhantomData,
    }
}

/// Encode messages the way `send` does.
fn frames<T: Message>(msgs: impl IntoIterator<Item = T>) -> Vec<u8> {
    let mut buf = Vec::new();
    for msg in msgs {
        buf.extend_from_slice(&(msg.encoded_len() as u64).to_le_bytes());
        msg.encode(&mut buf).unwrap();
    }
    buf
}

fn frame<T: Message>(msg: T) -> Vec<u8> {
    frames([msg])
}

/// Decode all the frames written by a channel.
fn decode_all<T: Message + Default>(mut buf: &[u8]) -> Vec<T> {
    let mut msgs = Vec::new();
    while !buf.is_empty() {
        let len = u64::from_le_bytes(buf[..8].try_into().unwrap()) as usize;
        msgs.push(T::decode(&buf[8..8 + len]).unwrap());
        buf = &buf[8 + len..];
    }
    msgs
}

fn resp(msg: response::Msg) -> Response {
    Response { msg: Some(msg) }
}

fn req(msg: request::Msg) -> Request {
    Request { msg: Some(msg) }
}

fn readdir_resp() -> ResponseReadDir {
    ResponseReadDir {
        filesinfo: vec![FileInfo {
            path: "/dir/file".into(),
            ftype: 1,
            size: 42,
            timestamp: 1_700_000_000,
        }],
        max_depth_reached: false,
    }
}

// Wire format

#[test]
fn send_writes_le_length_prefix() {
    let msg = RequestReadDir {
        path: "/some/path".into(),
    };
    let mut comm: MemComm<()> = mem_comm(Vec::new());
    comm.send(msg.clone()).unwrap();

    let len = msg.encoded_len();
    assert_eq!(comm.output.len(), 8 + len);
    assert_eq!(comm.output[..8], (len as u64).to_le_bytes());
    assert_eq!(comm.output[8..], msg.encode_to_vec());
}

#[test]
fn send_empty_message() {
    let mut comm: MemComm<()> = mem_comm(Vec::new());
    comm.send(RequestEnd {}).unwrap();
    assert_eq!(comm.output, 0u64.to_le_bytes());

    let mut comm: MemComm<()> = mem_comm(comm.output);
    assert_eq!(comm.recv::<RequestEnd>().unwrap(), RequestEnd {});
}

#[test]
fn send_recv_roundtrip() {
    let msgs = vec![
        req(request::Msg::ReadDir(RequestReadDir { path: "/a".into() })),
        req(request::Msg::UserId(RequestUserId {})),
        req(request::Msg::End(RequestEnd {})),
    ];
    let mut tx: MemComm<()> = mem_comm(Vec::new());
    for msg in &msgs {
        tx.send(msg.clone()).unwrap();
    }
    let mut rx: MemComm<()> = mem_comm(tx.output);
    for msg in msgs {
        assert_eq!(rx.recv::<Request>().unwrap(), msg);
    }
    // nothing left
    let err = rx.recv::<Request>().unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn recv_rejects_oversized_message() {
    let mut input = (MAX_PROTO_MESS_SIZE + 1).to_le_bytes().to_vec();
    input.extend_from_slice(&[0; 16]);
    let mut comm: MemComm<()> = mem_comm(input);
    let err = comm.recv::<Request>().unwrap_err();
    assert_eq!(err.to_string(), "Message size exceeded");
    // only the length was consumed, the body wasn't read
    assert_eq!(comm.input.position(), 8);
}

#[test]
fn recv_rejects_huge_length() {
    let mut comm: MemComm<()> = mem_comm(u64::MAX.to_le_bytes().to_vec());
    let err = comm.recv::<Request>().unwrap_err();
    assert_eq!(err.to_string(), "Message size exceeded");
}

#[test]
fn recv_accepts_max_size() {
    // A max size length passes the check: the read then fails on the
    // missing body, not on the size.
    let mut comm: MemComm<()> = mem_comm(MAX_PROTO_MESS_SIZE.to_le_bytes().to_vec());
    let err = comm.recv::<Request>().unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn recv_truncated_length() {
    let mut comm: MemComm<()> = mem_comm(vec![1, 0, 0]);
    let err = comm.recv::<Request>().unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn recv_truncated_body() {
    let mut input = frame(RequestReadDir {
        path: "/some/path".into(),
    });
    input.truncate(input.len() - 1);
    let mut comm: MemComm<()> = mem_comm(input);
    let err = comm.recv::<RequestReadDir>().unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn recv_invalid_protobuf() {
    // field 1, length delimited, announces 10 bytes but there's only 1
    let body = [0x0a, 0x0a, 0x00];
    let mut input = (body.len() as u64).to_le_bytes().to_vec();
    input.extend_from_slice(&body);
    let mut comm: MemComm<()> = mem_comm(input);
    let err = comm.recv::<RequestReadDir>().unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn raw_read_write_bypass_framing() {
    let mut comm: MemComm<()> = mem_comm(vec![1, 2, 3, 4]);
    comm.write_all(&[0xaa, 0xbb]).unwrap();
    comm.flush().unwrap();
    assert_eq!(comm.output, [0xaa, 0xbb]);

    let mut buf = [0; 4];
    comm.read_exact(&mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 4]);
}

// Requester side

#[test]
fn request_ok() {
    let mut comm: MemComm<Request> = mem_comm(frame(resp(response::Msg::ReadDir(readdir_resp()))));
    let rep = ProtoReqUsbsas::readdir(
        &mut comm,
        RequestReadDir {
            path: "/dir".into(),
        },
    )
    .unwrap();
    assert_eq!(rep, readdir_resp());

    // the request was sent before the response was read
    let sent: Vec<Request> = decode_all(&comm.output);
    assert_eq!(
        sent,
        [req(request::Msg::ReadDir(RequestReadDir {
            path: "/dir".into()
        }))]
    );
}

#[test]
fn request_error_response() {
    let mut comm: MemComm<Request> = mem_comm(frame(resp(response::Msg::Error(ResponseError {
        err: "no such dir".into(),
    }))));
    let err = ProtoReqUsbsas::readdir(
        &mut comm,
        RequestReadDir {
            path: "/dir".into(),
        },
    )
    .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Other);
    assert_eq!(err.to_string(), "no such dir");
}

#[test]
fn request_unexpected_response() {
    // response to another request
    let mut comm: MemComm<Request> = mem_comm(frame(resp(response::Msg::UserId(ResponseUserId {
        userid: "user".into(),
    }))));
    let err = ProtoReqUsbsas::readdir(&mut comm, RequestReadDir::default()).unwrap_err();
    assert_eq!(err.to_string(), "Unexpected response");

    // a status
    let mut comm: MemComm<Request> = mem_comm(frame(resp(response::Msg::Status(
        ResponseStatus::default(),
    ))));
    let err = ProtoReqUsbsas::readdir(&mut comm, RequestReadDir::default()).unwrap_err();
    assert_eq!(err.to_string(), "Unexpected response");

    // no message
    let mut comm: MemComm<Request> = mem_comm(frame(Response { msg: None }));
    let err = ProtoReqUsbsas::readdir(&mut comm, RequestReadDir::default()).unwrap_err();
    assert_eq!(err.to_string(), "Unexpected response");
}

#[test]
fn request_no_response() {
    let mut comm: MemComm<Request> = mem_comm(Vec::new());
    let err = ProtoReqUsbsas::userid(&mut comm, RequestUserId {}).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn request_end() {
    let mut comm: MemComm<Request> = mem_comm(frame(resp(response::Msg::End(ResponseEnd {}))));
    ProtoReqCommon::end(&mut comm).unwrap();
    let sent: Vec<Request> = decode_all(&comm.output);
    assert_eq!(sent, [req(request::Msg::End(RequestEnd {}))]);
}

#[test]
fn request_end_error() {
    let mut comm: MemComm<Request> = mem_comm(frame(resp(response::Msg::Error(ResponseError {
        err: "busy".into(),
    }))));
    let err = ProtoReqCommon::end(&mut comm).unwrap_err();
    assert_eq!(err.to_string(), "busy");

    let mut comm: MemComm<Request> = mem_comm(frame(resp(response::Msg::UserId(
        ResponseUserId::default(),
    ))));
    let err = ProtoReqCommon::end(&mut comm).unwrap_err();
    assert_eq!(err.to_string(), "Unexpected response");
}

#[test]
fn request_recv_status() {
    let statuses = [
        ResponseStatus {
            current: 1,
            total: 3,
            done: false,
            status: Status::ReadSrc.into(),
        },
        ResponseStatus {
            current: 3,
            total: 3,
            done: true,
            status: Status::ReadSrc.into(),
        },
    ];
    let mut comm: MemComm<Request> = mem_comm(frames(
        statuses
            .iter()
            .cloned()
            .map(|s| resp(response::Msg::Status(s))),
    ));
    for status in statuses {
        assert_eq!(comm.recv_status().unwrap(), status);
    }
    // recv_status doesn't send anything
    assert!(comm.output.is_empty());
}

#[test]
fn request_recv_status_error() {
    let mut comm: MemComm<Request> = mem_comm(frames([
        resp(response::Msg::Error(ResponseError {
            err: "copy failed".into(),
        })),
        resp(response::Msg::End(ResponseEnd {})),
    ]));
    assert_eq!(comm.recv_status().unwrap_err().to_string(), "copy failed");
    assert_eq!(
        comm.recv_status().unwrap_err().to_string(),
        "Unexpected response"
    );
}

// Responder side

#[test]
fn responder_recv_req() {
    let mut comm: MemComm<Response> = mem_comm(frames([
        req(request::Msg::ReadDir(RequestReadDir { path: "/x".into() })),
        req(request::Msg::End(RequestEnd {})),
    ]));
    assert_eq!(
        comm.recv_req().unwrap(),
        request::Msg::ReadDir(RequestReadDir { path: "/x".into() })
    );
    assert_eq!(comm.recv_req().unwrap(), request::Msg::End(RequestEnd {}));
    assert_eq!(
        comm.recv_req().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

#[test]
fn responder_recv_req_without_msg() {
    let mut comm: MemComm<Response> = mem_comm(frame(Request { msg: None }));
    let err = comm.recv_req().unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn responder_send_responses() {
    let mut comm: MemComm<Response> = mem_comm(Vec::new());
    ProtoRespUsbsas::readdir(&mut comm, readdir_resp()).unwrap();
    comm.status(10, 20, false, Status::MkFs).unwrap();
    comm.done(Status::MkArchive).unwrap();
    comm.error("oops").unwrap();
    comm.error(io::Error::other("io oops")).unwrap();
    ProtoRespCommon::end(&mut comm).unwrap();

    let sent: Vec<Response> = decode_all(&comm.output);
    assert_eq!(
        sent,
        [
            resp(response::Msg::ReadDir(readdir_resp())),
            resp(response::Msg::Status(ResponseStatus {
                current: 10,
                total: 20,
                done: false,
                status: Status::MkFs.into(),
            })),
            resp(response::Msg::Status(ResponseStatus {
                current: 0,
                total: 0,
                done: true,
                status: Status::MkArchive.into(),
            })),
            resp(response::Msg::Error(ResponseError { err: "oops".into() })),
            resp(response::Msg::Error(ResponseError {
                err: "io oops".into()
            })),
            resp(response::Msg::End(ResponseEnd {})),
        ]
    );
}

// Requester and responder talking to each other over pipes

fn pipe_files() -> (File, File) {
    let (r, w) = io::pipe().unwrap();
    (File::from(OwnedFd::from(r)), File::from(OwnedFd::from(w)))
}

/// Requester and responder connected with two pipes.
fn comm_pair() -> (ComRqUsbsas, ComRpUsbsas) {
    let (rq_in, rp_out) = pipe_files();
    let (rp_in, rq_out) = pipe_files();
    (Comm::new(rq_in, rq_out), Comm::new(rp_in, rp_out))
}

#[test]
fn requester_responder_over_pipes() {
    let (mut rq, mut rp) = comm_pair();

    let responder = std::thread::spawn(move || {
        loop {
            match rp.recv_req().unwrap() {
                request::Msg::UserId(_) => rp
                    .userid(ResponseUserId {
                        userid: "alice".into(),
                    })
                    .unwrap(),
                request::Msg::ReadDir(req) if req.path == "/" => {
                    rp.readdir(readdir_resp()).unwrap()
                }
                request::Msg::ReadDir(req) => rp.error(format!("{}: not found", req.path)).unwrap(),
                request::Msg::Wipe(_) => {
                    for i in 0..3 {
                        rp.status(i, 3, false, Status::MkFs).unwrap();
                    }
                    rp.done(Status::MkFs).unwrap();
                }
                request::Msg::End(_) => {
                    ProtoRespCommon::end(&mut rp).unwrap();
                    break;
                }
                req => panic!("unexpected request {req:?}"),
            }
        }
    });

    assert_eq!(rq.userid(RequestUserId {}).unwrap().userid, "alice");
    assert_eq!(
        rq.readdir(RequestReadDir { path: "/".into() }).unwrap(),
        readdir_resp()
    );
    let err = rq
        .readdir(RequestReadDir {
            path: "/nope".into(),
        })
        .unwrap_err();
    assert_eq!(err.to_string(), "/nope: not found");
    // the channel is still usable after an error response
    assert_eq!(rq.userid(RequestUserId {}).unwrap().userid, "alice");

    rq.send(req(request::Msg::Wipe(Default::default())))
        .unwrap();
    let mut currents = Vec::new();
    loop {
        let status = rq.recv_status().unwrap();
        if status.done {
            break;
        }
        currents.push(status.current);
    }
    assert_eq!(currents, [0, 1, 2]);

    ProtoReqCommon::end(&mut rq).unwrap();
    responder.join().unwrap();
}

#[test]
fn recv_fails_when_peer_exits() {
    let (mut rq, rp) = comm_pair();
    drop(rp);
    // EPIPE on send or EOF on recv, depending on the ordering
    assert!(rq.userid(RequestUserId {}).is_err());
}

#[test]
fn large_message_over_pipes() {
    // bigger than a pipe buffer
    let (mut rq, mut rp) = comm_pair();
    let filesinfo: Vec<FileInfo> = (0..20_000)
        .map(|i| FileInfo {
            path: format!("/dir/file_{i}"),
            ..Default::default()
        })
        .collect();
    let expected = ResponseReadDir {
        filesinfo: filesinfo.clone(),
        max_depth_reached: true,
    };
    let responder = std::thread::spawn(move || {
        let request::Msg::ReadDir(_) = rp.recv_req().unwrap() else {
            panic!("unexpected request");
        };
        rp.readdir(ResponseReadDir {
            filesinfo,
            max_depth_reached: true,
        })
        .unwrap();
    });
    assert_eq!(rq.readdir(RequestReadDir::default()).unwrap(), expected);
    responder.join().unwrap();
}

#[test]
fn from_fd_and_to_fd() {
    let (r, w) = io::pipe().unwrap();
    let (r, w) = (OwnedFd::from(r), OwnedFd::from(w));
    let (r_raw, w_raw) = (r.as_raw_fd(), w.as_raw_fd());
    let mut comm = ComRqUsbsas::from_fd(r, w);
    assert_eq!(comm.input_fd(), r_raw);
    assert_eq!(comm.output_fd(), w_raw);
    assert_eq!(comm.input().as_raw_fd(), r_raw);
    assert_eq!(comm.output().as_raw_fd(), w_raw);

    // output is connected to input
    comm.send(RequestUserId {}).unwrap();
    assert_eq!(comm.recv::<RequestUserId>().unwrap(), RequestUserId {});
}

// Pipes from the environment

fn raw_pipe() -> (OwnedFd, OwnedFd) {
    let (r, w) = io::pipe().unwrap();
    (OwnedFd::from(r), OwnedFd::from(w))
}

#[test]
fn check_pipe_fd_rejects_std_and_negative_fds() {
    for fd in [-1, 0, 1, 2] {
        let err = check_pipe_fd(fd, libc::O_RDONLY).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}

#[test]
fn check_pipe_fd_access_mode() {
    let (r, w) = raw_pipe();
    check_pipe_fd(r.as_raw_fd(), libc::O_RDONLY).unwrap();
    check_pipe_fd(w.as_raw_fd(), libc::O_WRONLY).unwrap();
    for (fd, mode) in [(&r, libc::O_WRONLY), (&w, libc::O_RDONLY)] {
        let err = check_pipe_fd(fd.as_raw_fd(), mode).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}

#[test]
fn check_pipe_fd_closed_fd() {
    // not a recently closed fd: other tests running in parallel could reuse
    // its number. This one is above any RLIMIT_NOFILE.
    let err = check_pipe_fd(RawFd::MAX, libc::O_RDONLY).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::EBADF));
}

fn cloexec(fd: RawFd) -> bool {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_ne!(flags, -1);
    flags & libc::FD_CLOEXEC != 0
}

#[test]
fn take_pipe_fds_ok() {
    let (r, w) = raw_pipe();
    let (r_raw, w_raw) = (r.into_raw_fd(), w.into_raw_fd());
    for fd in [r_raw, w_raw] {
        assert_ne!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, -1);
        assert!(!cloexec(fd));
    }
    let (r, w) = unsafe { take_pipe_fds(r_raw, w_raw) }.unwrap();
    assert_eq!((r.as_raw_fd(), w.as_raw_fd()), (r_raw, w_raw));
    assert!(cloexec(r_raw));
    assert!(cloexec(w_raw));
}

#[test]
fn take_pipe_fds_rejects_invalid_pipes() {
    let (r, w) = raw_pipe();
    let (r, w) = (r.as_raw_fd(), w.as_raw_fd());
    for (input, output) in [(r, r), (w, w), (w, r), (r, 2), (0, w)] {
        let err = unsafe { take_pipe_fds(input, output) }.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
    // ownership wasn't taken: the fds are still open
    check_pipe_fd(r, libc::O_RDONLY).unwrap();
    check_pipe_fd(w, libc::O_WRONLY).unwrap();
}
