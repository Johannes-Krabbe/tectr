//! The `[appearance]` section: how the app looks, and the themes it can look like.
//!
//! `FIELDS` is the only place defaults live. The UI draws its rows from what's resolved
//! here, and `apps/dsk/src/appearance.ts` turns the values into CSS variables. Themes are
//! kept in `themes.toml`, in the same format as a theme written in the settings file.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table};

const SUBTABLES: &[&str] = &["text", "code", "layout", "transcript"];
const THEMES_KEY: &str = "themes";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Dark,
    Light,
}

impl Scheme {
    const fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// A setting's value, in the form the file and the UI both have it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Flag(bool),
    Number(f64),
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
}

/// What a setting can be, which also says how the UI lets you change it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    Choice {
        options: &'static [Choice],
    },
    /// A theme for this scheme.
    Theme {
        scheme: Scheme,
    },
    /// A font family. Empty means the font that comes with the app.
    Font {
        bundled: &'static str,
        /// What choosing the system's own font writes.
        system: &'static str,
        monospace: bool,
    },
    Number {
        min: f64,
        max: f64,
        step: f64,
        unit: &'static str,
    },
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DefaultValue {
    Flag(bool),
    Number(f64),
    Text(&'static str),
}

impl DefaultValue {
    fn value(self) -> Value {
        match self {
            Self::Flag(flag) => Value::Flag(flag),
            Self::Number(number) => Value::Number(number),
            Self::Text(text) => Value::Text(text.to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Field {
    /// Its place in the file: `size` under `text` is `text.size`.
    id: &'static str,
    group: &'static str,
    label: &'static str,
    about: &'static str,
    default: DefaultValue,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FieldGroup {
    pub id: &'static str,
    pub title: &'static str,
}

pub const GROUPS: &[FieldGroup] = &[
    FieldGroup {
        id: "theme",
        title: "Theme",
    },
    FieldGroup {
        id: "text",
        title: "Text",
    },
    FieldGroup {
        id: "code",
        title: "Code",
    },
    FieldGroup {
        id: "layout",
        title: "Layout",
    },
    FieldGroup {
        id: "transcript",
        title: "Transcript",
    },
];

const DEFAULT_DARK_THEME: &str = "tectr-dark";
const DEFAULT_LIGHT_THEME: &str = "tectr-light";

const MODES: &[Choice] = &[
    Choice {
        value: "system",
        label: "Follow the system",
    },
    Choice {
        value: "light",
        label: "Light",
    },
    Choice {
        value: "dark",
        label: "Dark",
    },
];

const SMOOTHING: &[Choice] = &[
    Choice {
        value: "antialiased",
        label: "Thin",
    },
    Choice {
        value: "auto",
        label: "As the system draws it",
    },
];

const STEPS: &[Choice] = &[
    Choice {
        value: "folded",
        label: "Folded into one line",
    },
    Choice {
        value: "shown",
        label: "All shown",
    },
];

const FIELDS: &[Field] = &[
    Field {
        id: "mode",
        group: "theme",
        label: "Mode",
        about: "Whether to use the dark theme or the light theme",
        default: DefaultValue::Text("system"),
        kind: Kind::Choice { options: MODES },
    },
    Field {
        id: "dark_theme",
        group: "theme",
        label: "Dark theme",
        about: "Used in dark mode",
        default: DefaultValue::Text(DEFAULT_DARK_THEME),
        kind: Kind::Theme {
            scheme: Scheme::Dark,
        },
    },
    Field {
        id: "light_theme",
        group: "theme",
        label: "Light theme",
        about: "Used in light mode",
        default: DefaultValue::Text(DEFAULT_LIGHT_THEME),
        kind: Kind::Theme {
            scheme: Scheme::Light,
        },
    },
    Field {
        id: "text.family",
        group: "text",
        label: "Font",
        about: "For everything that isn't code",
        default: DefaultValue::Text(""),
        kind: Kind::Font {
            bundled: "Inter",
            system: "system-ui",
            monospace: false,
        },
    },
    Field {
        id: "text.size",
        group: "text",
        label: "Size",
        about: "Other text, rows and icons grow in proportion",
        default: DefaultValue::Number(13.0),
        kind: Kind::Number {
            min: 9.0,
            max: 28.0,
            step: 0.5,
            unit: "px",
        },
    },
    Field {
        id: "text.line_height",
        group: "text",
        label: "Line height in messages",
        about: "How much room agents' messages have between lines",
        default: DefaultValue::Number(1.55),
        kind: Kind::Number {
            min: 1.0,
            max: 2.5,
            step: 0.05,
            unit: "",
        },
    },
    Field {
        id: "text.smoothing",
        group: "text",
        label: "Font smoothing",
        about: "How heavy letters look on macOS",
        default: DefaultValue::Text("antialiased"),
        kind: Kind::Choice { options: SMOOTHING },
    },
    Field {
        id: "code.family",
        group: "code",
        label: "Font",
        about: "For code, commands and paths",
        default: DefaultValue::Text(""),
        kind: Kind::Font {
            bundled: "Geist Mono",
            system: "ui-monospace",
            monospace: true,
        },
    },
    Field {
        id: "code.size",
        group: "code",
        label: "Size",
        about: "Code in messages follows this too",
        default: DefaultValue::Number(12.5),
        kind: Kind::Number {
            min: 8.0,
            max: 28.0,
            step: 0.5,
            unit: "px",
        },
    },
    Field {
        id: "code.line_height",
        group: "code",
        label: "Line height",
        about: "How much room code has between lines",
        default: DefaultValue::Number(1.55),
        kind: Kind::Number {
            min: 1.0,
            max: 2.5,
            step: 0.05,
            unit: "",
        },
    },
    Field {
        id: "code.ligatures",
        group: "code",
        label: "Ligatures",
        about: "Joins characters like -> in fonts that have them",
        default: DefaultValue::Flag(true),
        kind: Kind::Toggle,
    },
    Field {
        id: "layout.density",
        group: "layout",
        label: "Density",
        about: "Scales row heights and the space between things",
        default: DefaultValue::Number(100.0),
        kind: Kind::Number {
            min: 70.0,
            max: 150.0,
            step: 5.0,
            unit: "%",
        },
    },
    Field {
        id: "layout.corners",
        group: "layout",
        label: "Corner roundness",
        about: "0% is square",
        default: DefaultValue::Number(100.0),
        kind: Kind::Number {
            min: 0.0,
            max: 300.0,
            step: 25.0,
            unit: "%",
        },
    },
    Field {
        id: "layout.sidebar_width",
        group: "layout",
        label: "Sidebar width",
        about: "",
        default: DefaultValue::Number(256.0),
        kind: Kind::Number {
            min: 160.0,
            max: 560.0,
            step: 8.0,
            unit: "px",
        },
    },
    Field {
        id: "layout.content_width",
        group: "layout",
        label: "Message width",
        about: "The widest messages and the message box get",
        default: DefaultValue::Number(800.0),
        kind: Kind::Number {
            min: 480.0,
            max: 2400.0,
            step: 40.0,
            unit: "px",
        },
    },
    Field {
        id: "layout.show_shortcuts",
        group: "layout",
        label: "Show keyboard shortcuts",
        about: "A line along the bottom with the keys that work right now",
        default: DefaultValue::Flag(false),
        kind: Kind::Toggle,
    },
    Field {
        id: "layout.remote_path_delay",
        group: "layout",
        label: "Remote path suggestions delay",
        about: "How long to wait after typing before asking another device for its folders. This device never waits",
        default: DefaultValue::Number(150.0),
        kind: Kind::Number {
            min: 0.0,
            max: 1000.0,
            step: 25.0,
            unit: "ms",
        },
    },
    Field {
        id: "transcript.steps",
        group: "transcript",
        label: "Tool calls and thinking",
        about: "What an agent does between its messages. Messages always show",
        default: DefaultValue::Text("folded"),
        kind: Kind::Choice { options: STEPS },
    },
    Field {
        id: "transcript.animate",
        group: "transcript",
        label: "Animations",
        about: "The sweep over what an agent is working on, messages typing out as they stream, and scrolling easing into place",
        default: DefaultValue::Flag(true),
        kind: Kind::Toggle,
    },
];

fn field(id: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|field| field.id == id)
}

/// A setting with what's in effect for it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedField {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    pub kind: Kind,
    pub value: Value,
    /// The file sets this, so it can be reset.
    pub customized: bool,
}

/// A theme with every color worked out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub scheme: Scheme,
    /// Written in the settings file, rather than coming with the app.
    pub custom: bool,
    /// By token name without `color.`, such as `surface.base`.
    pub colors: BTreeMap<String, String>,
}

/// What the UI needs to apply and show the appearance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Appearance {
    pub groups: &'static [FieldGroup],
    pub fields: Vec<ResolvedField>,
    pub themes: Vec<Theme>,
}

/// What the file says.
#[derive(Debug, Clone, Default)]
pub struct Values {
    overrides: HashMap<&'static str, Value>,
    custom: Vec<Theme>,
}

impl Values {
    /// The mode, or None to follow the system.
    pub fn forced_scheme(&self) -> Option<Scheme> {
        match self.text("mode") {
            "light" => Some(Scheme::Light),
            "dark" => Some(Scheme::Dark),
            _ => None,
        }
    }

    /// The color behind the window in this scheme, which the native title bar shows.
    pub fn window_color(&self, scheme: Scheme) -> [u8; 3] {
        let id = self.text(match scheme {
            Scheme::Dark => "dark_theme",
            Scheme::Light => "light_theme",
        });
        let theme = find_theme(&self.custom, id).unwrap_or_else(|| default_theme(scheme));
        base_color(theme)
    }

    /// A theme's scheme and the color behind the window with it, if there's such a theme.
    pub fn theme_window(&self, id: &str) -> Option<(Scheme, [u8; 3])> {
        let theme = find_theme(&self.custom, id)?;
        Some((theme.scheme, base_color(theme)))
    }

    fn text(&self, id: &str) -> &str {
        match self.overrides.get(id) {
            Some(Value::Text(text)) => text,
            _ => match field(id).map(|field| field.default) {
                Some(DefaultValue::Text(text)) => text,
                _ => "",
            },
        }
    }

    pub fn resolve(&self) -> Appearance {
        let fields = FIELDS
            .iter()
            .map(|field| ResolvedField {
                id: field.id,
                group: field.group,
                label: field.label,
                about: field.about,
                kind: field.kind,
                value: self
                    .overrides
                    .get(field.id)
                    .cloned()
                    .unwrap_or_else(|| field.default.value()),
                customized: self.overrides.contains_key(field.id),
            })
            .collect();
        let themes = builtin_themes()
            .iter()
            .map(|builtin| builtin.theme.clone())
            .chain(self.custom.iter().cloned())
            .collect();
        Appearance {
            groups: GROUPS,
            fields,
            themes,
        }
    }
}

// ----- Colors -----

/// `#rrggbb` or `#rrggbbaa`, as red, green, blue and alpha.
fn channels(hex: &str) -> Option<[u8; 4]> {
    let digits = hex.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    Some([
        channel(0)?,
        channel(2)?,
        channel(4)?,
        if digits.len() == 8 { channel(6)? } else { 0xff },
    ])
}

fn hex_of(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

fn rgb_of(hex: &str) -> [u8; 3] {
    let [r, g, b, _] = channels(hex).unwrap_or([0, 0, 0, 0xff]);
    [r, g, b]
}

/// The same color with this alpha.
fn with_alpha(hex: &str, alpha: u8) -> String {
    format!("{}{alpha:02x}", hex_of(rgb_of(hex)))
}

/// `share` of the way from `from` to `to`.
fn mix(from: &str, to: &str, share: f64) -> String {
    let (from, to) = (rgb_of(from), rgb_of(to));
    let channel = |index: usize| {
        let (a, b) = (f64::from(from[index]), f64::from(to[index]));
        // Between two u8s, so it fits.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mixed = (a + (b - a) * share).round() as u8;
        mixed
    };
    hex_of([channel(0), channel(1), channel(2)])
}

fn luminance(hex: &str) -> f64 {
    let [r, g, b] = rgb_of(hex).map(|channel| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast(a: &str, b: &str) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

// ----- Themes -----

/// Every color a theme ends up with. These are the names in `design/tokens`, without
/// `color.`, and a test checks that they're the same.
const TOKENS: &[&str] = &[
    "surface.base",
    "surface.panel",
    "surface.raised",
    "surface.overlay",
    "surface.control",
    "surface.control-hover",
    "line.subtle",
    "line.strong",
    "text.primary",
    "text.secondary",
    "text.tertiary",
    "text.on-accent",
    "accent.solid",
    "accent.text",
    "accent.tint",
    "accent.ring",
    "status.working",
    "status.waiting",
    "status.waiting-tint",
    "status.waiting-tint-strong",
    "status.waiting-border",
    "status.failed",
    "status.done",
    "status.idle",
    "status.stale",
    "status.online",
    "diff.add-bg",
    "diff.add-text",
    "diff.del-bg",
    "diff.del-text",
    "syntax.plain",
    "syntax.keyword",
    "syntax.string",
    "syntax.constant",
    "syntax.function",
    "syntax.type",
    "syntax.comment",
    "syntax.punctuation",
    "scrim",
    "shadow.popover",
    "shadow.sheet",
];

type Colors = BTreeMap<String, String>;
type Rule<'a> = &'a dyn Fn(&Colors) -> String;

/// Works out what a theme left out. Changing a theme's accent therefore changes its tint,
/// and only colors it set itself stay as they are.
fn complete(scheme: Scheme, mut colors: Colors) -> Colors {
    let dark = scheme == Scheme::Dark;
    let color =
        |colors: &Colors, token: &str| -> String { colors.get(token).cloned().unwrap_or_default() };
    let rules: &[(&str, Rule)] = &[
        ("surface.overlay", &|c| {
            color(
                c,
                if dark {
                    "surface.raised"
                } else {
                    "surface.panel"
                },
            )
        }),
        ("surface.control-hover", &|c| color(c, "line.strong")),
        ("text.on-accent", &|c| {
            // White if it's readable, as it is on most accents. Otherwise whichever reads best.
            let solid = color(c, "accent.solid");
            if contrast("#ffffff", &solid) >= 4.5 {
                return "#ffffff".to_owned();
            }
            let inky = color(c, if dark { "surface.base" } else { "text.primary" });
            if contrast(&inky, &solid) > contrast("#ffffff", &solid) {
                inky
            } else {
                "#ffffff".to_owned()
            }
        }),
        ("accent.tint", &|c| {
            with_alpha(&color(c, "accent.solid"), if dark { 0x29 } else { 0x1f })
        }),
        ("accent.ring", &|c| {
            with_alpha(&color(c, "accent.solid"), 0x80)
        }),
        ("status.working", &|c| color(c, "accent.text")),
        ("status.waiting-tint", &|c| {
            with_alpha(&color(c, "status.waiting"), if dark { 0x12 } else { 0x14 })
        }),
        ("status.waiting-tint-strong", &|c| {
            with_alpha(&color(c, "status.waiting"), 0x26)
        }),
        ("status.waiting-border", &|c| {
            with_alpha(&color(c, "status.waiting"), 0x47)
        }),
        ("status.done", &|c| color(c, "text.tertiary")),
        ("status.idle", &|c| color(c, "text.tertiary")),
        ("status.stale", &|c| color(c, "text.tertiary")),
        ("diff.add-bg", &|c| {
            with_alpha(&color(c, "status.online"), if dark { 0x21 } else { 0x1a })
        }),
        ("diff.del-bg", &|c| {
            with_alpha(&color(c, "status.failed"), if dark { 0x21 } else { 0x17 })
        }),
        ("diff.add-text", &|c| {
            mix(&color(c, "status.online"), &color(c, "text.primary"), 0.5)
        }),
        ("diff.del-text", &|c| {
            mix(&color(c, "status.failed"), &color(c, "text.primary"), 0.5)
        }),
        ("syntax.plain", &|c| color(c, "text.primary")),
        ("syntax.comment", &|c| color(c, "text.tertiary")),
        ("syntax.punctuation", &|c| color(c, "text.secondary")),
        ("scrim", &|c| {
            if dark {
                "#0000008c".to_owned()
            } else {
                with_alpha(&color(c, "text.primary"), 0x40)
            }
        }),
        ("shadow.popover", &|c| {
            if dark {
                "#00000080".to_owned()
            } else {
                with_alpha(&color(c, "text.primary"), 0x1f)
            }
        }),
        ("shadow.sheet", &|c| {
            if dark {
                "#00000099".to_owned()
            } else {
                with_alpha(&color(c, "text.primary"), 0x2e)
            }
        }),
    ];
    for (token, rule) in rules {
        if !colors.contains_key(*token) {
            let value = rule(&colors);
            colors.insert((*token).to_owned(), value);
        }
    }
    colors
}

/// A theme from `themes.toml`, with what it set itself so another theme can extend it.
#[derive(Debug)]
struct Builtin {
    set: BTreeMap<String, String>,
    theme: Theme,
}

/// Reads one theme's colors out of a table, along with `name` and `mode` where it has them.
struct Written {
    name: Option<String>,
    scheme: Option<Scheme>,
    extends: Option<String>,
    colors: BTreeMap<String, String>,
}

fn read_written(table: &dyn toml_edit::TableLike, id: &str, problems: &mut Vec<String>) -> Written {
    let mut written = Written {
        name: None,
        scheme: None,
        extends: None,
        colors: BTreeMap::new(),
    };
    let place = format!("appearance.themes.{id}");
    for (key, item) in table.iter() {
        let text = item.as_str();
        match (key, text) {
            ("name", Some(name)) => written.name = Some(name.to_owned()),
            ("extends", Some(base)) => written.extends = Some(base.to_owned()),
            ("mode", Some("dark")) => written.scheme = Some(Scheme::Dark),
            ("mode", Some("light")) => written.scheme = Some(Scheme::Light),
            ("mode", _) => problems.push(format!(
                "{place}.mode has to be \"dark\" or \"light\", so it's ignored"
            )),
            ("name" | "extends", None) => {
                problems.push(format!("{place}.{key} has to be text, so it's ignored"));
            }
            _ if !TOKENS.contains(&key) => problems.push(format!(
                "{place}.{key} isn't a color a theme has, so it's ignored"
            )),
            (_, Some(hex)) if channels(hex).is_some() => {
                written
                    .colors
                    .insert(key.to_owned(), hex.to_ascii_lowercase());
            }
            _ => problems.push(format!(
                "{place}.{key} has to be a color like \"#rrggbb\" or \"#rrggbbaa\", so it's ignored"
            )),
        }
    }
    written
}

fn builtin_themes() -> &'static [Builtin] {
    static THEMES: OnceLock<Vec<Builtin>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let document = include_str!("themes.toml")
            .parse::<DocumentMut>()
            .expect("themes.toml is checked by a test");
        document
            .iter()
            .filter_map(|(id, item)| {
                let mut problems = Vec::new();
                let written = read_written(item.as_table_like()?, id, &mut problems);
                debug_assert_eq!(problems, Vec::<String>::new(), "themes.toml");
                let scheme = written.scheme?;
                Some(Builtin {
                    theme: Theme {
                        id: id.to_owned(),
                        name: written.name?,
                        scheme,
                        custom: false,
                        colors: complete(scheme, written.colors.clone()),
                    },
                    set: written.colors,
                })
            })
            .collect()
    })
}

fn base_color(theme: &Theme) -> [u8; 3] {
    let [r, g, b, _] = theme
        .colors
        .get("surface.base")
        .and_then(|hex| channels(hex))
        .unwrap_or([0, 0, 0, 0xff]);
    [r, g, b]
}

fn default_theme(scheme: Scheme) -> &'static Theme {
    let id = match scheme {
        Scheme::Dark => DEFAULT_DARK_THEME,
        Scheme::Light => DEFAULT_LIGHT_THEME,
    };
    builtin_themes()
        .iter()
        .map(|builtin| &builtin.theme)
        .find(|theme| theme.id == id)
        .expect("themes.toml has the default themes")
}

fn find_theme<'a>(custom: &'a [Theme], id: &str) -> Option<&'a Theme> {
    custom.iter().find(|theme| theme.id == id).or_else(|| {
        builtin_themes()
            .iter()
            .map(|builtin| &builtin.theme)
            .find(|theme| theme.id == id)
    })
}

