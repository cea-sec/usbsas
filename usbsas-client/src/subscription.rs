//! Periodic events.

use crate::{GUI, Message, State};
use iced::{
    Subscription,
    time::{self, Duration},
};

impl GUI {
    /// Emit a `Message::Tick` every second in states that need to sandbox,
    /// connect or poll usbsas (for devices or user id).
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
        let frames = match self.state {
            State::Sandbox | State::Connect | State::Init | State::Reload
                if self.comm.is_none() =>
            {
                iced::window::frames().map(Message::Frame)
            }
            _ => Subscription::none(),
        };
        Subscription::batch(vec![subs, frames])
    }
}
