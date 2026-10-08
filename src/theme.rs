//! Color schemes of the interface.
//!
//! A scheme comes from the terminal itself, or from a theme file in the format of
//! Ghostty: a set of them is built in, the rest is read from where Ghostty keeps its
//! own. Such a file only gives a palette; which color plays which part is decided
//! here, so that every scheme stays readable.
use crate::config;
use ratatui::style::{Color, Modifier, Style};
use std::path::PathBuf;

/// The colors of the terminal the interface runs in.
pub const TERMINAL: &str = "terminal";
/// No colors at all: bold, dim and reverse video.
pub const MONO: &str = "mono";

// Copies of theme files that ship with Ghostty; see themes/README.md.
const BUILT_IN: [(&str, &str); 45] = [
    ("Atom One Dark", include_str!("../themes/Atom One Dark")),
    ("Atom One Light", include_str!("../themes/Atom One Light")),
    ("Ayu", include_str!("../themes/Ayu")),
    ("Ayu Light", include_str!("../themes/Ayu Light")),
    ("Ayu Mirage", include_str!("../themes/Ayu Mirage")),
    ("Carbonfox", include_str!("../themes/Carbonfox")),
    (
        "Catppuccin Frappe",
        include_str!("../themes/Catppuccin Frappe"),
    ),
    (
        "Catppuccin Latte",
        include_str!("../themes/Catppuccin Latte"),
    ),
    (
        "Catppuccin Macchiato",
        include_str!("../themes/Catppuccin Macchiato"),
    ),
    (
        "Catppuccin Mocha",
        include_str!("../themes/Catppuccin Mocha"),
    ),
    ("Dracula", include_str!("../themes/Dracula")),
    (
        "Everforest Dark Hard",
        include_str!("../themes/Everforest Dark Hard"),
    ),
    (
        "Everforest Light Med",
        include_str!("../themes/Everforest Light Med"),
    ),
    ("Flexoki Dark", include_str!("../themes/Flexoki Dark")),
    ("Flexoki Light", include_str!("../themes/Flexoki Light")),
    (
        "Ghostty Default Style Dark",
        include_str!("../themes/Ghostty Default Style Dark"),
    ),
    ("GitHub Dark", include_str!("../themes/GitHub Dark")),
    (
        "GitHub Light Default",
        include_str!("../themes/GitHub Light Default"),
    ),
    ("Gruvbox Dark", include_str!("../themes/Gruvbox Dark")),
    ("Gruvbox Light", include_str!("../themes/Gruvbox Light")),
    (
        "Gruvbox Material Dark",
        include_str!("../themes/Gruvbox Material Dark"),
    ),
    (
        "iTerm2 Solarized Dark",
        include_str!("../themes/iTerm2 Solarized Dark"),
    ),
    (
        "iTerm2 Solarized Light",
        include_str!("../themes/iTerm2 Solarized Light"),
    ),
    ("Kanagawa Dragon", include_str!("../themes/Kanagawa Dragon")),
    ("Kanagawa Lotus", include_str!("../themes/Kanagawa Lotus")),
    ("Kanagawa Wave", include_str!("../themes/Kanagawa Wave")),
    ("Material Ocean", include_str!("../themes/Material Ocean")),
    ("Monokai Classic", include_str!("../themes/Monokai Classic")),
    ("Monokai Pro", include_str!("../themes/Monokai Pro")),
    ("Night Owl", include_str!("../themes/Night Owl")),
    ("Nightfox", include_str!("../themes/Nightfox")),
    ("Nord", include_str!("../themes/Nord")),
    ("Nord Light", include_str!("../themes/Nord Light")),
    ("Oxocarbon", include_str!("../themes/Oxocarbon")),
    ("Poimandres", include_str!("../themes/Poimandres")),
    ("Rose Pine", include_str!("../themes/Rose Pine")),
    ("Rose Pine Dawn", include_str!("../themes/Rose Pine Dawn")),
    ("Rose Pine Moon", include_str!("../themes/Rose Pine Moon")),
    ("Synthwave", include_str!("../themes/Synthwave")),
    ("TokyoNight", include_str!("../themes/TokyoNight")),
    ("TokyoNight Day", include_str!("../themes/TokyoNight Day")),
    (
        "TokyoNight Storm",
        include_str!("../themes/TokyoNight Storm"),
    ),
    ("Tomorrow Night", include_str!("../themes/Tomorrow Night")),
    ("Vesper", include_str!("../themes/Vesper")),
    ("Zenburn", include_str!("../themes/Zenburn")),
];