/// Reads `[appearance.themes]`. A theme with something wrong keeps what's right about it.
fn parse_themes(item: &Item, problems: &mut Vec<String>) -> Vec<Theme> {
    let Some(table) = item.as_table_like() else {
        problems.push(
            "appearance.themes has to be a section, like [appearance.themes.mine]".to_owned(),
        );
        return Vec::new();
    };
    let mut themes = Vec::new();
    for (id, item) in table.iter() {
        let place = format!("appearance.themes.{id}");
        let Some(table) = item.as_table_like() else {
            problems.push(format!("{place} has to be a section"));
            continue;
        };
        if builtin_themes().iter().any(|b| b.theme.id == id) {
            problems.push(format!(
                "{place} has the same name as a theme that comes with the app, so it's ignored. Give it another name"
            ));
            continue;
        }
        let written = read_written(table, id, problems);
        let base = written.extends.as_deref().and_then(|base| {
            let found = builtin_themes().iter().find(|b| b.theme.id == base);
            if found.is_none() {
                problems.push(format!(
                    "{place} extends \"{base}\", which isn't a theme that comes with the app"
                ));
            }
            found
        });
        // Without anything to extend, the colors it doesn't set are tectr's own.
        let scheme = base
            .map(|base| base.theme.scheme)
            .or(written.scheme)
            .unwrap_or_else(|| {
                problems.push(format!(
                    "{place} doesn't say mode = \"dark\" or \"light\", or what it extends, so it's dark"
                ));
                Scheme::Dark
            });
        if let (Some(base), Some(mode)) = (base, written.scheme)
            && base.theme.scheme != mode
        {
            problems.push(format!(
                "{place} says it's {} but extends a {} theme, so it's {}",
                mode.name(),
                base.theme.scheme.name(),
                base.theme.scheme.name(),
            ));
        }
        let mut colors = match base {
            Some(base) => base.set.clone(),
            None => builtin_themes()
                .iter()
                .find(|b| b.theme.id == default_theme(scheme).id)
                .map(|b| b.set.clone())
                .unwrap_or_default(),
        };
        colors.extend(written.colors);
        themes.push(Theme {
            id: id.to_owned(),
            name: written.name.unwrap_or_else(|| id.to_owned()),
            scheme,
            custom: true,
            colors: complete(scheme, colors),
        });
    }
    themes
}

