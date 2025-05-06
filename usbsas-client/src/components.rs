use crate::Message;
use iced::{
    border::Radius,
    widget::{button, text, Button},
    Border, Color, Length, Renderer, Shadow, Theme,
};

//use iced::widget::{container, Container, Space};
//pub fn space<'a>(
//    width: impl Into<Length>,
//    height: impl Into<Length>,
//) -> Container<'a, Message, Theme, Renderer> {
//    let bg = iced::Background::Color(iced::Color {
//        r: 0.5,
//        g: 0.0,
//        b: 0.0,
//        a: 1.0,
//    });
//    Container::new(Space::new(width, height)).style(move |_| container::background(bg))
//}
//pub fn space2<'a>(
//    width: impl Into<Length>,
//    height: impl Into<Length>,
//) -> Container<'a, Message, Theme, Renderer> {
//    let bg = iced::Background::Color(iced::Color {
//        r: 0.0,
//        g: 0.5,
//        b: 0.0,
//        a: 1.0,
//    });
//    Container::new(Space::new(width, height)).style(move |_| container::background(bg))
//}

pub const MENU_SIZE: f32 = 30.0;
pub const HEADER_SIZE: f32 = 20.0;
pub const TXT_SIZE: f32 = 16.0;
pub const OPT_SIZE: f32 = 13.0;
pub const FOOT_SIZE: f32 = 10.0;

pub fn style_primary(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::primary(theme, status);
    style.border = Border::default()
        .rounded(Radius::new(5))
        .width(0.5)
        .color(Color::from_rgba(0.5, 0.5, 0.5, 0.7));
    style.shadow = Shadow {
        blur_radius: 50.0,
        ..Default::default()
    };
    style
}

pub fn style_secondary(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::secondary(theme, status);
    style.border = Border::default()
        .rounded(Radius::new(5))
        .width(0.5)
        .color(Color::from_rgba(0.5, 0.5, 0.5, 0.7));
    style.shadow = Shadow {
        blur_radius: 50.0,
        ..Default::default()
    };
    style
}

pub fn style_numpad(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::secondary(theme, status);
    style.border = Border::default()
        .rounded(Radius::new(50))
        .width(1)
        .color(Color::from_rgba(0.5, 0.5, 0.5, 0.7));
    style.shadow = Shadow {
        blur_radius: 50.0,
        ..Default::default()
    };
    style
}

use iced::advanced::text::IntoFragment;
pub fn button_numpad<'a>(
    content: impl IntoFragment<'a>,
    message: Message,
) -> Button<'a, Message, Theme, Renderer> {
    button(
        text(content)
            .shaping(text::Shaping::Advanced)
            .center()
            .size(HEADER_SIZE)
            .height(Length::Fill)
            .width(Length::Fill),
    )
    .style(style_numpad)
    .height(Length::Fill)
    .width(Length::Fill)
    .on_press(message)
}
