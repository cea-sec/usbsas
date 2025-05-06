use crate::{ComRqUsbsas, Message, State, Status, GUI};
use iced::{
    futures::{SinkExt, Stream, StreamExt},
    stream::channel,
    time::{self, Duration},
    Subscription,
};
use std::{
    hash::Hash,
    sync::{Arc, Mutex},
};
use usbsas_comm::ProtoReqCommon;

pub fn status<I: 'static + Hash + Copy + Send + Sync>(
    id: I,
    comm: Arc<Mutex<ComRqUsbsas>>,
) -> iced::Subscription<(I, Status)> {
    Subscription::run_with_id(id, recv(comm).map(move |progress| (id, progress)))
}

fn recv(comm: Arc<Mutex<ComRqUsbsas>>) -> impl Stream<Item = Status> {
    channel(0, move |mut output| async move {
        let mut done = false;
        while !done {
            let status = match comm.lock() {
                Ok(mut guard) => match guard.recv_status() {
                    Ok(resp) => {
                        if let Ok(usbsas_proto::common::Status::AllDone) = resp.status.try_into() {
                            done = true;
                        }
                        Status::Progress(resp)
                    }
                    Err(err) => {
                        done = true;
                        Status::Error(format!("{}", err))
                    }
                },
                Err(err) => {
                    done = true;
                    Status::Error(format!("{}", err))
                }
            };
            let _ = output.send(status).await;
        }
    })
}

impl GUI {
    pub fn subscription(&self) -> Subscription<Message> {
        let subs = match self.state {
            State::Init | State::DevSelect | State::Wipe(_) | State::DiskImg | State::Done => {
                time::every(Duration::from_secs(1)).map(Message::Tick)
            }
            State::UserID => time::every(Duration::from_secs(1)).map(Message::Tick),
            State::Status(_) => status(1, self.comm.as_ref().unwrap().clone()).map(Message::Status),
            State::Reload => Subscription::run_with_id(
                2,
                channel(1, move |mut output| async move {
                    let _ = output.send(Message::Reset).await;
                }),
            ),
            _ => Subscription::none(),
        };

        Subscription::batch(vec![subs])
    }
}