// ----- Reading and writing the file -----

fn value_of_item(item: &Item) -> Option<Value> {
    let value = item.as_value()?;
    if let Some(flag) = value.as_bool() {
        return Some(Value::Flag(flag));
    }
    if let Some(number) = value.as_float() {
        return Some(Value::Number(number));
    }
    if let Some(number) = value.as_integer() {
        // Settings are small numbers, which a float holds exactly.
        #[allow(clippy::cast_precision_loss)]
        return Some(Value::Number(number as f64));
    }
    value.as_str().map(|text| Value::Text(text.to_owned()))
}

/// Why a family name can't be written as a CSS name, if it can't. Names only: the value
/// ends up in a CSS variable, and a list of fallbacks is already there behind it.
fn family_problem(family: &str) -> Option<&'static str> {
    if family.chars().count() > 120 {
        Some("is too long")
    } else if family.chars().any(|c| {
        c.is_control() || matches!(c, '"' | '\'' | '\\' | ';' | ',' | '{' | '}' | '(' | ')')
    }) {
        Some("has to be one font's name, without quotes or commas")
    } else {
        None
    }
}

/// Checks a value against what its setting can be, and puts it in the form that's kept.
fn checked(field: &Field, value: Value, custom: &[Theme]) -> Result<Value, String> {
    let id = field.id;
    match (field.kind, value) {
        (Kind::Choice { options }, Value::Text(text)) => {
            if options.iter().any(|option| option.value == text) {
                Ok(Value::Text(text))
            } else {
                let list: Vec<&str> = options.iter().map(|option| option.value).collect();
                Err(format!(
                    "appearance.{id} has to be one of {}",
                    list.join(", ")
                ))
            }
        }
        (Kind::Theme { scheme }, Value::Text(text)) => match find_theme(custom, &text) {
            Some(theme) if theme.scheme == scheme => Ok(Value::Text(text)),
            Some(_) => Err(format!(
                "appearance.{id} needs a {} theme, and \"{text}\" isn't one",
                scheme.name()
            )),
            None => Err(format!(
                "appearance.{id}: there's no theme called \"{text}\""
            )),
        },
        (Kind::Font { .. }, Value::Text(text)) => {
            let text = text.trim().to_owned();
            match family_problem(&text) {
                Some(problem) => Err(format!("appearance.{id} {problem}")),
                None => Ok(Value::Text(text)),
            }
        }
        (Kind::Number { min, max, .. }, Value::Number(number)) => {
            if number.is_finite() && (min..=max).contains(&number) {
                // So that steps of 0.05 don't leave 1.6000000000000001 in the file.
                Ok(Value::Number((number * 1000.0).round() / 1000.0))
            } else {
                Err(format!("appearance.{id} has to be from {min} to {max}"))
            }
        }
        (Kind::Toggle, Value::Flag(flag)) => Ok(Value::Flag(flag)),
        (kind, _) => Err(match kind {
            Kind::Toggle => format!("appearance.{id} has to be true or false"),
            Kind::Number { .. } => format!("appearance.{id} has to be a number"),
            _ => format!("appearance.{id} has to be text"),
        }),
    }
}

