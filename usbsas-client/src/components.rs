//! Shared widget styles and text sizes.

use crate::Message;
use iced::{
    Border, Color, Length, Renderer, Shadow, Theme,
    border::Radius,
    widget::{Button, button, text},
};

/// Text size of the top menu
pub const MENU_SIZE: f32 = 30.0;
/// Text size of titles
pub const HEADER_SIZE: f32 = 20.0;
/// Default text size
pub const TXT_SIZE: f32 = 16.0;
/// Text size of secondary information
pub const OPT_SIZE: f32 = 13.0;
/// Text size of the footer
pub const FOOT_SIZE: f32 = 10.0;

/// Primary button with rounded border.
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

/// Secondary button with rounded border.
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

/// Round button of the PIN pad.
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
/// Button of the PIN pad, filling its cell, sending `message` when pressed.
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
