//! Brings a settings file written by an older tectr up to the layout this one reads.
//!
//! The file says which layout it's in with `version`, on its first lines. One that doesn't
//! say predates versions, which makes it version 1.
//!
//! To change the layout: raise `CURRENT`, add a step to `STEPS` that takes the version before
//! it there, and give it a test with a file as people had it. Steps only move things, so
//! comments, and everything a step doesn't know about, stay as written.

use toml_edit::{DocumentMut, Item, Key, Table, TableLike};

use crate::appearance::remove_if_empty;
use crate::keybinds::subtable;

/// The layout this tectr reads and writes.
pub const CURRENT: i64 = 2;
pub const KEY: &str = "version";

/// What a new file starts with. The version goes right under it.
pub const HEADER: &str = "\
# tectr desktop settings. Changes apply as soon as this file is saved.
# Only put what differs from the defaults here. Every setting and its default
# is in dsk.defaults.toml, next to this file.
";

/// One step for each version before the current one: the first takes version 1 to 2.
const STEPS: &[fn(&mut Table)] = &[sections_per_page];

/// Version 2 gave every page of settings a section of its own. Until then they were all
/// under `[appearance]`, whatever they were about.
fn sections_per_page(root: &mut Table) {
    const MOVES: &[(&str, &str)] = &[
        ("appearance.layout.show_shortcuts", "general.show_shortcuts"),
        (
            "appearance.layout.remote_path_delay",
            "general.remote_path_delay",
        ),
        (
            "appearance.layout.animate_status",
            "appearance.motion.status_icons",
        ),
        (
            "appearance.transcript.animate",
            "appearance.motion.transcript",
        ),
        ("appearance.transcript.steps", "conversation.steps"),
        ("appearance.images", "conversation.images"),
        ("appearance.terminal", "terminal"),
        ("appearance.vim", "vim"),
    ];
    for (from, to) in MOVES {
        let from: Vec<&str> = from.split('.').collect();
        let to: Vec<&str> = to.split('.').collect();
        move_item(root, &from, &to);
    }
}

/// Which layout the file is in, or what's wrong with how it says so.
pub fn version(document: &DocumentMut) -> Result<i64, String> {
    match document.get(KEY) {
        None => Ok(1),
        Some(item) => item
            .as_integer()
            .filter(|version| *version >= 1)
            .ok_or_else(|| format!("{KEY} has to be a whole number, 1 or more")),
    }
}

/// What's wrong with the file's version, if anything, written to be shown as is.
pub fn problem(document: &DocumentMut) -> Option<String> {
    match version(document) {
        Err(problem) => Some(format!("{problem}. Reading the file as version {CURRENT}.")),
        Ok(found) if found > CURRENT => Some(format!(
            "This file is version {found}, from a newer tectr than this one, which reads \
             version {CURRENT}. What this one doesn't know is ignored, and it won't save to \
             the file."
        )),
        Ok(_) => None,
    }
}

/// The file in the current layout, and the version it was in, if that was an older one.
/// None means there's nothing to do, or no telling what to do.
pub fn upgraded(document: &DocumentMut) -> Option<(i64, String)> {
    let from = version(document).ok().filter(|found| *found < CURRENT)?;
    let mut document = document.clone();
    let done = usize::try_from(from - 1).ok()?;
    for step in STEPS.iter().skip(done) {
        step(document.as_table_mut());
    }
    // A file that says it's version 1 keeps saying so where it did.
    let said = document.contains_key(KEY);
    if said {
        document[KEY] = toml_edit::value(CURRENT);
    }
    let text = document.to_string();
    Some((from, if said { text } else { stamped(&text) }))
}

/// Puts the version under the header a new file starts with, or first when that's gone, so
/// it's always where the file starts.
pub fn stamped(text: &str) -> String {
    let line = format!("{KEY} = {CURRENT}\n");
    match text.strip_prefix(HEADER) {
        Some(rest) if rest.is_empty() || rest.starts_with('\n') => {
            format!("{HEADER}{line}{rest}")
        }
        Some(rest) => format!("{HEADER}{line}\n{rest}"),
        None if text.is_empty() => line,
        None => format!("{line}\n{text}"),
    }
}

fn get<'a>(table: &'a dyn TableLike, path: &[&str]) -> Option<&'a Item> {
    let (name, rest) = path.split_first()?;
    let item = table.get(name)?;
    if rest.is_empty() {
        Some(item)
    } else {
        get(item.as_table_like()?, rest)
    }
}

/// Whether sections that can take another entry lead to `path`, or could be made to.
fn reachable(table: &Table, path: &[&str]) -> bool {
    match path.split_first() {
        None => true,
        Some((name, rest)) => match table.get(name) {
            None => true,
            Some(Item::Table(inner)) => reachable(inner, rest),
            Some(_) => false,
        },
    }
}

