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
                .fg(Color::Rgb(48, 48, 48))
                .bg(Color::Rgb(250, 250, 250)),
            Self::Dark => Style::new()
                .fg(Color::Rgb(216, 216, 216))
                .bg(Color::Rgb(30, 30, 30)),
        }
    }
    pub fn accent(self) -> Style {
        self.base().fg(match self {
            Self::Auto => Color::Blue,
            Self::Light => Color::Rgb(49, 95, 155),
            Self::Dark => Color::Rgb(138, 180, 248),
        })
    }
    pub fn surface(self) -> Style {
        match self {
            Self::Auto => self.base(),
            Self::Light => self.base().bg(Color::Rgb(240, 240, 240)),
            Self::Dark => self.base().bg(Color::Rgb(40, 40, 40)),
        }
    }
    pub fn muted(self) -> Style {
        match self {
            Self::Auto => self.base().add_modifier(Modifier::DIM),
            Self::Light => self.base().fg(Color::Rgb(104, 104, 104)),
            Self::Dark => self.base().fg(Color::Rgb(160, 160, 160)),
        }
    }
    pub fn border(self, focused: bool) -> Style {
        if focused {
            self.accent()
        } else {
            match self {
                Self::Auto => self.base().add_modifier(Modifier::DIM),
                Self::Light => self.base().fg(Color::Rgb(182, 182, 182)),
                Self::Dark => self.base().fg(Color::Rgb(85, 85, 85)),
            }
        }
    }
    pub fn selected(self) -> Style {
        match self {
            Self::Auto => self.base().add_modifier(Modifier::REVERSED),
            Self::Light => self
                .base()
                .fg(Color::Rgb(40, 65, 135))
                .bg(Color::Rgb(224, 233, 245)),
            Self::Dark => self
                .base()
                .fg(Color::Rgb(230, 237, 245))
                .bg(Color::Rgb(45, 69, 99)),
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
