//! Clavier virtuel à la souris (aucun clavier physique sur la borne :
//! le pilote HID d'usbsas ne gère que la souris, protection BadUSB).

use crate::style;
use iced::widget::{Column, Row, button, text};
use iced::{Element, Length};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Lower,
    Upper,
    Symbols,
    /// Pavé numérique (PIN)
    Digits,
}

#[derive(Debug, Clone)]
pub enum Key {
    Char(char),
    Backspace,
    Clear,
    Layout(Layout),
    ToggleVisible,
}

const LOWER: [&str; 4] = ["1234567890", "azertyuiop", "qsdfghjklm", "wxcvbn,.-_"];
const UPPER: [&str; 4] = ["1234567890", "AZERTYUIOP", "QSDFGHJKLM", "WXCVBN;:!?"];
const SYMBOLS: [&str; 4] = ["&é\"'(§è!çà", "@#$%*+=/\\|", "<>[]{}()~^", "€£°²µ`ùû?¨"];
const DIGITS: [&str; 4] = ["123", "456", "789", "0"];

/// Saisie en cours (le texte est effacé de la mémoire à la destruction).
#[derive(Default)]
pub struct Input {
    pub value: zeroize::Zeroizing<String>,
    pub layout: Layout,
    pub visible: bool,
    pub max_len: usize,
}

impl Input {
    pub fn digits(max_len: usize) -> Self {
        Input {
            layout: Layout::Digits,
            max_len,
            ..Default::default()
        }
    }

    pub fn text(max_len: usize) -> Self {
        Input {
            max_len,
            ..Default::default()
        }
    }

    pub fn press(&mut self, k: Key) {
        match k {
            Key::Char(c) if self.value.chars().count() < self.max_len => self.value.push(c),
            Key::Char(_) => (),
            Key::Backspace => {
                self.value.pop();
            }
            Key::Clear => self.value.clear(),
            Key::Layout(l) => self.layout = l,
            Key::ToggleVisible => self.visible = !self.visible,
        }
    }

    /// Texte affiché dans le champ (masqué par défaut).
    pub fn shown(&self) -> String {
        if self.visible {
            self.value.to_string()
        } else {
            "●".repeat(self.value.chars().count())
        }
    }

    pub fn view<'a, M: Clone + 'a>(&self, on: impl Fn(Key) -> M + Copy + 'a) -> Element<'a, M> {
        let rows = match self.layout {
            Layout::Lower => LOWER,
            Layout::Upper => UPPER,
            Layout::Symbols => SYMBOLS,
            Layout::Digits => DIGITS,
        };
        let size = if self.layout == Layout::Digits {
            70.0
        } else {
            54.0
        };
        let key = |label: String, msg: M, w: f32| {
            button(
                text(label)
                    .size(24)
                    .center()
                    .width(Length::Fill)
                    .shaping(text::Shaping::Advanced),
            )
            .width(Length::Fixed(w))
            .height(Length::Fixed(size))
            .style(style::key)
            .on_press(msg)
        };
        let mut col = Column::new().spacing(6).align_x(iced::Alignment::Center);
        for r in rows {
            let mut row = Row::new().spacing(6);
            for c in r.chars() {
                row = row.push(key(c.to_string(), on(Key::Char(c)), size));
            }
            col = col.push(row);
        }
        let mut ctl = Row::new().spacing(6);
        if self.layout != Layout::Digits {
            let (shift, sym) = match self.layout {
                Layout::Upper => ("minuscules", "&?@"),
                Layout::Symbols => ("MAJUSCULES", "abc"),
                _ => ("MAJUSCULES", "&?@"),
            };
            let shift_to = if self.layout == Layout::Upper {
                Layout::Lower
            } else {
                Layout::Upper
            };
            let sym_to = if self.layout == Layout::Symbols {
                Layout::Lower
            } else {
                Layout::Symbols
            };
            ctl = ctl
                .push(key(shift.into(), on(Key::Layout(shift_to)), 170.0))
                .push(key(sym.into(), on(Key::Layout(sym_to)), 90.0))
                .push(key("espace".into(), on(Key::Char(' ')), 220.0))
                .push(key(
                    if self.visible { "masquer" } else { "afficher" }.into(),
                    on(Key::ToggleVisible),
                    130.0,
                ));
        }
        ctl = ctl
            .push(key("⌫ effacer".into(), on(Key::Backspace), 160.0))
            .push(key("tout effacer".into(), on(Key::Clear), 160.0));
        col.push(ctl).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saisie() {
        let mut i = Input::digits(4);
        for c in "12345".chars() {
            i.press(Key::Char(c));
        }
        assert_eq!(i.value.as_str(), "1234");
        assert_eq!(i.shown(), "●●●●");
        i.press(Key::Backspace);
        i.press(Key::ToggleVisible);
        assert_eq!(i.shown(), "123");
        i.press(Key::Clear);
        assert!(i.value.is_empty());
    }
}
