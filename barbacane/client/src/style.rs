//! Charte graphique : grands boutons pour la souris, contrastes élevés.

use iced::widget::{button, container};
use iced::{Background, Border, Color, Shadow, Theme, border::Radius};

pub const BLEU: Color = Color::from_rgb(0.07, 0.20, 0.38);
pub const BLEU_CLAIR: Color = Color::from_rgb(0.16, 0.38, 0.66);
pub const FOND: Color = Color::from_rgb(0.95, 0.96, 0.98);
pub const TEXTE: Color = Color::from_rgb(0.10, 0.11, 0.13);
pub const VERT: Color = Color::from_rgb(0.05, 0.45, 0.22);
pub const ROUGE: Color = Color::from_rgb(0.70, 0.05, 0.10);
pub const ORANGE: Color = Color::from_rgb(0.75, 0.40, 0.0);
pub const GRIS: Color = Color::from_rgb(0.45, 0.47, 0.50);

pub const TITRE: f32 = 40.0;
pub const SOUS_TITRE: f32 = 28.0;
pub const TEXTE_TAILLE: f32 = 22.0;
pub const PETIT: f32 = 17.0;

fn base(bg: Color, fg: Color, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Color { a: 0.88, ..bg },
        button::Status::Pressed => Color { a: 0.75, ..bg },
        button::Status::Disabled => Color { a: 0.35, ..bg },
        button::Status::Active => bg,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: Border::default().rounded(Radius::new(12)),
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn primaire(_: &Theme, s: button::Status) -> button::Style {
    base(BLEU_CLAIR, Color::WHITE, s)
}

pub fn secondaire(_: &Theme, s: button::Status) -> button::Style {
    let mut st = base(Color::WHITE, BLEU, s);
    st.border = Border::default()
        .rounded(Radius::new(12))
        .width(2)
        .color(BLEU_CLAIR);
    st
}

pub fn danger(_: &Theme, s: button::Status) -> button::Style {
    base(ROUGE, Color::WHITE, s)
}

pub fn discret(_: &Theme, s: button::Status) -> button::Style {
    base(Color::TRANSPARENT, GRIS, s)
}

pub fn key(_: &Theme, s: button::Status) -> button::Style {
    let mut st = base(Color::WHITE, TEXTE, s);
    st.border = Border::default()
        .rounded(Radius::new(8))
        .width(1)
        .color(GRIS);
    st
}

pub fn bandeau(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BLEU)),
        text_color: Some(Color::WHITE),
        ..Default::default()
    }
}

pub fn carte(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::WHITE)),
        border: Border::default()
            .rounded(Radius::new(16))
            .width(1)
            .color(Color::from_rgb(0.85, 0.87, 0.90)),
        text_color: Some(TEXTE),
        ..Default::default()
    }
}

pub fn alerte(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb(1.0, 0.95, 0.85))),
        border: Border::default()
            .rounded(Radius::new(10))
            .width(2)
            .color(ORANGE),
        text_color: Some(TEXTE),
        ..Default::default()
    }
}

pub fn fond(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(FOND)),
        text_color: Some(TEXTE),
        ..Default::default()
    }
}
