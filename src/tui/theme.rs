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
                .fg(Color::Rgb(36, 41, 47))
                .bg(Color::Rgb(255, 255, 255)),
            Self::Dark => Style::new()
                .fg(Color::Rgb(212, 212, 212))
                .bg(Color::Rgb(30, 30, 30)),
        }
    }
    pub fn accent(self) -> Style {
        self.base().fg(match self {
            Self::Auto => Color::Blue,
            Self::Light => Color::Rgb(9, 105, 218),
            Self::Dark => Color::Rgb(86, 156, 214),
        })
    }
    pub fn muted(self) -> Style {
        match self {
            // The host knows its actual contrast; do not fade text on light terminals.
            Self::Auto => self.base(),
            Self::Light => self.base().fg(Color::Rgb(87, 96, 106)),
            Self::Dark => self.base().fg(Color::Rgb(157, 165, 180)),
        }
    }
    pub fn border(self, focused: bool) -> Style {
        if focused {
            self.accent()
        } else {
            self.base()
        }
    }
    pub fn selected(self) -> Style {
        self.base()
            .add_modifier(Modifier::REVERSED | Modifier::BOLD)
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