fn read_setting(id: &str, item: &Item, values: &mut Values, problems: &mut Vec<String>) {
    let Some(field) = field(id) else {
        problems.push(format!(
            "appearance.{id} isn't a known setting, so it's ignored"
        ));
        return;
    };
    let Some(value) = value_of_item(item) else {
        problems.push(format!("appearance.{id} isn't a value. Using the default."));
        return;
    };
    match checked(field, value, &values.custom) {
        Ok(value) => {
            values.overrides.insert(field.id, value);
        }
        Err(problem) => problems.push(format!("{problem}. Using the default.")),
    }
}

/// Reads `[appearance]`. A bad entry is reported and left out, so it keeps its default.
pub fn parse(section: &Item, problems: &mut Vec<String>) -> Values {
    let mut values = Values::default();
    let Some(table) = section.as_table_like() else {
        problems.push("appearance has to be a section, like [appearance]".to_owned());
        return values;
    };
    // Themes first, because settings below can name them.
    if let Some(themes) = table.get(THEMES_KEY) {
        values.custom = parse_themes(themes, problems);
    }
    for (name, item) in table.iter() {
        if name == THEMES_KEY {
            continue;
        }
        if !SUBTABLES.contains(&name) {
            read_setting(name, item, &mut values, problems);
            continue;
        }
        let Some(subtable) = item.as_table_like() else {
            problems.push(format!("appearance.{name} has to be a section"));
            continue;
        };
        for (key, item) in subtable.iter() {
            read_setting(&format!("{name}.{key}"), item, &mut values, problems);
        }
    }
    values
}