#[derive(Clone, Copy, PartialEq, Debug)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn parse(value: &str) -> Option<Self> {
        let hex = value.trim().trim_matches('"').trim_start_matches('#');
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        let part = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        Some(Self(part(0)?, part(2)?, part(4)?))
    }

    // Relative luminance as WCAG defines it.
    fn luminance(self) -> f64 {
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.0) + 0.7152 * linear(self.1) + 0.0722 * linear(self.2)
    }

    // How well the two colors tell apart, from 1 (not at all) to 21.
    fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    // This color with the given share of `other` mixed in.
    fn mix(self, other: Self, share: f64) -> Self {
        let blend =
            |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * share).round() as u8;
        Self(
            blend(self.0, other.0),
            blend(self.1, other.1),
            blend(self.2, other.2),
        )
    }

    fn color(self) -> Color {
        Color::Rgb(self.0, self.1, self.2)
    }

    // The closest of the 240 colors that every 256-color terminal shows alike: a
    // cube of six levels per channel and a ramp of grays. The first sixteen are
    // left out, as they are whatever the scheme of the terminal makes them.
    fn indexed(self) -> u8 {
        let near = |value: u8| {
            (0..6u8)
                .min_by_key(|level| LEVELS[usize::from(*level)].abs_diff(value))
                .unwrap_or(0)
        };
        let (red, green, blue) = (near(self.0), near(self.1), near(self.2));
        let cube = 16 + 36 * red + 6 * green + blue;
        let mean = (u16::from(self.0) + u16::from(self.1) + u16::from(self.2)) / 3;
        let gray = 232 + ((mean.saturating_sub(3)) / 10).min(23) as u8;
        let distance = |index: u8| {
            let other = Self::of(index);
            [(self.0, other.0), (self.1, other.1), (self.2, other.2)]
                .iter()
                .map(|(a, b)| u32::from(a.abs_diff(*b)).pow(2))
                .sum::<u32>()
        };
        if distance(gray) < distance(cube) {
            gray
        } else {
            cube
        }
    }

    // The color behind an index of the cube or the ramp.
    fn of(index: u8) -> Self {
        match index {
            232.. => {
                let gray = 8 + 10 * (index - 232);
                Self(gray, gray, gray)
            }
            _ => {
                let at = index.saturating_sub(16);
                let level = |step: u8| LEVELS[usize::from(step % 6)];
                Self(level(at / 36), level(at / 6), level(at))
            }
        }
    }
}

const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// What a Ghostty theme or configuration file says about colors.
#[derive(Clone, Default, PartialEq, Debug)]
struct Colors {
    palette: [Option<Rgb>; 16],
    background: Option<Rgb>,
    foreground: Option<Rgb>,
    selection_background: Option<Rgb>,
    selection_foreground: Option<Rgb>,
    // The `theme` key of a configuration file.
    theme: Option<String>,
}

