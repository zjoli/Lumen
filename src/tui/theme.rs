use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

impl Theme {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
    pub fn base(self) -> Style {
        match self {
            Self::Auto => Style::new().fg(Color::Reset).bg(Color::Reset),
            Self::Light => Style::new()
                .fg(Color::Rgb(37, 43, 65))
                .bg(Color::Rgb(250, 250, 253)),
            Self::Dark => Style::new()
                .fg(Color::Rgb(218, 222, 235))
                .bg(Color::Rgb(27, 29, 38)),
        }
    }
    pub fn accent(self) -> Style {
        self.base().fg(match self {
            Self::Auto => Color::Blue,
            Self::Light => Color::Rgb(61, 91, 176),
            Self::Dark => Color::Rgb(145, 174, 245),
        })
    }
    pub fn violet(self) -> Style {
        self.base().fg(match self {
            Self::Auto => Color::Magenta,
            Self::Light => Color::Rgb(125, 86, 196),
            Self::Dark => Color::Rgb(185, 154, 243),
        })
    }
    pub fn surface(self) -> Style {
        match self {
            Self::Auto => self.base(),
            Self::Light => self.base().bg(Color::Rgb(240, 242, 249)),
            Self::Dark => self.base().bg(Color::Rgb(36, 39, 51)),
        }
    }
    pub fn muted(self) -> Style {
        match self {
            // The host knows its actual contrast; do not fade text on light terminals.
            Self::Auto => self.base(),
            Self::Light => self.base().fg(Color::Rgb(105, 112, 136)),
            Self::Dark => self.base().fg(Color::Rgb(157, 165, 190)),
        }
    }
    pub fn border(self, focused: bool) -> Style {
        if focused {
            self.accent()
        } else {
            match self {
                Self::Auto => self.base().add_modifier(Modifier::DIM),
                Self::Light => self.base().fg(Color::Rgb(184, 190, 208)),
                Self::Dark => self.base().fg(Color::Rgb(66, 73, 96)),
            }
        }
    }
    pub fn selected(self) -> Style {
        match self {
            Self::Auto => self.base().fg(Color::White).bg(Color::Blue),
            Self::Light => self
                .base()
                .fg(Color::Rgb(40, 65, 135))
                .bg(Color::Rgb(216, 227, 252)),
            Self::Dark => self
                .base()
                .fg(Color::Rgb(221, 231, 255))
                .bg(Color::Rgb(52, 67, 105)),
        }
    }
    pub fn error(self) -> Style {
        self.base().fg(match self {
            Self::Auto => Color::Red,
            Self::Light => Color::Rgb(180, 35, 24),
            Self::Dark => Color::Rgb(244, 135, 113),
        })
    }
    pub fn link(self) -> Style {
        self.accent().add_modifier(Modifier::UNDERLINED)
    }
}
