use crate::{Message, State, GUI};
use iced::{
    time::{self, Duration},
    Subscription,
};

impl GUI {
    pub fn subscription(&self) -> Subscription<Message> {
        let subs = match self.state {
            State::Connect
            | State::Sandbox
            | State::Init
            | State::DevSelect
            | State::Wipe(_)
            | State::DiskImg
            | State::Done
            | State::SysInfo
            | State::UserID => time::every(Duration::from_secs(1)).map(Message::Tick),
            _ => Subscription::none(),
        };

        Subscription::batch(vec![subs])
    }
}