impl Colors {
    // Later lines win, as later files do when one is read after another.
    fn read(&mut self, text: &str) {
        for line in text.lines() {
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match name.trim() {
                "palette" => {
                    if let Some((index, color)) = value.split_once('=')
                        && let Ok(index) = index.trim().parse::<usize>()
                        && index < 16
                        && let Some(color) = Rgb::parse(color)
                    {
                        self.palette[index] = Some(color);
                    }
                }
                "background" => self.background = Rgb::parse(value).or(self.background),
                "foreground" => self.foreground = Rgb::parse(value).or(self.foreground),
                "selection-background" => {
                    self.selection_background = Rgb::parse(value).or(self.selection_background)
                }
                "selection-foreground" => {
                    self.selection_foreground = Rgb::parse(value).or(self.selection_foreground)
                }
                "theme" => self.theme = Some(value.trim_matches('"').to_owned()),
                _ => (),
            }
        }
    }
}

/// The styles of the parts of the interface under one scheme.
#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    pub name: String,
    /// The whole screen; sets the background when the scheme has one of its own.
    pub base: Style,
    pub text: Style,
    /// Bold text: messages.
    pub strong: Style,
    /// Secondary text: hints, artists, empty lists.
    pub muted: Style,
    pub border: Style,
    /// Titles and whatever leads the eye.
    pub accent: Style,
    /// The filled part of the progress bar.
    pub bar: Style,
    /// The row under the cursor and the current view.
    pub selected: Style,
    /// The button that Enter would press.
    pub focused: Style,
    pub playing: Style,
    /// Loading, paused, being downloaded.
    pub waiting: Style,
    pub favorite: Style,
    pub stored: Style,
    pub error: Style,
}

impl Theme {
    pub fn mono() -> Self {
        Self::plain(MONO, |_| Style::new())
    }

    // The sixteen colors every terminal has; what they look like is up to its scheme.
    fn ansi() -> Self {
        Self::plain(TERMINAL, |color| Style::new().fg(color))
    }

    // Without knowing the actual colors, emphasis has to come from the terminal's own
    // means. Dim text may hold brighter parts, which therefore undo it.
    fn plain(name: &str, color: impl Fn(Color) -> Style) -> Self {
        let clear = Style::new().remove_modifier(Modifier::DIM);
        let dim = Style::new().add_modifier(Modifier::DIM);
        let bold = clear.add_modifier(Modifier::BOLD);
        // A colored mark in a reversed row would turn into a colored block.
        let reversed = clear.add_modifier(Modifier::REVERSED).fg(Color::Reset);
        Self {
            name: name.into(),
            base: Style::new(),
            text: Style::new(),
            strong: bold,
            muted: dim,
            border: dim,
            accent: bold.patch(color(Color::Blue)),
            bar: clear.patch(color(Color::Blue)),
            selected: reversed,
            focused: reversed
                .add_modifier(Modifier::BOLD)
                .patch(color(Color::Blue)),
            playing: bold.patch(color(Color::Green)),
            waiting: clear.patch(color(Color::Yellow)),
            favorite: clear.patch(color(Color::Yellow)),
            stored: clear.patch(color(Color::Cyan)),
            error: clear.patch(color(Color::Red)),
        }
    }