/// Sets a value in the file, after checking it.
pub fn set(
    document: &mut DocumentMut,
    current: &Values,
    id: &str,
    value: Value,
) -> Result<(), String> {
    let field = field(id).ok_or_else(|| format!("{id} isn't a known appearance setting"))?;
    let value = checked(field, value, &current.custom)?;
    let section = crate::keybinds::subtable(document.as_table_mut(), "appearance")?;
    let (table, key): (&mut Table, &str) = match id.split_once('.') {
        Some((subtable, key)) => (crate::keybinds::subtable(section, subtable)?, key),
        None => (section, id),
    };
    table.insert(key, toml_item(&value));
    Ok(())
}

/// Takes a value out of the file, and any section left empty, unless it has comments.
pub fn remove(document: &mut DocumentMut, id: &str) {
    let Some(section) = document
        .get_mut("appearance")
        .and_then(Item::as_table_like_mut)
    else {
        return;
    };
    match id.split_once('.') {
        None => {
            section.remove(id);
        }
        Some((subtable, key)) => {
            if let Some(values) = section.get_mut(subtable).and_then(Item::as_table_like_mut) {
                values.remove(key);
            }
            remove_if_empty(section, subtable);
        }
    }
    remove_if_empty(document.as_table_mut(), "appearance");
}

fn remove_if_empty(parent: &mut dyn toml_edit::TableLike, name: &str) {
    let Some(table) = parent.get(name).and_then(Item::as_table) else {
        return;
    };
    let commented = table
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .is_some_and(|prefix| prefix.contains('#'));
    if table.is_empty() && !commented {
        parent.remove(name);
    }
}

/// Whole numbers are written without a point, as people would write them.
fn toml_item(value: &Value) -> Item {
    match value {
        Value::Flag(flag) => toml_edit::value(*flag),
        Value::Text(text) => toml_edit::value(text.as_str()),
        // Checked against each setting's range, so far inside what i64 holds exactly.
        #[allow(clippy::cast_possible_truncation)]
        Value::Number(number) if number.fract() == 0.0 => toml_edit::value(*number as i64),
        Value::Number(number) => toml_edit::value(*number),
    }
}

fn default_line(field: &Field) -> String {
    let key = field.id.split_once('.').map_or(field.id, |(_, key)| key);
    let value = toml_item(&field.default.value());
    format!("{key} = {}\n", value.to_string().trim())
}

