//! The desktop app's settings file. People and agents edit it directly, and the
//! settings screen changes it one value at a time, so it keeps its comments and layout.
//!
//! The file holds only what differs from the defaults, which live in code. To add a
//! section: add it to `SECTIONS`, give `Values` a field for it, and read it in
//! `parse_document`.

mod appearance;
mod fonts;
mod keybinds;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use toml_edit::{DocumentMut, Item};

pub use appearance::{Appearance, Scheme, Value};
pub use fonts::{InstalledFont, installed_fonts};
pub use keybinds::{Binding, GROUPS, Group, ResolvedAction};

const SECTIONS: &[&str] = &["appearance", "keybinds"];

const STARTER: &str = "\
# tectr desktop settings. Changes apply as soon as this file is saved.
# Only put what differs from the defaults here. Every setting and its default
# is in dsk.defaults.toml, next to this file.
";

const REFERENCE_HEADER: &str = "\
# Every tectr desktop setting and its default. Don't edit this file: the app rewrites
# it on launch. Copy what you want to change into dsk.toml, next to this file.
#
# Keybinds
#
#   new_topic = \"mod+n\"                         one key
#   down = [\"j\", \"down\"]                        several keys
#   next_request = []                           no key at all
#   allow_once = { key = \"y\", hold_ms = 500 }   has to be held for half a second
#   down = { key = \"j\", repeat = false }        doesn't repeat while held
#   first = [\"g g\", \"home\"]                     a sequence, with spaces between its keys
#   last = \"G\"                                  a capital on its own means Shift, like Vim
#
# Modifiers: mod (cmd on macOS, ctrl elsewhere), cmd, ctrl, alt, shift.
# Keys: a character, up, down, left, right, home, end, pageup, pagedown, enter, escape,
# tab, space, backspace, delete, plus, f1 to f24. Write symbols as they're typed, so
# \"?\" rather than \"shift+/\".
#
# A key without cmd, ctrl or alt doesn't fire while typing in a text field, except in the
# sections for one: composer, new_topic, sheet and palette.
#
# Sequences work like Vim's: the first key waits for the next until sequence_timeout_ms,
# and a key that doesn't continue it drops what came before and acts on its own. They don't
# repeat, and `hold_ms` applies to the last key. A key that also starts a longer sequence
# fires straight away rather than waiting, so the longer one never does, which the settings
# screen warns about. Keys that type a character can't be part of a sequence in a text box.
#
# `repeat` does nothing on a key with `hold_ms`, which runs once per hold. macOS doesn't
# report a key coming up while Command is down, so letting go of a held Command shortcut
# only cancels it once Command is released too.
#
# An action marked \"Follows nav\" uses that nav key until it's set itself, so changing
# nav.down moves everything that follows it. Setting sidebar.down changes only the sidebar.

";

/// What the UI needs to run and show the settings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    pub path: PathBuf,
    pub appearance: Appearance,
    pub groups: &'static [Group],
    pub keybinds: Vec<ResolvedAction>,
    pub sequence_timeout_ms: u32,
    /// Written to be shown as is: what's wrong with the file, and what's being done instead.
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct Values {
    appearance: appearance::Values,
    keybinds: HashMap<&'static str, Vec<Binding>>,
    sequence_timeout_ms: Option<u32>,
}

fn parse_document(document: &DocumentMut, problems: &mut Vec<String>) -> Values {
    let mut values = Values::default();
    for (name, item) in document.as_table().iter() {
        if !SECTIONS.contains(&name) {
            problems.push(format!("{name} isn't a known setting, so it's ignored"));
        } else if name == "appearance" {
            values.appearance = appearance::parse(item, problems);
        } else if name == "keybinds" {
            values.keybinds = keybinds::parse(item, problems);
            values.sequence_timeout_ms = keybinds::parse_timeout(item, problems);
        }
    }
    values
}

/// Writes through a temporary file, so nothing ever reads half a file. Follows a symlink,
/// so a settings file kept in someone's dotfiles stays there.
fn write_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let temporary = target.with_extension(format!("toml.{}.tmp", std::process::id()));
    let result = fs::write(&temporary, contents).and_then(|()| fs::rename(&temporary, &target));
    result.map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("Couldn't save {}: {error}", target.display())
    })
}

/// The settings file, and what was last read from it.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    /// The last version that parsed. A file that doesn't parse changes nothing, so a typo
    /// can't take the keyboard away.
    values: Values,
    problems: Vec<String>,
    /// Problems that come from this run rather than the file.
    notices: Vec<String>,
}