    // `paint` is off when the colors are those the terminal already shows: its own
    // background, possibly translucent, is then left alone.
    fn colored(name: &str, colors: &Colors, paint: bool) -> Option<Self> {
        let (background, foreground) = (colors.background?, colors.foreground?);
        // A palette color keeps its hue and moves toward the text color only as far
        // as it takes to read on the background.
        let tone = |index: usize| {
            let color = colors.palette[index].unwrap_or(foreground);
            (0..=10)
                .map(|step| color.mix(foreground, f64::from(step) / 10.0))
                .find(|color| color.contrast(background) >= 3.0)
                .unwrap_or(foreground)
        };
        // As close to the background as still tells apart from it that much.
        let faint = |least: f64| {
            (1..10)
                .map(|step| background.mix(foreground, f64::from(step) / 10.0))
                .find(|color| color.contrast(background) >= least)
                .unwrap_or(foreground)
        };
        let accent = tone(4);
        let mut selection = colors
            .selection_background
            .filter(|color| color.contrast(background) >= 1.2)
            .unwrap_or_else(|| background.mix(accent, 0.3));
        let mut on_selection = colors
            .selection_foreground
            .filter(|color| color.contrast(selection) >= 4.0)
            .unwrap_or(
                if foreground.contrast(selection) >= background.contrast(selection) {
                    foreground
                } else {
                    background
                },
            );
        if on_selection.contrast(selection) < 4.0 {
            // A selection of a middling tone carries no text well. Bring it closer to
            // the background until the usual text reads on it; if it then fades into
            // the background, swap the two colors as reverse video does.
            (selection, on_selection) = (1..10)
                .map(|step| selection.mix(background, f64::from(step) / 10.0))
                .find(|color| {
                    foreground.contrast(*color) >= 4.0 && color.contrast(background) >= 1.2
                })
                .map_or((foreground, background), |color| (color, foreground));
        }
        let fg = |color: Rgb| Style::new().fg(color.color());
        let text = if paint { fg(foreground) } else { Style::new() };
        Some(Self {
            name: name.into(),
            base: if paint {
                text.bg(background.color())
            } else {
                Style::new()
            },
            text,
            strong: text.add_modifier(Modifier::BOLD),
            muted: fg(faint(3.0)),
            border: fg(faint(1.7)),
            accent: fg(accent).add_modifier(Modifier::BOLD),
            bar: fg(accent),
            selected: fg(on_selection).bg(selection.color()),
            focused: fg(background)
                .bg(accent.color())
                .add_modifier(Modifier::BOLD),
            playing: fg(tone(2)).add_modifier(Modifier::BOLD),
            waiting: fg(tone(3)),
            favorite: fg(tone(3)),
            stored: fg(tone(6)),
            error: fg(tone(1)),
        })
    }

    /// The scheme called `name`, as the terminal at hand can show it; an unknown one
    /// gives the scheme of the terminal.
    pub fn load(name: &str) -> Self {
        Self::with(name, truecolor(|name| std::env::var(name).ok()))
    }

    /// `truecolor` tells whether colors may be given as red, green and blue.
    pub fn with(name: &str, truecolor: bool) -> Self {
        match name {
            MONO => Self::mono(),
            TERMINAL => terminal(),
            _ => find(name)
                .and_then(|text| {
                    let mut colors = Colors::default();
                    colors.read(&text);
                    Self::colored(name, &colors, true)
                })
                .map(|theme| if truecolor { theme } else { theme.reduced() })
                .unwrap_or_else(terminal),
        }
    }

    /// What a picture may be drawn with under this scheme: whether cells take colors
    /// at all, the two colors that its tones run between, the darker first, and
    /// whether marks are lighter than what is behind them.
    pub fn palette(&self) -> crate::cover::Palette {
        let rgb = |color: Option<Color>| match color {
            Some(Color::Rgb(red, green, blue)) => Some(Rgb(red, green, blue)),
            Some(Color::Indexed(index)) => Some(Rgb::of(index)),
            _ => None,
        };
        // A scheme that leaves the colors to the terminal names neither: its tones
        // are grays, and the terminal is taken for a dark one.
        let (behind, marks) = match (rgb(self.base.bg), rgb(self.base.fg)) {
            (Some(behind), Some(marks)) => (behind, marks),
            _ => (Rgb(0, 0, 0), Rgb(255, 255, 255)),
        };
        let light_marks = marks.luminance() >= behind.luminance();
        let (dark, light) = if light_marks {
            (behind, marks)
        } else {
            (marks, behind)
        };
        // The colors of the scheme: what it gives the roles of the interface, once each.
        let mut scheme: Vec<[u8; 3]> = Vec::new();
        let named = (rgb(self.base.bg).is_some())
            .then_some([
                self.base.bg,
                self.base.fg,
                self.muted.fg,
                self.border.fg,
                self.accent.fg,
                self.bar.fg,
                self.playing.fg,
                self.waiting.fg,
                self.favorite.fg,
                self.stored.fg,
                self.error.fg,
            ])
            .into_iter()
            .flatten();
        for color in named.filter_map(rgb) {
            let color = [color.0, color.1, color.2];
            if !scheme.contains(&color) {
                scheme.push(color);
            }
        }
        crate::cover::Palette {
            scheme,
            colors: self.text.fg.is_some() || self.base.bg.is_some(),
            tones: ([dark.0, dark.1, dark.2], [light.0, light.1, light.2]),
            light_marks,
        }
    }