/// The `[appearance]` part of `dsk.defaults.toml`.
pub fn reference() -> String {
    let dark: Vec<&str> = builtin_themes()
        .iter()
        .filter(|b| b.theme.scheme == Scheme::Dark)
        .map(|b| b.theme.id.as_str())
        .collect();
    let light: Vec<&str> = builtin_themes()
        .iter()
        .filter(|b| b.theme.scheme == Scheme::Light)
        .map(|b| b.theme.id.as_str())
        .collect();
    let tokens = TOKENS
        .chunks(4)
        .map(|line| format!("#   {}", line.join(", ")))
        .collect::<Vec<_>>()
        .join("\n");

    let mut out = format!(
        "\n# Appearance\n\
         #\n\
         # mode is system, light or dark.\n\
         # Dark themes: {}\n\
         # Light themes: {}\n\
         # A font is one family's name, or empty for the one that comes with the app. Use\n\
         # system-ui or ui-monospace for the system's own.\n\
         #\n\
         # To make a theme, add a section with any name and choose it as dark_theme or light_theme:\n\
         #\n\
         #   [appearance.themes.midnight]\n\
         #   name = \"Midnight\"\n\
         #   extends = \"tokyo-night\"         # or mode = \"dark\" to start from tectr's own\n\
         #   \"surface.base\" = \"#000000\"\n\
         #   \"accent.solid\" = \"#ff7a00\"      # the tints made from it follow\n\
         #\n\
         # A theme can set any of these, as \"#rrggbb\" or \"#rrggbbaa\":\n\
         {tokens}\n\n",
        dark.join(", "),
        light.join(", "),
    );
    out.push_str("[appearance]\n");
    for field in FIELDS.iter().filter(|field| !field.id.contains('.')) {
        out.push_str(&default_line(field));
    }
    for subtable in SUBTABLES {
        out.push_str(&format!("\n[appearance.{subtable}]\n"));
        for field in FIELDS
            .iter()
            .filter(|field| field.id.starts_with(&format!("{subtable}.")))
        {
            out.push_str(&default_line(field));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a theme in `themes.toml` has to say. Everything else has a rule in `complete`.
    const REQUIRED: &[&str] = &[
        "surface.base",
        "surface.panel",
        "surface.raised",
        "surface.control",
        "line.subtle",
        "line.strong",
        "text.primary",
        "text.secondary",
        "text.tertiary",
        "accent.solid",
        "accent.text",
        "status.waiting",
        "status.failed",
        "status.online",
        "syntax.keyword",
        "syntax.string",
        "syntax.constant",
        "syntax.function",
        "syntax.type",
    ];

    fn parsed(contents: &str) -> (Values, Vec<String>) {
        let document = contents.parse::<DocumentMut>().unwrap();
        let mut problems = Vec::new();
        let values = parse(document.get("appearance").unwrap(), &mut problems);
        (values, problems)
    }

    fn themes() -> impl Iterator<Item = &'static Theme> {
        builtin_themes().iter().map(|builtin| &builtin.theme)
    }

    fn value(values: &Values, id: &str) -> Value {
        values
            .resolve()
            .fields
            .into_iter()
            .find(|field| field.id == id)
            .unwrap()
            .value
    }

    #[test]
    fn has_tectrs_own_themes_and_every_family_with_a_choice_in_each_mode() {
        let names: Vec<&str> = themes()
            .filter(|theme| !theme.id.starts_with("tectr"))
            .map(|theme| theme.name.as_str())
            .collect();
        for family in [
            "Catppuccin",
            "Tokyo Night",
            "Dracula",
            "One ",
            "GitHub",
            "Nord",
            "Gruvbox",
            "Solarized",
            "Rosé Pine",
            "Monokai",
            "Vague",
        ] {
            assert!(
                names.iter().any(|name| name.starts_with(family)),
                "{family}"
            );
        }
        assert!(themes().filter(|t| t.scheme == Scheme::Dark).count() >= 10);
        assert!(themes().filter(|t| t.scheme == Scheme::Light).count() >= 8);
    }

    #[test]
    fn every_theme_in_the_data_file_parses_and_says_everything_required() {
        let document = include_str!("themes.toml").parse::<DocumentMut>().unwrap();
        assert_eq!(
            document.iter().count(),
            builtin_themes().len(),
            "a theme was dropped"
        );
        for (id, item) in document.iter() {
            let mut problems = Vec::new();
            read_written(item.as_table_like().unwrap(), id, &mut problems);
            assert_eq!(problems, Vec::<String>::new(), "{id}");
        }
        for builtin in builtin_themes() {
            for token in REQUIRED {
                assert!(
                    builtin.set.contains_key(*token),
                    "{} doesn't set {token}",
                    builtin.theme.id
                );
            }
        }
    }

    #[test]
    fn every_theme_has_every_token_as_a_color() {
        for theme in themes() {
            for token in TOKENS {
                let color = theme.colors.get(*token);
                assert!(
                    color.is_some_and(|hex| channels(hex).is_some()),
                    "{}: {token} is {color:?}",
                    theme.id
                );
            }
            assert_eq!(theme.colors.len(), TOKENS.len(), "{}", theme.id);
        }
    }

    /// Colors with alpha laid over what's behind them.
    fn over(top: &str, below: &str) -> String {
        let [_, _, _, alpha] = channels(top).unwrap();
        mix(below, top, f64::from(alpha) / 255.0)
    }

    #[test]
    fn every_theme_is_readable() {
        let mut failures = Vec::new();
        for theme in themes() {
            let color = |token: &str| theme.colors[token].as_str();
            let mut check = |fg: &str, bg: String, ratio: f64, what: String| {
                let found = contrast(fg, &bg);
                if found < ratio {
                    failures.push(format!("{}: {what}: {found:.2}", theme.id));
                }
            };
            let surfaces =
                ["base", "panel", "raised", "overlay"].map(|name| format!("surface.{name}"));
            for fg in ["text.primary", "text.secondary"] {
                for bg in surfaces
                    .iter()
                    .map(String::as_str)
                    .chain(["surface.control"])
                {
                    check(
                        color(fg),
                        color(bg).to_owned(),
                        4.5,
                        format!("{fg} on {bg}"),
                    );
                }
            }
            for fg in [
                "text.tertiary",
                "accent.text",
                "status.waiting",
                "status.failed",
                "status.online",
            ] {
                for bg in &surfaces {
                    check(
                        color(fg),
                        color(bg).to_owned(),
                        4.5,
                        format!("{fg} on {bg}"),
                    );
                }
            }
            check(
                color("text.on-accent"),
                color("accent.solid").to_owned(),
                4.5,
                "text.on-accent on accent.solid".to_owned(),
            );
            for fg in ["text.primary", "text.secondary", "status.waiting"] {
                for tint in ["status.waiting-tint", "status.waiting-tint-strong"] {
                    for bg in ["surface.base", "surface.panel"] {
                        check(
                            color(fg),
                            over(color(tint), color(bg)),
                            4.5,
                            format!("{fg} on {tint} over {bg}"),
                        );
                    }
                }
            }
            check(
                color("diff.add-text"),
                over(color("diff.add-bg"), color("surface.base")),
                4.5,
                "diff.add-text".to_owned(),
            );
            check(
                color("diff.del-text"),
                over(color("diff.del-bg"), color("surface.base")),
                4.5,
                "diff.del-text".to_owned(),
            );
            // Syntax stays close to what each theme published, so the bar is that it can be seen.
            for role in [
                "plain",
                "keyword",
                "string",
                "constant",
                "function",
                "type",
                "comment",
                "punctuation",
            ] {
                let fg = format!("syntax.{role}");
                for bg in ["surface.panel", "surface.raised"] {
                    check(
                        color(&fg),
                        color(bg).to_owned(),
                        3.0,
                        format!("{fg} on {bg}"),
                    );
                }
            }
        }
        assert_eq!(failures, Vec::<String>::new());
    }

    /// What `design/tokens/build.mjs` writes for this crate to check against.
    fn design_defaults() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../design/tokens/generated/desktop-defaults.json"
        ))
        .unwrap()
    }

    #[test]
    fn tectrs_themes_match_the_design_tokens() {
        let defaults = design_defaults();
        for (id, mode) in [("tectr-dark", "dark"), ("tectr-light", "light")] {
            let theme = themes().find(|theme| theme.id == id).unwrap();
            let tokens: BTreeMap<String, String> =
                serde_json::from_value(defaults[mode].clone()).unwrap();
            assert_eq!(theme.colors, tokens, "{id}");
        }
    }

    #[test]
    fn default_sizes_match_the_design_tokens() {
        let defaults = design_defaults();
        for id in [
            "text.size",
            "text.line_height",
            "code.size",
            "code.line_height",
        ] {
            let Some(DefaultValue::Number(number)) = field(id).map(|field| field.default) else {
                panic!("{id} isn't a number");
            };
            assert_eq!(defaults["scale"][id].as_f64(), Some(number), "{id}");
        }
    }

    #[test]
    fn every_field_is_in_a_group_and_its_default_passes_its_own_check() {
        for field in FIELDS {
            assert!(
                GROUPS.iter().any(|group| group.id == field.group),
                "{}",
                field.id
            );
            assert_eq!(
                checked(field, field.default.value(), &[]),
                Ok(field.default.value()),
                "{}",
                field.id
            );
        }
    }

    #[test]
    fn the_shortcut_line_is_off_until_the_file_turns_it_on() {
        let shown = |values: &Values| {
            let fields = values.resolve().fields;
            let field = fields.iter().find(|f| f.id == "layout.show_shortcuts");
            field.map(|field| field.value.clone())
        };
        assert_eq!(shown(&Values::default()), Some(Value::Flag(false)));
        let (values, problems) = parsed("[appearance.layout]\nshow_shortcuts = true\n");
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(shown(&values), Some(Value::Flag(true)));
    }

    #[test]
    fn steps_start_folded_and_animated_until_the_file_says_otherwise() {
        let defaults = Values::default();
        assert_eq!(
            value(&defaults, "transcript.steps"),
            Value::Text("folded".into())
        );
        assert_eq!(value(&defaults, "transcript.animate"), Value::Flag(true));
        let (values, problems) =
            parsed("[appearance.transcript]\nsteps = \"shown\"\nanimate = false\n");
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(
            value(&values, "transcript.steps"),
            Value::Text("shown".into())
        );
        assert_eq!(value(&values, "transcript.animate"), Value::Flag(false));
    }

    #[test]
    fn the_window_color_is_the_chosen_themes_base() {
        let (values, problems) = parsed("[appearance]\nmode = \"dark\"\ndark_theme = \"nord\"\n");
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(values.forced_scheme(), Some(Scheme::Dark));
        assert_eq!(values.window_color(Scheme::Dark), [0x27, 0x2c, 0x36]);
        // The light theme wasn't chosen, so it's still tectr's.
        assert_eq!(values.window_color(Scheme::Light), [0xf5, 0xf4, 0xf7]);
        assert_eq!(Values::default().forced_scheme(), None);
    }

    #[test]
    fn any_theme_can_be_looked_up_for_the_window_whichever_is_chosen() {
        let (values, _) = parsed("[appearance.themes.mine]\nextends = \"github-light\"\n");
        assert_eq!(
            values.theme_window("vague"),
            Some((Scheme::Dark, [0x14, 0x14, 0x15]))
        );
        assert_eq!(
            values.theme_window("mine"),
            Some((Scheme::Light, [0xf6, 0xf8, 0xfa]))
        );
        assert_eq!(values.theme_window("nope"), None);
    }

    #[test]
    fn a_theme_of_the_wrong_kind_keeps_the_default_and_says_so() {
        let (values, problems) =
            parsed("[appearance]\ndark_theme = \"github-light\"\nlight_theme = \"missing\"\n");
        assert_eq!(
            value(&values, "dark_theme"),
            Value::Text("tectr-dark".into())
        );
        assert_eq!(
            value(&values, "light_theme"),
            Value::Text("tectr-light".into())
        );
        let problems = problems.join("\n");
        assert!(problems.contains("needs a dark theme"), "{problems}");
        assert!(
            problems.contains("no theme called \"missing\""),
            "{problems}"
        );
    }

    #[test]
    fn a_custom_theme_extends_another_and_what_comes_from_a_color_follows_it() {
        let (values, problems) = parsed(
            "[appearance]\ndark_theme = \"mine\"\n\
             [appearance.themes.mine]\nname = \"Mine\"\nextends = \"nord\"\n\
             \"surface.base\" = \"#000000\"\n\"accent.solid\" = \"#FF7A00\"\n",
        );
        assert_eq!(problems, Vec::<String>::new());
        let appearance = values.resolve();
        let theme = appearance.themes.iter().find(|t| t.id == "mine").unwrap();
        assert!(theme.custom);
        assert_eq!(theme.name, "Mine");
        assert_eq!(theme.scheme, Scheme::Dark);
        assert_eq!(theme.colors["surface.base"], "#000000");
        assert_eq!(theme.colors["accent.tint"], "#ff7a0029");
        // Everything it didn't set comes from Nord.
        let nord = themes().find(|t| t.id == "nord").unwrap();
        assert_eq!(theme.colors["surface.panel"], nord.colors["surface.panel"]);
        assert_eq!(values.window_color(Scheme::Dark), [0, 0, 0]);
    }

    #[test]
    fn a_custom_theme_with_only_a_mode_starts_from_tectrs_own() {
        let (values, problems) = parsed(
            "[appearance]\nlight_theme = \"paper\"\n\
             [appearance.themes.paper]\nmode = \"light\"\n\"surface.base\" = \"#fffff0\"\n",
        );
        assert_eq!(problems, Vec::<String>::new());
        let appearance = values.resolve();
        let theme = appearance.themes.iter().find(|t| t.id == "paper").unwrap();
        assert_eq!(theme.name, "paper");
        assert_eq!(theme.colors["surface.panel"], "#ffffff");
        assert_eq!(theme.colors.len(), TOKENS.len());
    }

    #[test]
    fn a_bad_custom_theme_says_what_is_wrong_and_keeps_the_rest() {
        let (values, problems) = parsed(
            "[appearance.themes.bad]\nmode = \"dusk\"\nextends = \"nope\"\n\
             \"surface.base\" = \"red\"\n\"surface.nope\" = \"#000000\"\n\
             \"surface.panel\" = \"#111111\"\n\
             [appearance.themes.nord]\nmode = \"dark\"\n",
        );
        let problems = problems.join("\n");
        for expected in [
            "themes.bad.mode",
            "extends \"nope\"",
            "themes.bad.surface.base",
            "themes.bad.surface.nope",
            "themes.nord has the same name",
        ] {
            assert!(problems.contains(expected), "{expected} in:\n{problems}");
        }
        let appearance = values.resolve();
        let theme = appearance.themes.iter().find(|t| t.id == "bad").unwrap();
        assert_eq!(theme.colors["surface.panel"], "#111111");
        assert_eq!(theme.colors["surface.base"], "#09080c");
        assert_eq!(
            appearance.themes.iter().filter(|t| t.id == "nord").count(),
            1
        );
    }

    #[test]
    fn bad_values_fall_back_and_say_so() {
        let (values, problems) = parsed(
            "[appearance]\nmode = \"dusk\"\nnope = 1\n\
             [appearance.text]\nsize = 200\nfamily = \"Evil\\\"; }\"\n\
             [appearance.code]\nligatures = \"yes\"\nsize = \"big\"\n\
             [appearance.layout]\nsidebar_width = 20\n",
        );
        for id in [
            "mode",
            "text.size",
            "text.family",
            "code.ligatures",
            "layout.sidebar_width",
        ] {
            assert_eq!(
                value(&values, id),
                field(id).unwrap().default.value(),
                "{id}"
            );
        }
        let problems = problems.join("\n");
        for expected in [
            "appearance.mode has to be one of system, light, dark",
            "appearance.nope isn't a known setting",
            "appearance.text.size has to be from 9 to 28",
            "appearance.text.family has to be one font's name",
            "appearance.code.ligatures has to be true or false",
            "appearance.code.size has to be a number",
            "appearance.layout.sidebar_width has to be from 160 to 560",
        ] {
            assert!(problems.contains(expected), "{expected} in:\n{problems}");
        }
    }

    #[test]
    fn whole_and_fractional_numbers_are_both_numbers() {
        let (values, problems) = parsed("[appearance.text]\nsize = 14\nline_height = 1.6\n");
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(value(&values, "text.size"), Value::Number(14.0));
        assert_eq!(value(&values, "text.line_height"), Value::Number(1.6));
    }

    #[test]
    fn the_webview_can_send_any_kind_of_value() {
        for (json, value) in [
            ("14", Value::Number(14.0)),
            ("13.5", Value::Number(13.5)),
            ("true", Value::Flag(true)),
            ("\"nord\"", Value::Text("nord".to_owned())),
        ] {
            assert_eq!(serde_json::from_str::<Value>(json).unwrap(), value);
        }
    }

    #[test]
    fn numbers_are_kept_tidy() {
        let line_height = field("text.line_height").unwrap();
        assert_eq!(
            checked(line_height, Value::Number(1.6 + 1e-9), &[]),
            Ok(Value::Number(1.6))
        );
    }

    #[test]
    fn a_family_can_be_a_name_with_spaces_but_not_more() {
        let family = field("code.family").unwrap();
        assert_eq!(
            checked(family, Value::Text(" JetBrains Mono ".into()), &[]),
            Ok(Value::Text("JetBrains Mono".into()))
        );
        for bad in ["a, b", "a\"", "a;b", "url(x)", "a\nb"] {
            assert!(
                checked(family, Value::Text(bad.into()), &[]).is_err(),
                "{bad}"
            );
        }
    }
}