/// Takes out what's at `path`, with its key, which is what holds the comments above it.
fn take(table: &mut dyn TableLike, path: &[&str]) -> Option<(Key, Item)> {
    let (name, rest) = path.split_first()?;
    if rest.is_empty() {
        let key = table.key(name)?.clone();
        return Some((key, table.remove(name)?));
    }
    take(table.get_mut(name)?.as_table_like_mut()?, rest)
}

/// Takes out the sections along `path` that have nothing left in them, innermost first.
fn prune(table: &mut dyn TableLike, path: &[&str]) {
    let Some((name, rest)) = path.split_first() else {
        return;
    };
    if let Some(inner) = table.get_mut(name).and_then(Item::as_table_like_mut) {
        prune(inner, rest);
    }
    remove_if_empty(table, name);
}

/// Moves what's at `from` to `to`, with the comments above it.
///
/// Nothing is ever dropped. Where both places hold a section, what the new one doesn't say
/// comes over from the old one. Where the new place is taken, or can't be written to, the
/// old one stays as it is, and reading the file reports it as something it doesn't know.
fn move_item(root: &mut Table, from: &[&str], to: &[&str]) {
    let Some(source) = get(root, from) else {
        return;
    };
    if let Some(taken) = get(root, to) {
        let (Some(old), Some(_)) = (source.as_table_like(), taken.as_table_like()) else {
            return;
        };
        let names: Vec<String> = old.iter().map(|(name, _)| name.to_owned()).collect();
        for name in &names {
            let from = [from, &[name.as_str()]].concat();
            let to = [to, &[name.as_str()]].concat();
            move_item(root, &from, &to);
        }
        prune(root, from);
        return;
    }
    let (Some((leaf, parents)), Some((_, emptied))) = (to.split_last(), from.split_last()) else {
        return;
    };
    if !reachable(root, parents) {
        return;
    }
    let Some((key, item)) = take(root, from) else {
        return;
    };
    prune(root, emptied);
    let mut table = root;
    for name in parents {
        // Checked by `reachable`, so this only fails to leave the item where it can't go.
        let Ok(inner) = subtable(table, name) else {
            return;
        };
        table = inner;
    }
    let key = if key.get() == *leaf {
        key
    } else {
        Key::new(*leaf).with_leaf_decor(key.leaf_decor().clone())
    };
    table.insert_formatted(&key, item);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upgrade(text: &str) -> Option<String> {
        let document = text.parse::<DocumentMut>().unwrap();
        upgraded(&document).map(|(_, text)| text)
    }

    #[test]
    fn there_is_a_step_from_every_older_version() {
        assert_eq!(i64::try_from(STEPS.len()).unwrap(), CURRENT - 1);
    }

    #[test]
    fn a_file_in_the_current_layout_is_left_alone() {
        assert_eq!(upgrade(&stamped(HEADER)), None);
        assert_eq!(
            upgrade(&format!("{KEY} = {CURRENT}\n[vim]\nenabled = true\n")),
            None
        );
    }

    #[test]
    fn a_new_file_and_an_untouched_old_one_end_up_the_same() {
        let new = stamped(HEADER);
        assert_eq!(new, format!("{HEADER}{KEY} = {CURRENT}\n"));
        assert_eq!(upgrade(HEADER), Some(new));
        assert_eq!(upgrade(""), Some(format!("{KEY} = {CURRENT}\n")));
    }

    #[test]
    fn the_version_goes_under_the_header_and_above_everything_else() {
        let old = format!("{HEADER}\n[keybinds.global]\npalette = \"mod+p\"\n");
        assert_eq!(
            upgrade(&old).unwrap(),
            format!("{HEADER}{KEY} = {CURRENT}\n\n[keybinds.global]\npalette = \"mod+p\"\n")
        );
        // Without the header, someone's own first lines aren't taken for it.
        let own = "# My keys\n[keybinds.global]\npalette = \"mod+p\"\n";
        assert_eq!(upgrade(own).unwrap(), format!("{KEY} = {CURRENT}\n\n{own}"));
    }

    #[test]
    fn every_page_gets_its_own_section_from_what_was_under_appearance() {
        let old = "\
[appearance]
mode = \"dark\"

[appearance.text]
size = 14

[appearance.layout]
density = 90
show_shortcuts = true
remote_path_delay = 200
animate_status = false

[appearance.transcript]
steps = \"shown\"
animate = false

[appearance.images]
transcript = false

[appearance.terminal]
size = 12
cursor = \"bar\"

[appearance.vim]
enabled = true
escape = \"jk\"

[appearance.themes.mine]
extends = \"nord\"

[keybinds.global]
palette = \"mod+p\"
";
        let new = upgrade(old).unwrap();
        let document = new.parse::<DocumentMut>().unwrap();
        let at = |path: &str| {
            let path: Vec<&str> = path.split('.').collect();
            get(document.as_table(), &path).map(|item| item.to_string().trim().to_owned())
        };
        for (path, value) in [
            ("version", "2"),
            ("appearance.mode", "\"dark\""),
            ("appearance.text.size", "14"),
            ("appearance.layout.density", "90"),
            ("general.show_shortcuts", "true"),
            ("general.remote_path_delay", "200"),
            ("appearance.motion.status_icons", "false"),
            ("appearance.motion.transcript", "false"),
            ("conversation.steps", "\"shown\""),
            ("conversation.images.transcript", "false"),
            ("terminal.size", "12"),
            ("terminal.cursor", "\"bar\""),
            ("vim.enabled", "true"),
            ("vim.escape", "\"jk\""),
            ("appearance.themes.mine.extends", "\"nord\""),
            ("keybinds.global.palette", "\"mod+p\""),
        ] {
            assert_eq!(at(path).as_deref(), Some(value), "{path} in:\n{new}");
        }
        for gone in [
            "appearance.layout.show_shortcuts",
            "appearance.layout.remote_path_delay",
            "appearance.layout.animate_status",
            "appearance.transcript",
            "appearance.images",
            "appearance.terminal",
            "appearance.vim",
        ] {
            assert_eq!(at(gone), None, "{gone} in:\n{new}");
        }
        // Whole sections keep their place in the file, under their new name.
        assert!(
            new.contains("\n[terminal]\nsize = 12\ncursor = \"bar\"\n"),
            "{new}"
        );
        assert!(
            new.contains("\n[vim]\nenabled = true\nescape = \"jk\"\n"),
            "{new}"
        );
        // And it's done: upgrading what came out changes nothing.
        assert_eq!(upgrade(&new), None);
    }

    #[test]
    fn comments_move_with_what_they_are_about() {
        let old = "\
# How I like my shell
[appearance.terminal]
size = 12 # small

[appearance.layout]
# Always show me the keys
show_shortcuts = true
";
        let new = upgrade(old).unwrap();
        assert!(
            new.contains("# How I like my shell\n[terminal]\nsize = 12 # small\n"),
            "{new}"
        );
        assert!(
            new.contains("[general]\n# Always show me the keys\nshow_shortcuts = true\n"),
            "{new}"
        );
        // Its last setting moved out, so the section went with it.
        assert!(!new.contains("appearance"), "{new}");
    }

    #[test]
    fn a_section_someone_commented_stays_even_when_it_empties() {
        let old = "# Layout tweaks go here\n[appearance.layout]\nshow_shortcuts = true\n";
        let new = upgrade(old).unwrap();
        assert!(
            new.contains("# Layout tweaks go here\n[appearance.layout]\n"),
            "{new}"
        );
        assert!(new.contains("[general]\nshow_shortcuts = true\n"), "{new}");
    }

    #[test]
    fn what_is_already_in_the_new_place_wins_and_the_old_stays_to_be_seen() {
        let old = "\
[vim]
enabled = false

[appearance.vim]
enabled = true
escape = \"jk\"
";
        let new = upgrade(old).unwrap();
        let document = new.parse::<DocumentMut>().unwrap();
        let vim = document["vim"].as_table().unwrap();
        assert_eq!(vim["enabled"].as_bool(), Some(false));
        // What the new section didn't say came over.
        assert_eq!(vim["escape"].as_str(), Some("jk"));
        // What it did say is still where it was, for reading the file to point out.
        assert_eq!(
            document["appearance"]["vim"]["enabled"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn inline_and_dotted_sections_move_too() {
        let new = upgrade("[appearance]\nvim = { enabled = true }\nterminal.size = 12\n").unwrap();
        let document = new.parse::<DocumentMut>().unwrap();
        let root = document.as_table();
        assert_eq!(
            get(root, &["vim", "enabled"]).and_then(Item::as_bool),
            Some(true),
            "{new}"
        );
        assert_eq!(
            get(root, &["terminal", "size"]).and_then(Item::as_integer),
            Some(12),
            "{new}"
        );
        assert!(get(root, &["appearance"]).is_none(), "{new}");
    }

    #[test]
    fn a_file_that_says_version_1_is_upgraded_where_it_says_so() {
        let new = upgrade("# Mine\nversion = 1\n\n[appearance.vim]\nenabled = true\n").unwrap();
        assert_eq!(new, "# Mine\nversion = 2\n\n[vim]\nenabled = true\n");
    }

    #[test]
    fn a_version_that_makes_no_sense_or_is_too_new_is_left_alone_and_reported() {
        for (text, expected) in [
            ("version = \"two\"\n", "has to be a whole number"),
            ("version = 0\n", "has to be a whole number"),
            ("[version]\nx = 1\n", "has to be a whole number"),
            ("version = 3\n", "from a newer tectr"),
        ] {
            let document = text.parse::<DocumentMut>().unwrap();
            assert_eq!(upgraded(&document), None, "{text}");
            let said = problem(&document).unwrap_or_default();
            assert!(said.contains(expected), "{text}: {said}");
        }
        let current = stamped("").parse::<DocumentMut>().unwrap();
        assert_eq!(problem(&current), None);
    }
}