    /// The color of the terminal for one of a picture: itself where the scheme is in
    /// true color, the nearest of the 256 otherwise. A scheme of the terminal's own
    /// colors asks the terminal what it takes.
    pub fn paint(&self, [red, green, blue]: [u8; 3], truecolor: bool) -> Color {
        if truecolor {
            Color::Rgb(red, green, blue)
        } else {
            Color::Indexed(Rgb(red, green, blue).indexed())
        }
    }

    /// Whether colors may be given as red, green and blue under this scheme.
    pub fn truecolor(&self) -> bool {
        match self.base.bg {
            Some(Color::Rgb(..)) => true,
            Some(Color::Indexed(_)) => false,
            _ => truecolor(|name| std::env::var(name).ok()),
        }
    }

    // The same scheme in the 256 colors that terminals without true color have.
    fn reduced(mut self) -> Self {
        let index = |color: Option<Color>| match color {
            Some(Color::Rgb(red, green, blue)) => {
                Some(Color::Indexed(Rgb(red, green, blue).indexed()))
            }
            other => other,
        };
        let (text, background) = (index(self.base.fg), index(self.base.bg));
        for style in [
            &mut self.base,
            &mut self.text,
            &mut self.strong,
            &mut self.muted,
            &mut self.border,
            &mut self.accent,
            &mut self.bar,
            &mut self.selected,
            &mut self.focused,
            &mut self.playing,
            &mut self.waiting,
            &mut self.favorite,
            &mut self.stored,
            &mut self.error,
        ] {
            (style.fg, style.bg) = (index(style.fg), index(style.bg));
            // Close tones can land on one color, or too near to read: nothing may
            // vanish into the background, and a marked row or button stays marked.
            let apart = |a: Option<Color>, b: Option<Color>| match (a, b) {
                (Some(Color::Indexed(a)), Some(Color::Indexed(b))) => {
                    Rgb::of(a).contrast(Rgb::of(b))
                }
                _ => 21.0,
            };
            if style.bg.is_none() && style.fg == background {
                style.fg = text;
            } else if style.bg == background || apart(style.fg, style.bg) < 3.0 {
                (style.fg, style.bg) = (background, text);
            }
        }
        (self.base.fg, self.base.bg) = (text, background);
        self
    }
}

// Whether the terminal says that it takes colors as red, green and blue. One that does
// not may read such a sequence as something else, bold or blinking, so without a sign
// of it the colors are picked from the 256 that every color terminal has.
fn truecolor(variable: impl Fn(&str) -> Option<String>) -> bool {
    let among = |name: &str, values: &[&str]| {
        variable(name).is_some_and(|value| values.iter().any(|known| value.contains(known)))
    };
    among("COLORTERM", &["truecolor", "24bit"])
        // Windows Terminal, also around a Linux shell in WSL.
        || variable("WT_SESSION").is_some()
        || among("TERM_PROGRAM", &["ghostty", "iTerm", "WezTerm", "vscode"])
        // tmux turns such colors into what the terminal around it can show.
        || among(
            "TERM",
            &["direct", "ghostty", "kitty", "alacritty", "foot", "wezterm", "tmux"],
        )
}