impl Store {
    /// Reads the file, creating it and the reference next to it if needed. Failing to
    /// create either is reported in `problems`, and the app still runs on defaults.
    pub fn open(path: PathBuf) -> Self {
        let mut store = Self {
            path,
            values: Values::default(),
            problems: Vec::new(),
            notices: Vec::new(),
        };
        if let Err(problem) = store.prepare_folder() {
            store.notices.push(problem);
        }
        store.reload();
        store
    }

    fn prepare_folder(&self) -> Result<(), String> {
        if let Some(folder) = self.path.parent() {
            fs::create_dir_all(folder)
                .map_err(|error| format!("Couldn't create {}: {error}", folder.display()))?;
        }
        if !self.path.exists() {
            write_atomically(&self.path, STARTER)?;
        }
        let reference = self.path.with_file_name("dsk.defaults.toml");
        let contents = format!(
            "{REFERENCE_HEADER}{}{}",
            keybinds::reference(),
            appearance::reference()
        );
        if fs::read_to_string(&reference).ok().as_deref() != Some(&contents) {
            fs::write(&reference, contents)
                .map_err(|error| format!("Couldn't write {}: {error}", reference.display()))?;
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reports something that isn't about the file, such as changes not being watched.
    pub fn add_notice(&mut self, notice: String) {
        self.notices.push(notice);
    }

    /// Reads the file again. Returns whether anything the UI shows has changed.
    pub fn reload(&mut self) -> bool {
        let before = self.settings();
        let mut problems = Vec::new();
        match fs::read_to_string(&self.path) {
            Ok(text) => match text.parse::<DocumentMut>() {
                Ok(document) => self.values = parse_document(&document, &mut problems),
                Err(error) => problems.push(format!(
                    "{} has a syntax error, so the last good settings are still in use: {error}",
                    self.path.display()
                )),
            },
            // Deleting the file is a fine way to go back to the defaults.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.values = Values::default();
            }
            Err(error) => problems.push(format!(
                "Couldn't read {}, so the last good settings are still in use: {error}",
                self.path.display()
            )),
        }
        self.problems = problems;
        before != self.settings()
    }

    pub fn settings(&self) -> Settings {
        let keybinds = keybinds::resolve(&self.values.keybinds);
        let mut problems = self.notices.clone();
        problems.extend(self.problems.iter().cloned());
        problems.extend(keybinds::warnings(&keybinds));
        Settings {
            path: self.path.clone(),
            appearance: self.values.appearance.resolve(),
            groups: GROUPS,
            keybinds,
            sequence_timeout_ms: self
                .values
                .sequence_timeout_ms
                .unwrap_or(keybinds::DEFAULT_SEQUENCE_TIMEOUT_MS),
            problems,
        }
    }

    /// The mode in effect, or None when it follows the system.
    pub fn forced_scheme(&self) -> Option<Scheme> {
        self.values.appearance.forced_scheme()
    }

    /// The color behind the window in this scheme, as red, green and blue.
    pub fn window_color(&self, scheme: Scheme) -> [u8; 3] {
        self.values.appearance.window_color(scheme)
    }

    /// A theme's scheme and the color behind the window with it, if there's such a theme.
    pub fn theme_window(&self, id: &str) -> Option<(Scheme, [u8; 3])> {
        self.values.appearance.theme_window(id)
    }

    /// Changes one value, reading the file first so an edit made since the last read stays.
    fn edit(
        &mut self,
        change: impl FnOnce(&mut DocumentMut) -> Result<(), String>,
    ) -> Result<(), String> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => STARTER.to_owned(),
            Err(error) => return Err(format!("Couldn't read {}: {error}", self.path.display())),
        };
        let mut document = text.parse::<DocumentMut>().map_err(|error| {
            format!(
                "{} has a syntax error, so nothing was saved. Fix it first: {error}",
                self.path.display()
            )
        })?;
        change(&mut document)?;
        write_atomically(&self.path, &document.to_string())?;
        self.reload();
        Ok(())
    }

    /// Sets an action's keys. An empty list leaves it with no key.
    pub fn set_keybind(&mut self, id: &str, bindings: Vec<Binding>) -> Result<(), String> {
        let action = keybinds::find(id).ok_or_else(|| format!("{id} isn't a known action"))?;
        let (context, name) = id
            .split_once('.')
            .ok_or_else(|| format!("{id} has no section"))?;
        let bindings = bindings
            .into_iter()
            .map(|binding| {
                Ok(Binding {
                    key: keybinds::canonical_key(&binding.key)?,
                    ..binding
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let value = keybinds::value_of(action, &bindings);
        self.edit(|document| {
            let section = keybinds::subtable(document.as_table_mut(), "keybinds")?;
            let context = keybinds::subtable(section, context)?;
            context.insert(name, Item::Value(value));
            Ok(())
        })
    }

    /// Takes an action out of the file, so it's back to its default.
    pub fn reset_keybind(&mut self, id: &str) -> Result<(), String> {
        let (context, name) = id
            .split_once('.')
            .ok_or_else(|| format!("{id} has no section"))?;
        self.edit(|document| {
            let Some(section) = document
                .get_mut("keybinds")
                .and_then(Item::as_table_like_mut)
            else {
                return Ok(());
            };
            let Some(actions) = section.get_mut(context).and_then(Item::as_table_like_mut) else {
                return Ok(());
            };
            actions.remove(name);
            if actions.is_empty() {
                let commented = section
                    .get(context)
                    .and_then(Item::as_table)
                    .and_then(|table| table.decor().prefix())
                    .and_then(|prefix| prefix.as_str())
                    .is_some_and(|prefix| prefix.contains('#'));
                if !commented {
                    section.remove(context);
                }
            }
            Ok(())
        })
    }

    /// Sets one appearance setting, if it's something that setting can be.
    pub fn set_appearance(&mut self, id: &str, value: Value) -> Result<(), String> {
        self.edit(|document| {
            // Read from what's being edited, in case a theme was added since the last read.
            let mut problems = Vec::new();
            let current = document
                .get("appearance")
                .map(|section| appearance::parse(section, &mut problems))
                .unwrap_or_default();
            appearance::set(document, &current, id, value)
        })
    }

    /// Takes an appearance setting out of the file, so it's back to its default.
    pub fn reset_appearance(&mut self, id: &str) -> Result<(), String> {
        self.edit(|document| {
            appearance::remove(document, id);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(contents: &str) -> (tempfile::TempDir, Store) {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("nested").join("dsk.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        let store = Store::open(path);
        (folder, store)
    }

    fn action(store: &Store, id: &str) -> ResolvedAction {
        store
            .settings()
            .keybinds
            .into_iter()
            .find(|action| action.id == id)
            .unwrap()
    }

    fn keys(store: &Store, id: &str) -> Vec<String> {
        action(store, id)
            .bindings
            .into_iter()
            .map(|binding| binding.key)
            .collect()
    }

    fn on(key: &str) -> Binding {
        Binding {
            key: key.to_owned(),
            hold_ms: 0,
            repeat: false,
        }
    }

    #[test]
    fn creates_the_file_and_a_reference_that_are_both_valid() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("tectr").join("dsk.toml");
        let store = Store::open(path.clone());
        assert_eq!(store.settings().problems, Vec::<String>::new());
        assert_eq!(fs::read_to_string(&path).unwrap(), STARTER);

        let reference = fs::read_to_string(path.with_file_name("dsk.defaults.toml")).unwrap();
        let copied = Store::open({
            let copy = folder.path().join("copy").join("dsk.toml");
            fs::create_dir_all(copy.parent().unwrap()).unwrap();
            fs::write(&copy, &reference).unwrap();
            copy
        });
        // Used as a settings file, the reference changes nothing.
        assert_eq!(copied.settings().problems, Vec::<String>::new());
        let (with_reference, defaults) = (copied.settings().keybinds, store.settings().keybinds);
        for (a, b) in with_reference.iter().zip(&defaults) {
            assert_eq!(a.bindings, b.bindings, "{}", a.id);
        }
    }

    #[test]
    fn changing_nav_moves_everything_that_follows_it() {
        let (_folder, store) = store_with("[keybinds.nav]\ndown = \"n\"\n");
        for id in ["sidebar.down", "pane.down", "device.down"] {
            assert_eq!(keys(&store, id), ["n"], "{id}");
        }
        // Typing searches the palette and pickers, so they follow their own default instead.
        for id in ["palette.down", "picker.down"] {
            assert!(!keys(&store, id).contains(&"n".to_owned()), "{id}");
        }
    }

    #[test]
    fn lists_you_type_in_follow_their_own_nav_keys() {
        let (_folder, store) = store_with("");
        assert!(keys(&store, "palette.down").contains(&"down".to_owned()));
        assert_eq!(
            keys(&store, "palette.up").contains(&"ctrl+k".to_owned()),
            cfg!(target_os = "macos")
        );
        assert!(action(&store, "palette.down").bindings[0].repeat);

        let (_folder, store) = store_with("[keybinds.nav]\ninput_down = \"alt+j\"\n");
        assert_eq!(keys(&store, "palette.down"), ["alt+j"]);
        assert_eq!(keys(&store, "picker.down"), ["alt+j"]);
        assert_ne!(keys(&store, "sidebar.down"), ["alt+j"]);

        let (_folder, store) = store_with(
            "[keybinds.nav]\ninput_down = \"alt+j\"\n[keybinds.palette]\ndown = \"alt+p\"\n",
        );
        assert_eq!(keys(&store, "palette.down"), ["alt+p"]);
    }

    #[test]
    fn pickers_leave_every_letter_for_their_search() {
        let (_folder, store) = store_with("");
        for id in ["picker.up", "picker.down"] {
            let keys = keys(&store, id);
            assert!(
                keys.iter().all(|key| key.contains('+') || key.len() > 1),
                "{id}: {keys:?}"
            );
        }
    }

    #[test]
    fn sequences_are_read_and_saved_like_any_other_key() {
        let (_folder, mut store) = store_with("[keybinds.nav]\nfirst = \"g  g\"\nlast = \"G\"\n");
        assert_eq!(keys(&store, "sidebar.first"), ["g g"]);
        assert_eq!(keys(&store, "pane.last"), ["shift+g"]);
        store
            .set_keybind("settings.reset", vec![on("g r")])
            .unwrap();
        assert!(
            fs::read_to_string(store.path())
                .unwrap()
                .contains("reset = \"g r\"")
        );
        assert_eq!(store.settings().problems, Vec::<String>::new());
    }

    #[test]
    fn the_sequence_timeout_can_be_set() {
        let (_folder, store) = store_with("");
        assert_eq!(store.settings().sequence_timeout_ms, 1000);
        let (_folder, store) = store_with("[keybinds]\nsequence_timeout_ms = 400\n");
        assert_eq!(store.settings().sequence_timeout_ms, 400);
        assert_eq!(store.settings().problems, Vec::<String>::new());
        let (_folder, store) = store_with("[keybinds]\nsequence_timeout_ms = 5\n");
        assert_eq!(store.settings().sequence_timeout_ms, 1000);
        assert!(store.settings().problems[0].contains("sequence_timeout_ms"));
    }

    #[test]
    fn a_key_that_starts_a_sequence_is_flagged() {
        let (_folder, store) = store_with("[keybinds.pane]\nup = \"g\"\n");
        let problems = store.settings().problems.join("\n");
        assert!(
            problems.contains("g on pane.up starts g g on pane.first"),
            "{problems}"
        );
        // And the other way round, when the longer one is listed first.
        let (_folder, store) = store_with("[keybinds.pane]\nback = \"g\"\n");
        let problems = store.settings().problems.join("\n");
        assert!(
            problems.contains("g on pane.back starts g g on pane.first"),
            "{problems}"
        );
    }

    #[test]
    fn a_typing_sequence_in_a_text_box_is_flagged() {
        let (_folder, store) =
            store_with("[keybinds.composer]\nblur = \"j k\"\nsend = \"ctrl+x ctrl+s\"\n");
        let problems = store.settings().problems.join("\n");
        assert!(
            problems.contains("j k on composer.blur never fires"),
            "{problems}"
        );
        assert!(!problems.contains("composer.send"), "{problems}");
    }

    #[test]
    fn a_palette_key_that_would_close_the_palette_is_flagged() {
        let (_folder, store) = store_with("[keybinds.palette]\ndown = \"mod+k\"\n");
        let problems = store.settings().problems.join("\n");
        assert!(
            problems.contains("global.palette and palette.down"),
            "{problems}"
        );
    }

    #[test]
    fn one_function_can_leave_nav_alone() {
        let (_folder, store) = store_with(
            "[keybinds.nav]\ndown = \"n\"\n[keybinds.sidebar]\ndown = [\"s\", \"down\"]\n",
        );
        assert_eq!(keys(&store, "sidebar.down"), ["s", "down"]);
        assert_eq!(keys(&store, "pane.down"), ["n"]);
        assert!(action(&store, "sidebar.down").customized);
        assert!(!action(&store, "pane.down").customized);
    }

    #[test]
    fn inherited_keys_repeat_like_nav_and_a_key_can_say_otherwise() {
        let (_folder, store) = store_with(
            "[keybinds.permission]\nallow_once = { key = \"y\", hold_ms = 500 }\n\
             [keybinds.pane]\ndown = { key = \"j\", repeat = false }\n",
        );
        assert!(action(&store, "sidebar.down").bindings[0].repeat);
        assert_eq!(
            action(&store, "permission.allow_once").bindings,
            [Binding {
                key: "y".to_owned(),
                hold_ms: 500,
                repeat: false
            }]
        );
        assert!(!action(&store, "pane.down").bindings[0].repeat);
    }

    #[test]
    fn an_empty_list_unbinds() {
        let (_folder, store) = store_with("[keybinds.permission]\nallow_always = []\n");
        assert!(keys(&store, "permission.allow_always").is_empty());
    }

    #[test]
    fn a_bad_entry_falls_back_and_says_so() {
        let (_folder, store) = store_with(
            "[keybinds.global]\npalette = \"hyper+k\"\nnew_topic = \"mod+y\"\n\
             interrupt = { key = \"x\", hold_ms = 99999 }\nmissing = \"x\"\n\
             [keybinds.nope]\nx = \"y\"\n\
             [other]\nx = 1\n",
        );
        assert_eq!(keys(&store, "global.palette"), ["mod+k", ":"]);
        assert_eq!(keys(&store, "global.new_topic"), ["mod+y"]);
        assert!(
            keys(&store, "global.interrupt")
                .iter()
                .any(|key| key == "mod+.")
        );
        let problems = store.settings().problems.join("\n");
        for expected in [
            "hyper",
            "hold_ms",
            "global.missing",
            "keybinds.nope",
            "other",
        ] {
            assert!(problems.contains(expected), "{expected} in:\n{problems}");
        }
    }

    #[test]
    fn a_syntax_error_keeps_the_last_good_settings() {
        let (_folder, mut store) = store_with("[keybinds.global]\npalette = \"mod+y\"\n");
        fs::write(store.path(), "[keybinds.global\npalette =").unwrap();
        assert!(store.reload());
        assert_eq!(keys(&store, "global.palette"), ["mod+y"]);
        assert!(store.settings().problems[0].contains("syntax error"));

        // And the screen won't write over a file someone's in the middle of fixing.
        let error = store
            .set_keybind("global.palette", vec![on("mod+z")])
            .unwrap_err();
        assert!(error.contains("syntax error"));
        assert_eq!(
            fs::read_to_string(store.path()).unwrap(),
            "[keybinds.global\npalette ="
        );
    }

    #[test]
    fn deleting_the_file_goes_back_to_the_defaults() {
        let (_folder, mut store) = store_with("[keybinds.global]\npalette = \"mod+y\"\n");
        fs::remove_file(store.path()).unwrap();
        assert!(store.reload());
        assert_eq!(keys(&store, "global.palette"), ["mod+k", ":"]);
        assert!(!store.reload());
    }

    #[test]
    fn saving_keeps_comments_layout_and_other_sections() {
        let original = "# My settings\n\n[appearance] # for later\nmode = \"dark\"\n\n\
                        [keybinds.global]\n# I like this one\npalette = \"mod+y\"\n";
        let (_folder, mut store) = store_with(original);
        store
            .set_keybind("global.new_topic", vec![on("mod+g")])
            .unwrap();
        store
            .set_keybind("sidebar.add", vec![on("x"), on("mod+x")])
            .unwrap();
        let saved = fs::read_to_string(store.path()).unwrap();
        for kept in [
            "# My settings",
            "# for later",
            "mode = \"dark\"",
            "# I like this one",
        ] {
            assert!(saved.contains(kept), "{kept} in:\n{saved}");
        }
        assert!(saved.contains("new_topic = \"mod+g\""), "{saved}");
        assert!(saved.contains("[keybinds.sidebar]"), "{saved}");
        assert!(saved.contains("add = [\"x\", \"mod+x\"]"), "{saved}");
        assert_eq!(keys(&store, "global.palette"), ["mod+y"]);
    }

    #[test]
    fn saving_includes_an_edit_made_since_the_last_read() {
        let (_folder, mut store) = store_with("");
        fs::write(store.path(), "[keybinds.global]\npalette = \"mod+y\"\n").unwrap();
        store
            .set_keybind("global.new_topic", vec![on("mod+g")])
            .unwrap();
        assert_eq!(keys(&store, "global.palette"), ["mod+y"]);
        assert_eq!(keys(&store, "global.new_topic"), ["mod+g"]);
    }

    #[test]
    fn saving_writes_the_short_form_unless_a_key_needs_more() {
        let (_folder, mut store) = store_with("");
        store
            .set_keybind(
                "permission.reject",
                vec![Binding {
                    key: "n".to_owned(),
                    hold_ms: 800,
                    repeat: false,
                }],
            )
            .unwrap();
        // `repeat` matches this action's default, so it isn't written.
        let saved = fs::read_to_string(store.path()).unwrap();
        assert!(
            saved.contains("reject = { key = \"n\", hold_ms = 800 }"),
            "{saved}"
        );
        assert_eq!(action(&store, "permission.reject").bindings[0].hold_ms, 800);

        // sidebar.down follows nav.down, which repeats, so true is the short form here.
        store
            .set_keybind(
                "sidebar.down",
                vec![Binding {
                    key: "s".to_owned(),
                    hold_ms: 0,
                    repeat: true,
                }],
            )
            .unwrap();
        let saved = fs::read_to_string(store.path()).unwrap();
        assert!(saved.contains("down = \"s\""), "{saved}");
    }

    #[test]
    fn saving_can_unbind_and_refuses_bad_input() {
        let (_folder, mut store) = store_with("");
        store.set_keybind("global.interrupt", Vec::new()).unwrap();
        assert!(keys(&store, "global.interrupt").is_empty());
        assert!(
            fs::read_to_string(store.path())
                .unwrap()
                .contains("interrupt = []")
        );

        assert!(store.set_keybind("global.nope", vec![on("x")]).is_err());
        assert!(
            store
                .set_keybind("global.palette", vec![on("hyper+x")])
                .is_err()
        );
    }

    #[test]
    fn resetting_removes_the_value_and_a_section_left_empty() {
        let (_folder, mut store) =
            store_with("[keybinds.global]\npalette = \"mod+y\"\n[keybinds.pane]\nup = \"u\"\n");
        store.reset_keybind("global.palette").unwrap();
        assert_eq!(keys(&store, "global.palette"), ["mod+k", ":"]);
        assert!(!action(&store, "global.palette").customized);
        let saved = fs::read_to_string(store.path()).unwrap();
        assert!(!saved.contains("keybinds.global"), "{saved}");
        assert!(saved.contains("[keybinds.pane]"), "{saved}");
    }

    #[test]
    fn conflicts_are_reported_only_where_the_keys_are_live_together() {
        let (_folder, store) = store_with(
            "[keybinds.global]\npalette = \"mod+y\"\nnew_topic = \"mod+y\"\n\
             [keybinds.pane]\nup = \"y\"\n",
        );
        let problems = store.settings().problems.join("\n");
        assert!(
            problems.contains("global.palette and global.new_topic"),
            "{problems}"
        );
        // pane.up shares Y with permission.allow_once, and both are live at once.
        assert!(problems.contains("permission.allow_once"), "{problems}");
        // new_tab.agent and sidebar.add share A by default, but they're never live at once.
        assert!(!problems.contains("new_tab.agent"), "{problems}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keys_the_macos_menu_takes_are_flagged() {
        let (_folder, store) = store_with("[keybinds.global]\nnew_tab = \"cmd+q\"\n");
        let problems = store.settings().problems.join("\n");
        assert!(problems.contains("cmd+q on global.new_tab"), "{problems}");
    }

    fn appearance_value(store: &Store, id: &str) -> Value {
        store
            .settings()
            .appearance
            .fields
            .into_iter()
            .find(|field| field.id == id)
            .unwrap()
            .value
    }

    #[test]
    fn appearance_saves_to_a_file_that_reads_back_the_same() {
        let (_folder, mut store) = store_with("");
        store
            .set_appearance("mode", Value::Text("dark".to_owned()))
            .unwrap();
        store
            .set_appearance("dark_theme", Value::Text("nord".to_owned()))
            .unwrap();
        store
            .set_appearance("text.size", Value::Number(14.0))
            .unwrap();
        store
            .set_appearance("code.line_height", Value::Number(1.6 + 1e-9))
            .unwrap();
        store
            .set_appearance("code.ligatures", Value::Flag(false))
            .unwrap();
        store
            .set_appearance("code.family", Value::Text("JetBrains Mono".to_owned()))
            .unwrap();

        let saved = fs::read_to_string(store.path()).unwrap();
        for expected in [
            "[appearance]",
            "mode = \"dark\"",
            "dark_theme = \"nord\"",
            "[appearance.text]",
            "size = 14\n",
            "line_height = 1.6\n",
            "ligatures = false",
            "family = \"JetBrains Mono\"",
        ] {
            assert!(saved.contains(expected), "{expected} in:\n{saved}");
        }

        let again = Store::open(store.path().to_owned());
        assert_eq!(again.settings().problems, Vec::<String>::new());
        assert_eq!(appearance_value(&again, "text.size"), Value::Number(14.0));
        assert_eq!(
            appearance_value(&again, "code.ligatures"),
            Value::Flag(false)
        );
        assert_eq!(again.forced_scheme(), Some(Scheme::Dark));
    }

    #[test]
    fn appearance_saving_refuses_bad_input_and_leaves_the_file_alone() {
        let (_folder, mut store) = store_with("# Mine\n");
        for (id, value) in [
            ("nope", Value::Flag(true)),
            ("mode", Value::Text("dusk".to_owned())),
            ("dark_theme", Value::Text("github-light".to_owned())),
            ("text.size", Value::Number(1000.0)),
            ("text.size", Value::Text("big".to_owned())),
            ("code.family", Value::Text("a; b".to_owned())),
        ] {
            assert!(store.set_appearance(id, value).is_err(), "{id}");
        }
        assert_eq!(fs::read_to_string(store.path()).unwrap(), "# Mine\n");
    }

    #[test]
    fn appearance_can_choose_a_theme_written_since_the_last_read() {
        let (_folder, mut store) = store_with("");
        fs::write(
            store.path(),
            "[appearance.themes.mine]\nextends = \"nord\"\n",
        )
        .unwrap();
        store
            .set_appearance("dark_theme", Value::Text("mine".to_owned()))
            .unwrap();
        assert_eq!(store.settings().problems, Vec::<String>::new());
        assert_eq!(store.window_color(Scheme::Dark), [0x27, 0x2c, 0x36]);
    }

    #[test]
    fn appearance_resets_and_tidies_up_what_it_empties() {
        let (_folder, mut store) = store_with(
            "[appearance]\nmode = \"dark\"\n[appearance.text]\nsize = 14\n\
             # Keep this note\n[appearance.code]\nsize = 14\n",
        );
        store.reset_appearance("mode").unwrap();
        store.reset_appearance("text.size").unwrap();
        store.reset_appearance("code.size").unwrap();
        assert_eq!(
            appearance_value(&store, "mode"),
            Value::Text("system".to_owned())
        );
        assert_eq!(appearance_value(&store, "text.size"), Value::Number(13.0));
        let saved = fs::read_to_string(store.path()).unwrap();
        assert!(!saved.contains("appearance.text"), "{saved}");
        assert!(saved.contains("# Keep this note"), "{saved}");
    }

    #[test]
    fn appearance_leaves_no_empty_header_behind() {
        let (_folder, mut store) = store_with("[appearance]\nmode = \"dark\"\n");
        store.reset_appearance("mode").unwrap();
        assert_eq!(fs::read_to_string(store.path()).unwrap().trim(), "");
    }

    #[test]
    fn a_syntax_error_keeps_the_last_good_appearance() {
        let (_folder, mut store) = store_with("[appearance]\ndark_theme = \"nord\"\n");
        fs::write(store.path(), "[appearance\n").unwrap();
        store.reload();
        assert_eq!(store.window_color(Scheme::Dark), [0x27, 0x2c, 0x36]);
        assert!(
            store
                .set_appearance("mode", Value::Text("dark".to_owned()))
                .unwrap_err()
                .contains("syntax error")
        );
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlink_keeps_it_a_symlink() {
        let folder = tempfile::tempdir().unwrap();
        let real = folder.path().join("dotfiles.toml");
        fs::write(&real, "").unwrap();
        let link = folder.path().join("dsk.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mut store = Store::open(link.clone());
        store
            .set_keybind("global.palette", vec![on("mod+y")])
            .unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(fs::read_to_string(&real).unwrap().contains("mod+y"));
    }
}