// Ghostty tells its theme in a file, so the exact colors are known and the fainter
// tones can be mixed from them. Elsewhere the terminal's sixteen colors are used.
fn terminal() -> Theme {
    // The convention of no-color.org: the user wants programs without color.
    if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return Theme {
            name: TERMINAL.into(),
            ..Theme::mono()
        };
    }
    let inside = std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "ghostty")
        || std::env::var("TERM").is_ok_and(|term| term == "xterm-ghostty");
    inside
        .then(|| {
            let directory = config::directory("XDG_CONFIG_HOME", ".config")?.join("ghostty");
            let text = ["config", "config.ghostty"]
                .iter()
                .find_map(|name| std::fs::read_to_string(directory.join(name)).ok())?;
            Theme::colored(TERMINAL, &configured(&text, find)?, false)
        })
        .flatten()
        .unwrap_or_else(Theme::ansi)
}

// The colors a Ghostty configuration results in: its theme, then its own color lines.
// One that switches between a light and a dark theme does not say which is shown.
fn configured(text: &str, find: impl Fn(&str) -> Option<String>) -> Option<Colors> {
    let mut own = Colors::default();
    own.read(text);
    let mut colors = Colors::default();
    if let Some(theme) = &own.theme {
        if theme.contains(':') {
            return None;
        }
        colors.read(&find(theme)?);
    }
    colors.read(text);
    Some(colors)
}

// Where Ghostty looks for themes: the user's own first.
fn directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    directories.extend(
        config::directory("XDG_CONFIG_HOME", ".config").map(|path| path.join("ghostty/themes")),
    );
    directories.extend(
        std::env::var_os("GHOSTTY_RESOURCES_DIR").map(|path| PathBuf::from(path).join("themes")),
    );
    directories.extend(
        [
            "/usr/share/ghostty/themes",
            "/usr/local/share/ghostty/themes",
            "/opt/homebrew/share/ghostty/themes",
            "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
        ]
        .map(PathBuf::from),
    );
    directories
}

// The text of the theme file called `name`: the user's own, a built-in one, or one
// installed with Ghostty.
fn find(name: &str) -> Option<String> {
    // A name, not a path: nothing outside the theme directories is read.
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
        return None;
    }
    let directories = directories();
    let read = |directory: &PathBuf| std::fs::read_to_string(directory.join(name)).ok();
    directories
        .first()
        .and_then(read)
        .or_else(|| {
            BUILT_IN
                .iter()
                .find(|(built_in, _)| *built_in == name)
                .map(|(_, text)| (*text).to_owned())
        })
        .or_else(|| directories.iter().skip(1).find_map(read))
}

/// Every scheme there is to choose from: the two of the terminal, then the themes.
pub fn names() -> Vec<String> {
    let mut themes: Vec<String> = BUILT_IN
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    for directory in directories() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        themes.extend(
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| !kind.is_dir()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| !name.starts_with('.')),
        );
    }
    themes.sort_by_cached_key(|name| name.to_lowercase());
    themes.dedup();
    let mut names = vec![TERMINAL.to_owned(), MONO.to_owned()];
    names.extend(themes);
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(style: Style) -> (Rgb, Option<Rgb>) {
        let part = |color| match color {
            Some(Color::Rgb(r, g, b)) => Some(Rgb(r, g, b)),
            _ => None,
        };
        (part(style.fg).expect("a color of its own"), part(style.bg))
    }

    #[test]
    fn theme_files_are_read_like_ghostty_reads_them() {
        let mut colors = Colors::default();
        colors.read(
            "# comment\npalette = 0=#282828\npalette=4 = 78a9ff\npalette = 16=#ffffff\n\
             palette = x=#ffffff\nbackground = #161616\nforeground = \"#F2F4F8\"\n\
             selection-background = nonsense\ncursor-color = #ffffff\ntheme = \"Rose Pine\"\n",
        );
        assert_eq!(colors.palette[0], Some(Rgb(0x28, 0x28, 0x28)));
        assert_eq!(colors.palette[4], Some(Rgb(0x78, 0xa9, 0xff)));
        assert_eq!(colors.palette.iter().flatten().count(), 2);
        assert_eq!(colors.background, Some(Rgb(0x16, 0x16, 0x16)));
        assert_eq!(colors.foreground, Some(Rgb(0xf2, 0xf4, 0xf8)));
        assert_eq!(colors.selection_background, None);
        assert_eq!(colors.theme.as_deref(), Some("Rose Pine"));
        assert!(Theme::colored("partial", &Colors::default(), true).is_none());
    }

    #[test]
    fn every_built_in_theme_is_readable() {
        assert!((Rgb(0, 0, 0).contrast(Rgb(255, 255, 255)) - 21.0).abs() < 0.01);
        let mut faults = Vec::new();
        for (name, text) in BUILT_IN {
            let mut colors = Colors::default();
            colors.read(text);
            assert_eq!(colors.palette.iter().flatten().count(), 16, "{name}");
            let theme = Theme::colored(name, &colors, true).expect(name);
            let (foreground, background) = rgb(theme.base);
            let background = background.unwrap();
            let text = foreground.contrast(background);
            let on = |style: Style| rgb(style).0.contrast(background);
            let (on_selection, selection) = rgb(theme.selected);
            let (on_focus, focus) = rgb(theme.focused);
            for (part, contrast, least) in [
                ("text", text, 4.0),
                ("muted", on(theme.muted), 3.0),
                ("border", on(theme.border), 1.7),
                ("accent", on(theme.accent), 3.0),
                ("playing", on(theme.playing), 3.0),
                ("waiting", on(theme.waiting), 3.0),
                ("stored", on(theme.stored), 3.0),
                ("error", on(theme.error), 3.0),
                ("selection", selection.unwrap().contrast(background), 1.15),
                (
                    "selected text",
                    on_selection.contrast(selection.unwrap()),
                    4.0,
                ),
                ("focus", on_focus.contrast(focus.unwrap()), 3.0),
                // Secondary text is fainter than the text, borders fainter still.
                ("muted below text", text / on(theme.muted), 1.0),
                (
                    "border below muted",
                    on(theme.muted) / on(theme.border),
                    1.0,
                ),
            ] {
                if contrast < least {
                    faults.push(format!("{name}: {part} {contrast:.2} < {least}"));
                }
            }
        }
        assert!(faults.is_empty(), "{}", faults.join("\n"));
    }

    #[test]
    fn schemes_survive_a_terminal_of_256_colors() {
        // The corners of the cube and of the ramp are themselves.
        for (color, index) in [
            (Rgb(0, 0, 0), 16),
            (Rgb(255, 255, 255), 231),
            (Rgb(255, 0, 0), 196),
            (Rgb(95, 135, 175), 67),
            (Rgb(8, 8, 8), 232),
            (Rgb(238, 238, 238), 255),
            (Rgb(128, 128, 128), 244),
        ] {
            assert_eq!(color.indexed(), index, "{color:?}");
            assert_eq!(Rgb::of(index), color);
        }
        let color = |color: Option<Color>| match color {
            Some(Color::Indexed(index)) => Rgb::of(index),
            other => panic!("{other:?}"),
        };
        for (name, _) in BUILT_IN {
            let theme = Theme::with(name, false);
            assert_eq!(theme.name, name);
            let (text, background) = (color(theme.base.fg), color(theme.base.bg));
            assert!(text.contrast(background) >= 3.5, "{name}: text");
            for (part, style) in [
                ("muted", theme.muted),
                ("border", theme.border),
                ("accent", theme.accent),
                ("playing", theme.playing),
                ("favorite", theme.favorite),
                ("stored", theme.stored),
                ("error", theme.error),
            ] {
                assert_eq!(style.bg, None, "{name}: {part}");
                assert_ne!(color(style.fg), background, "{name}: {part}");
            }
            for (part, style) in [("selected", theme.selected), ("focused", theme.focused)] {
                assert_ne!(color(style.bg), background, "{name}: {part}");
                assert!(
                    color(style.fg).contrast(color(style.bg)) >= 3.0,
                    "{name}: {part}"
                );
            }
        }
        assert_eq!(
            Theme::with("Dracula", true).base.bg,
            Some(Color::Rgb(0x28, 0x2a, 0x36))
        );
    }

    #[test]
    fn true_color_is_used_only_where_the_terminal_announces_it() {
        let environment = |pairs: &'static [(&str, &str)]| {
            move |name: &str| {
                (pairs.iter())
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        for (pairs, expected) in [
            (&[("COLORTERM", "truecolor")][..], true),
            (
                &[("COLORTERM", "24bit"), ("TERM", "xterm-256color")][..],
                true,
            ),
            (&[("TERM", "xterm-ghostty")][..], true),
            (&[("TERM", "xterm-kitty")][..], true),
            (&[("TERM", "tmux-256color")][..], true),
            (&[("TERM", "xterm-direct")][..], true),
            (
                &[("WT_SESSION", "0f8c"), ("TERM", "xterm-256color")][..],
                true,
            ),
            (&[("TERM_PROGRAM", "iTerm.app")][..], true),
            (
                &[
                    ("TERM_PROGRAM", "Apple_Terminal"),
                    ("TERM", "xterm-256color"),
                ][..],
                false,
            ),
            (&[("TERM", "screen-256color")][..], false),
            (&[("TERM", "linux")][..], false),
            (&[("COLORTERM", "")][..], false),
            (&[][..], false),
        ] {
            assert_eq!(truecolor(environment(pairs)), expected, "{pairs:?}");
        }
    }

    #[test]
    fn terminal_schemes_leave_the_colors_to_the_terminal() {
        let mono = Theme::mono();
        for style in [mono.base, mono.accent, mono.playing, mono.error] {
            assert_eq!((style.fg, style.bg), (None, None));
        }
        assert_eq!(
            (mono.selected.fg, mono.selected.bg),
            (Some(Color::Reset), None)
        );
        let ansi = Theme::ansi();
        assert_eq!((ansi.base.fg, ansi.base.bg), (None, None));
        assert_eq!(ansi.accent.fg, Some(Color::Blue));
        assert_eq!(ansi.selected.bg, None);

        // The theme named in Ghostty's configuration, with its own lines on top.
        let find = |name: &str| {
            (name == "Carbonfox").then(|| "background = #161616\nforeground = #f2f4f8\n".into())
        };
        let colors = configured("theme = Carbonfox\nbackground = #000000\n", find).unwrap();
        assert_eq!(colors.background, Some(Rgb(0, 0, 0)));
        assert_eq!(colors.foreground, Some(Rgb(0xf2, 0xf4, 0xf8)));
        let theme = Theme::colored(TERMINAL, &colors, false).unwrap();
        // Known colors serve the fainter tones, but the terminal keeps its background.
        assert_eq!(
            (theme.base.fg, theme.base.bg, theme.text.fg),
            (None, None, None)
        );
        assert!(matches!(theme.muted.fg, Some(Color::Rgb(..))));
        assert!(configured("theme = light:Dayfox,dark:Carbonfox\n", find).is_none());
        assert!(configured("theme = Missing\n", find).is_none());
        assert!(
            configured("font-size = 12\n", find)
                .unwrap()
                .background
                .is_none()
        );
    }

    #[test]
    fn schemes_are_found_by_name_only() {
        let names = names();
        assert_eq!(&names[..2], [TERMINAL, MONO]);
        assert!(names.len() >= 2 + BUILT_IN.len());
        assert!(names.iter().any(|name| name == "Catppuccin Mocha"));
        assert!(find("Dracula").is_some());
        for name in ["", ".", "..", "../Dracula", "/etc/passwd", "a\\b"] {
            assert!(find(name).is_none(), "{name}");
        }
        assert_eq!(Theme::load("Dracula").name, "Dracula");
        assert_eq!(Theme::load("no such theme").name, TERMINAL);
        assert_eq!(Theme::load(MONO), Theme::mono());
    }
}
