//! The `[keybinds]` section: every action the app has a key for, and how the file
//! changes them.
//!
//! This table is the only place defaults live. The UI asks for the resolved result,
//! and `apps/dsk/src/keybinds.ts` lists the same IDs by hand, which a test checks.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use toml_edit::{Array, InlineTable, Item, Table, Value};

pub const GROUPS: &[Group] = &[
    Group {
        id: "nav",
        title: "Navigation",
        about: "The shared default for moving around. Changing one here changes every action that follows it.",
    },
    Group {
        id: "global",
        title: "Everywhere",
        about: "Work anywhere in the app, unless a sheet is open.",
    },
    Group {
        id: "sidebar",
        title: "Sidebar",
        about: "While the sidebar has the keyboard.",
    },
    Group {
        id: "pane",
        title: "Agent pane",
        about: "While the main pane has the keyboard.",
    },
    Group {
        id: "composer",
        title: "Message box",
        about: "While typing a message to an agent.",
    },
    Group {
        id: "permission",
        title: "Permission requests",
        about: "While an agent is asking. A key without cmd, ctrl or alt never answers while you're typing.",
    },
    Group {
        id: "new_tab",
        title: "New tab",
        about: "While a new tab asks what to open.",
    },
    Group {
        id: "new_topic",
        title: "Starting an agent",
        about: "On the new topic screen, and in a new tab once you've chosen an agent.",
    },
    Group {
        id: "sheet",
        title: "Sheets",
        about: "In the sheets for adding a workspace, renaming and deleting.",
    },
    Group {
        id: "palette",
        title: "Command palette",
        about: "While the palette is open.",
    },
    Group {
        id: "picker",
        title: "Pickers",
        about: "While a dropdown is open.",
    },
    Group {
        id: "device",
        title: "Device settings",
        about: "In a device's providers settings.",
    },
    Group {
        id: "mcp",
        title: "MCP servers",
        about: "In a device's MCP servers, and in the sheet with a topic's MCP servers.",
    },
    Group {
        id: "mcp_form",
        title: "Editing an MCP server",
        about: "In the form for adding or editing an MCP server. Enter in a box of lines adds a line.",
    },
    Group {
        id: "sections",
        title: "Settings sections",
        about: "While the list of sections in settings has the keyboard.",
    },
    Group {
        id: "settings",
        title: "Settings",
        about: "While a section of settings has the keyboard.",
    },
    Group {
        id: "recorder",
        title: "Recording a shortcut",
        about: "After pressing a new shortcut in settings.",
    },
];

// Moving through a list you search by typing, where plain letters have to go to the search.
// Ctrl+J and Ctrl+K pair with J and K, but `mod` is Ctrl outside macOS, and there Ctrl+K
// is what opens and closes the palette, so there it's only the arrows.
const INPUT_UP: &[&str] = if cfg!(target_os = "macos") {
    &["ctrl+k", "up"]
} else {
    &["up"]
};
const INPUT_DOWN: &[&str] = if cfg!(target_os = "macos") {
    &["ctrl+j", "down"]
} else {
    &["down"]
};

// Ctrl+C is copy outside macOS. It comes first because the first key is the one the UI shows.
const INTERRUPT: &[&str] = if cfg!(target_os = "macos") {
    &["ctrl+c", "mod+."]
} else {
    &["mod+."]
};

// Outside macOS `mod` is Ctrl, so Ctrl+W on its own already closes a tab there and can't
// also start a sequence.
const TOGGLE_SIDEBAR: &[&str] = if cfg!(target_os = "macos") {
    &["mod+0", "ctrl+w w"]
} else {
    &["mod+0"]
};
const CLOSE_TAB: &[&str] = if cfg!(target_os = "macos") {
    &["mod+w", "ctrl+w q"]
} else {
    &["mod+w"]
};

const fn once(id: &'static str, label: &'static str, defaults: &'static [&'static str]) -> Action {
    Action {
        id,
        label,
        defaults,
        inherits: None,
        repeat: false,
    }
}

/// Keeps firing while the key is held, which is what moving through things wants.
const fn held(id: &'static str, label: &'static str, defaults: &'static [&'static str]) -> Action {
    Action {
        id,
        label,
        defaults,
        inherits: None,
        repeat: true,
    }
}

const fn follows(id: &'static str, label: &'static str, nav: &'static str) -> Action {
    Action {
        id,
        label,
        defaults: &[],
        inherits: Some(nav),
        repeat: false,
    }
}

/// Vim's keys wherever an action means the same thing in Vim. What Vim has no word for
/// keeps its ⌘ key, and the ⌘ and arrow keys stay alongside the Vim ones.
pub const ACTIONS: &[Action] = &[
    held("nav.up", "Move up", &["k", "up"]),
    held("nav.down", "Move down", &["j", "down"]),
    held(
        "nav.input_up",
        "Move up in a list you search by typing",
        INPUT_UP,
    ),
    held(
        "nav.input_down",
        "Move down in a list you search by typing",
        INPUT_DOWN,
    ),
    held("nav.left", "Move left", &["h", "left"]),
    once("nav.right", "Move right", &["l", "right"]),
    once(
        "nav.input_first",
        "Go to the first item in a list you search by typing",
        &["home"],
    ),
    once(
        "nav.input_last",
        "Go to the last item in a list you search by typing",
        &["end"],
    ),
    once("nav.first", "Go to the first item", &["g g", "home"]),
    once("nav.last", "Go to the last item", &["shift+g", "end"]),
    held("nav.half_up", "Move up half a screen", &["ctrl+u"]),
    held("nav.half_down", "Move down half a screen", &["ctrl+d"]),
    held("nav.page_up", "Move up a screen", &["ctrl+b", "pageup"]),
    held(
        "nav.page_down",
        "Move down a screen",
        &["ctrl+f", "pagedown"],
    ),
    once("nav.center", "Center the current item", &["z z"]),
    once("nav.confirm", "Confirm", &["enter"]),
    once("nav.back", "Go back", &["escape"]),
    once(
        "global.palette",
        "Open the command palette",
        &["mod+k", ":"],
    ),
    once("global.settings", "Open settings", &["mod+,"]),
    once("global.new_topic", "New topic", &["mod+n"]),
    once("global.new_workspace", "Add a workspace", &["mod+shift+n"]),
    once("global.new_tab", "New tab", &["mod+t"]),
    once("global.next_request", "Next permission request", &["mod+j"]),
    once("global.interrupt", "Interrupt the agent", INTERRUPT),
    once("global.close_tab", "Close the tab", CLOSE_TAB),
    once(
        "global.toggle_sidebar",
        "Move between the sidebar and the pane",
        TOGGLE_SIDEBAR,
    ),
    once("global.tab_1", "Go to tab 1", &["mod+1"]),
    once("global.tab_2", "Go to tab 2", &["mod+2"]),
    once("global.tab_3", "Go to tab 3", &["mod+3"]),
    once("global.tab_4", "Go to tab 4", &["mod+4"]),
    once("global.tab_5", "Go to tab 5", &["mod+5"]),
    once("global.tab_6", "Go to tab 6", &["mod+6"]),
    once("global.tab_7", "Go to tab 7", &["mod+7"]),
    once("global.tab_8", "Go to tab 8", &["mod+8"]),
    once("global.tab_9", "Go to tab 9", &["mod+9"]),
    held("global.prev_tab", "Previous tab", &["mod+[", "g shift+t"]),
    held("global.next_tab", "Next tab", &["mod+]", "g t"]),
    held("global.prev_topic", "Previous topic", &["alt+k", "alt+up"]),
    held("global.next_topic", "Next topic", &["alt+j", "alt+down"]),
    held(
        "global.prev_workspace",
        "Previous workspace",
        &["alt+shift+k", "alt+shift+up"],
    ),
    held(
        "global.next_workspace",
        "Next workspace",
        &["alt+shift+j", "alt+shift+down"],
    ),
    once(
        "global.mark_all_read",
        "Mark every topic read",
        &["mod+shift+u"],
    ),
    once(
        "global.topic_mcp",
        "Choose the topic's MCP servers",
        &["mod+shift+m"],
    ),
    once("global.toggle_vim", "Turn vim keys on or off", &[]),
    once("global.move_topic", "Move the topic to another folder", &[]),
    once("global.archived_topics", "Restore an archived topic", &[]),
    follows("sidebar.up", "Move up", "nav.up"),
    follows("sidebar.down", "Move down", "nav.down"),
    follows("sidebar.first", "Go to the first row", "nav.first"),
    follows("sidebar.last", "Go to the last row", "nav.last"),
    follows("sidebar.half_up", "Move up half a screen", "nav.half_up"),
    follows(
        "sidebar.half_down",
        "Move down half a screen",
        "nav.half_down",
    ),
    follows("sidebar.center", "Center the current row", "nav.center"),
    follows("sidebar.parent", "Go to the parent row", "nav.left"),
    follows("sidebar.open", "Go into the row", "nav.right"),
    follows("sidebar.confirm", "Open the row", "nav.confirm"),
    once(
        "sidebar.add",
        "Add to the pointed or focused row",
        &["a", "o"],
    ),
    once(
        "sidebar.rename",
        "Rename the pointed or focused topic",
        &["r"],
    ),
    once(
        "sidebar.archive",
        "Archive the pointed or focused topic",
        &["d d", "backspace"],
    ),
    once(
        "sidebar.toggle_unread",
        "Mark the pointed or focused topic read or unread",
        &["u"],
    ),
    follows("pane.up", "Scroll up", "nav.up"),
    follows("pane.down", "Scroll down", "nav.down"),
    follows("pane.first", "Scroll to the top", "nav.first"),
    follows("pane.last", "Scroll to the bottom", "nav.last"),
    follows("pane.half_up", "Scroll up half a screen", "nav.half_up"),
    follows(
        "pane.half_down",
        "Scroll down half a screen",
        "nav.half_down",
    ),
    follows("pane.page_up", "Scroll up a screen", "nav.page_up"),
    follows("pane.page_down", "Scroll down a screen", "nav.page_down"),
    once("pane.compose", "Write a message", &["i", "/", "enter"]),
    once("pane.toggle_steps", "Fold or show tool calls", &["o"]),
    once(
        "pane.toggle_turn",
        "Fold or show what the agent is doing now",
        &["shift+o"],
    ),
    follows("pane.back", "Go to the sidebar", "nav.back"),
    once("pane.next_request", "Next permission request", &["tab"]),
    once("pane.pick_model", "Choose the model", &["mod+l"]),
    once("pane.pick_effort", "Choose the effort", &["mod+e"]),
    once("pane.pick_mode", "Choose the permission mode", &["mod+u"]),
    once("composer.send", "Send", &["enter"]),
    once("composer.newline", "New line", &["shift+enter"]),
    follows("composer.blur", "Leave the message box", "nav.back"),
    once("permission.allow_once", "Allow once", &["y"]),
    once("permission.allow_always", "Always allow", &["a"]),
    once("permission.reject", "Deny", &["n"]),
    once("permission.option_1", "Choose option 1", &["1"]),
    once("permission.option_2", "Choose option 2", &["2"]),
    once("permission.option_3", "Choose option 3", &["3"]),
    once("permission.option_4", "Choose option 4", &["4"]),
    once("permission.option_5", "Choose option 5", &["5"]),
    once("permission.option_6", "Choose option 6", &["6"]),
    once("permission.option_7", "Choose option 7", &["7"]),
    once("permission.option_8", "Choose option 8", &["8"]),
    once("permission.option_9", "Choose option 9", &["9"]),
    follows("new_tab.up", "Move up", "nav.up"),
    follows("new_tab.down", "Move down", "nav.down"),
    follows("new_tab.confirm", "Open the chosen kind", "nav.confirm"),
    once("new_tab.agent", "Start an agent", &["1", "a"]),
    once("new_tab.terminal", "Open a terminal", &["2", "t"]),
    once("new_tab.diff", "Open a diff", &["3", "d"]),
    once("new_topic.start", "Start", &["enter"]),
    once("new_topic.newline", "New line", &["shift+enter"]),
    follows("new_topic.cancel", "Cancel", "nav.back"),
    once(
        "new_topic.pick_workspace",
        "Choose the workspace",
        &["mod+o"],
    ),
    once("new_topic.pick_place", "Choose where to work", &["mod+b"]),
    once("new_topic.pick_provider", "Choose the provider", &["mod+p"]),
    once("new_topic.pick_model", "Choose the model", &["mod+l"]),
    once("new_topic.pick_effort", "Choose the effort", &["mod+e"]),
    once(
        "new_topic.pick_mode",
        "Choose the permission mode",
        &["mod+u"],
    ),
    follows("sheet.submit", "Submit", "nav.confirm"),
    follows("sheet.cancel", "Cancel", "nav.back"),
    follows("sheet.up", "Move up the suggestions", "nav.input_up"),
    follows("sheet.down", "Move down the suggestions", "nav.input_down"),
    once("sheet.complete", "Complete the path", &["tab"]),
    follows("palette.up", "Move up", "nav.input_up"),
    follows("palette.down", "Move down", "nav.input_down"),
    follows("palette.confirm", "Run the command", "nav.confirm"),
    follows("palette.cancel", "Close", "nav.back"),
    follows("picker.up", "Move up", "nav.input_up"),
    follows("picker.down", "Move down", "nav.input_down"),
    follows("picker.first", "Go to the first choice", "nav.input_first"),
    follows("picker.last", "Go to the last choice", "nav.input_last"),
    follows("picker.confirm", "Choose", "nav.confirm"),
    follows("picker.cancel", "Close", "nav.back"),
    follows("device.up", "Move up", "nav.up"),
    follows("device.down", "Move down", "nav.down"),
    follows("device.first", "Go to the first provider", "nav.first"),
    follows("device.last", "Go to the last provider", "nav.last"),
    follows("device.center", "Center the current provider", "nav.center"),
    follows(
        "device.confirm",
        "Add or refresh the provider",
        "nav.confirm",
    ),
    follows("device.back", "Back to the sections", "nav.back"),
    once(
        "device.remove",
        "Remove the provider",
        &["d d", "backspace"],
    ),
    once("device.pick_model", "Choose the default model", &["m"]),
    once("device.pick_effort", "Choose the default effort", &["e"]),
    once("device.rename", "Rename the device", &["r"]),
    once(
        "device.toggle_keep_awake",
        "Keep the device awake while agents work, or not",
        &["w"],
    ),
    follows("mcp.up", "Move up", "nav.up"),
    follows("mcp.down", "Move down", "nav.down"),
    follows("mcp.first", "Go to the first server", "nav.first"),
    follows("mcp.last", "Go to the last server", "nav.last"),
    follows("mcp.center", "Center the current server", "nav.center"),
    follows("mcp.edit", "Edit the server", "nav.confirm"),
    follows(
        "mcp.back",
        "Back to the sections, or close the sheet",
        "nav.back",
    ),
    once("mcp.add", "Add a server", &["a", "o"]),
    once("mcp.remove", "Remove the server", &["d d", "backspace"]),
    once(
        "mcp.toggle",
        "Turn the server on or off, for topics by default or for this topic",
        &["t", "space"],
    ),
    once(
        "mcp.turn_on",
        "Turn a device's server on for this topic",
        &["y"],
    ),
    once(
        "mcp.turn_off",
        "Turn a device's server off for this topic",
        &["n"],
    ),
    once(
        "mcp.follow_default",
        "Let this topic follow a device's server's default",
        &["u"],
    ),
    once("mcp.login", "Log in to the server", &["l"]),
    once("mcp.logout", "Log out of the server", &["shift+l"]),
    once(
        "mcp.import",
        "Import the servers of Claude Code, Codex and OpenCode",
        &["i"],
    ),
    once("mcp_form.save", "Save, from any field", &["mod+enter"]),
    once(
        "mcp_form.switch_type",
        "Switch between stdio and HTTP",
        &["mod+t"],
    ),
    once(
        "mcp_form.toggle_enabled",
        "Turn the server on or off",
        &["mod+e"],
    ),
    follows("sections.up", "Previous section", "nav.up"),
    follows("sections.down", "Next section", "nav.down"),
    follows("sections.first", "First section", "nav.first"),
    follows("sections.last", "Last section", "nav.last"),
    follows("sections.open", "Go into the section", "nav.right"),
    follows("sections.confirm", "Go into the section", "nav.confirm"),
    follows("sections.close", "Close settings", "nav.back"),
    follows("settings.up", "Move up", "nav.up"),
    follows("settings.down", "Move down", "nav.down"),
    follows("settings.first", "Go to the first row", "nav.first"),
    follows("settings.last", "Go to the last row", "nav.last"),
    follows("settings.half_up", "Move up half a screen", "nav.half_up"),
    follows(
        "settings.half_down",
        "Move down half a screen",
        "nav.half_down",
    ),
    follows("settings.center", "Center the current row", "nav.center"),
    follows(
        "settings.edit",
        "Change the selected setting",
        "nav.confirm",
    ),
    follows("settings.back", "Back to the sections", "nav.back"),
    follows("settings.parent", "Back to the sections", "nav.left"),
    once("settings.add", "Add another shortcut", &["a"]),
    once(
        "settings.unbind",
        "Remove the shortcut",
        &["d d", "backspace"],
    ),
    once("settings.reset", "Reset to the default", &["u", "r"]),
    held(
        "settings.decrease",
        "Lower the value, or the previous choice",
        &["-"],
    ),
    held(
        "settings.increase",
        "Raise the value, or the next choice",
        &["=", "plus"],
    ),
    once(
        "settings.toggle_hold",
        "Switch holding on or off",
        &["shift+h"],
    ),
    once(
        "settings.toggle_repeat",
        "Switch repeating on or off",
        &["shift+r"],
    ),
    once("recorder.save", "Save", &["enter"]),
    once("recorder.cancel", "Cancel", &["escape"]),
];

/// Pairs of contexts that are live at the same time. Everything not listed is only
/// checked against itself: a sheet, palette or picker takes the keyboard from the rest.
const SHARED_WITH_GLOBAL: &[&str] = &[
    "sidebar",
    "pane",
    "composer",
    "permission",
    "new_tab",
    "new_topic",
    "device",
    "mcp",
    "sections",
    "settings",
];
const SHARED: &[(&str, &str)] = &[("pane", "permission"), ("sheet", "mcp_form")];

/// macOS handles these through the app menu before the webview sees them.
const MACOS_MENU_KEYS: &[&str] = &[
    "mod+q",
    "mod+h",
    "mod+alt+h",
    "mod+m",
    "mod+z",
    "mod+shift+z",
    "mod+x",
    "mod+c",
    "mod+v",
    "mod+a",
    "mod+ctrl+f",
];

const NAMED_KEYS: &[&str] = &[
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "enter",
    "escape",
    "tab",
    "space",
    "backspace",
    "delete",
    "plus",
];

/// The most keys a sequence can have.
const MAX_SEQUENCE: usize = 4;

/// How long a sequence waits for its next key, the same as Vim's `timeoutlen`.
pub const DEFAULT_SEQUENCE_TIMEOUT_MS: u32 = 1_000;
const SEQUENCE_TIMEOUT_KEY: &str = "sequence_timeout_ms";

/// The groups whose actions run while typing, as long as the keys don't type anything.
const TEXT_GROUPS: &[&str] = &["composer", "new_topic", "sheet", "palette", "mcp_form"];

/// The longest a key can be required to be held. Long enough for anyone, and short
/// enough that a typo can't leave an action unreachable.
const MAX_HOLD_MS: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Group {
    pub id: &'static str,
    pub title: &'static str,
    pub about: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Action {
    pub id: &'static str,
    pub label: &'static str,
    defaults: &'static [&'static str],
    inherits: Option<&'static str>,
    repeat: bool,
}

/// One key for an action, and how it has to be pressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub key: String,
    /// How long it has to be held before the action runs. 0 runs it on press.
    pub hold_ms: u32,
    /// Whether keeping it pressed runs the action again and again.
    pub repeat: bool,
}

/// An action with the keys that are in effect for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedAction {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub bindings: Vec<Binding>,
    /// The file sets this action, so it can be reset.
    pub customized: bool,
    /// The nav action it follows while the file doesn't set it.
    pub inherits: Option<&'static str>,
    /// What keys a new binding starts with for repeating.
    pub repeat: bool,
}

pub fn find(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|action| action.id == id)
}

/// `repeat` for a key that doesn't say. Inherited actions move the same way as what
/// they follow.
fn default_repeat(action: &Action) -> bool {
    action
        .inherits
        .and_then(find)
        .map_or(action.repeat, |nav| nav.repeat)
}

fn default_bindings(action: &Action) -> Vec<Binding> {
    let repeat = default_repeat(action);
    action
        .defaults
        .iter()
        .map(|key| Binding {
            key: (*key).to_owned(),
            hold_ms: 0,
            repeat,
        })
        .collect()
}

/// Turns what someone wrote into the one form the UI matches against:
/// modifiers in a fixed order, then the key, in lower case.
///
/// A sequence is keys separated by spaces: `"g g"`.
pub fn canonical_key(input: &str) -> Result<String, String> {
    let steps: Vec<&str> = input.split_whitespace().collect();
    if steps.is_empty() {
        return Err(format!("\"{input}\" doesn't say which key"));
    }
    if steps.len() > MAX_SEQUENCE {
        return Err(format!(
            "\"{input}\" is too long: a sequence has at most {MAX_SEQUENCE} keys"
        ));
    }
    let steps = steps
        .into_iter()
        .map(|step| canonical_step(step, input))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(steps.join(" "))
}

fn canonical_step(step: &str, input: &str) -> Result<String, String> {
    // Vim's notation: a capital letter on its own is that letter with Shift. With a
    // modifier it's just how someone wrote the letter, like "Cmd+K".
    let is_capital = step.len() == 1 && step.chars().all(|c| c.is_ascii_uppercase());
    let text = if is_capital {
        format!("shift+{}", step.to_lowercase())
    } else {
        step.to_lowercase()
    };
    let (modifiers, key) = if text.ends_with('+') {
        // A trailing + is the plus key itself: "mod++".
        (text[..text.len() - 1].trim_end_matches('+'), "plus")
    } else {
        match text.rsplit_once('+') {
            Some((modifiers, key)) => (modifiers, key),
            None => ("", text.as_str()),
        }
    };

    let mut seen = [false; 5];
    for name in modifiers.split('+').filter(|name| !name.is_empty()) {
        let slot = match name.trim() {
            "mod" => 0,
            "cmd" | "command" | "meta" | "super" => 1,
            "ctrl" | "control" => 2,
            "alt" | "option" | "opt" => 3,
            "shift" => 4,
            other => return Err(format!("\"{input}\" has an unknown modifier \"{other}\"")),
        };
        if std::mem::replace(&mut seen[slot], true) {
            return Err(format!("\"{input}\" has a modifier twice"));
        }
    }

    let key = match key.trim() {
        "esc" => "escape",
        "return" => "enter",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        "del" => "delete",
        "spacebar" => "space",
        other => other,
    };
    let is_function_key = key
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=24).contains(&number));
    let is_character = key.chars().count() == 1;
    if key.is_empty() {
        return Err(format!("\"{input}\" doesn't say which key"));
    }
    if [
        "mod", "cmd", "command", "ctrl", "control", "alt", "option", "shift",
    ]
    .contains(&key)
    {
        return Err(format!("\"{input}\" has modifiers but no key"));
    }
    if !is_character && !is_function_key && !NAMED_KEYS.contains(&key) {
        return Err(format!(
            "\"{input}\" uses an unknown key \"{key}\". A sequence needs spaces, like \"g g\""
        ));
    }

    let mut parts: Vec<&str> = ["mod", "cmd", "ctrl", "alt", "shift"]
        .into_iter()
        .zip(seen)
        .filter_map(|(name, on)| on.then_some(name))
        .collect();
    parts.push(key);
    Ok(parts.join("+"))
}

/// The form two keys are compared in, where `mod` and what it means here are the same.
fn effective_key(key: &str) -> String {
    key.split(' ')
        .map(effective_step)
        .collect::<Vec<_>>()
        .join(" ")
}

fn effective_step(key: &str) -> String {
    let native = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    let mut parts: Vec<&str> = key.split('+').collect();
    let last = parts.pop().unwrap_or_default();
    let has_mod = parts.contains(&"mod") || parts.contains(&native);
    let mut out: Vec<&str> = ["mod", "cmd", "ctrl", "alt", "shift"]
        .into_iter()
        .filter(|name| {
            if *name == "mod" {
                has_mod
            } else {
                *name != native && parts.contains(name)
            }
        })
        .collect();
    out.push(last);
    out.join("+")
}

fn binding_of(value: &Value, default_repeat: bool, where_: &str) -> Result<Binding, String> {
    if let Some(key) = value.as_str() {
        return Ok(Binding {
            key: canonical_key(key)?,
            hold_ms: 0,
            repeat: default_repeat,
        });
    }
    let Some(table) = value.as_inline_table() else {
        return Err(format!(
            "{where_}: a shortcut is text like \"mod+k\", or {{ key = \"mod+k\", hold_ms = 500, repeat = true }}"
        ));
    };
    binding_of_table(table, default_repeat, where_)
}

fn binding_of_table(
    table: &dyn toml_edit::TableLike,
    default_repeat: bool,
    where_: &str,
) -> Result<Binding, String> {
    let mut key = None;
    let mut hold_ms = 0;
    let mut repeat = default_repeat;
    for (name, item) in table.iter() {
        let value = item.as_value();
        match name {
            "key" => {
                let text = value
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("{where_}: `key` has to be text"))?;
                key = Some(canonical_key(text)?);
            }
            "hold_ms" => {
                let ms = value
                    .and_then(Value::as_integer)
                    .and_then(|ms| u32::try_from(ms).ok())
                    .filter(|ms| *ms <= MAX_HOLD_MS)
                    .ok_or_else(|| {
                        format!("{where_}: `hold_ms` has to be from 0 to {MAX_HOLD_MS}")
                    })?;
                hold_ms = ms;
            }
            "repeat" => {
                repeat = value
                    .and_then(Value::as_bool)
                    .ok_or_else(|| format!("{where_}: `repeat` has to be true or false"))?;
            }
            other => return Err(format!("{where_}: unknown option `{other}`")),
        }
    }
    let key = key.ok_or_else(|| format!("{where_}: `key` is missing"))?;
    Ok(Binding {
        key,
        hold_ms,
        repeat,
    })
}

fn bindings_of(item: &Item, action: &Action) -> Result<Vec<Binding>, String> {
    let repeat = default_repeat(action);
    let id = action.id;
    if let Some(array) = item.as_array() {
        return array
            .iter()
            .map(|value| binding_of(value, repeat, id))
            .collect();
    }
    if let Some(table) = item.as_table_like() {
        return Ok(vec![binding_of_table(table, repeat, id)?]);
    }
    match item.as_value() {
        Some(value) => Ok(vec![binding_of(value, repeat, id)?]),
        None => Err(format!("{id}: this isn't a shortcut")),
    }
}

/// Reads `[keybinds]`. A bad entry is reported and left out, so its action keeps working
/// with its default.
pub fn parse(section: &Item, problems: &mut Vec<String>) -> HashMap<&'static str, Vec<Binding>> {
    let mut overrides = HashMap::new();
    let Some(contexts) = section.as_table_like() else {
        problems.push("keybinds has to be a section, like [keybinds.global]".to_owned());
        return overrides;
    };
    for (context, item) in contexts.iter() {
        if context == SEQUENCE_TIMEOUT_KEY {
            continue;
        }
        let Some(actions) = item.as_table_like() else {
            problems.push(format!("keybinds.{context} has to be a section"));
            continue;
        };
        if !GROUPS.iter().any(|group| group.id == context) {
            problems.push(format!(
                "keybinds.{context} isn't a known section, so it's ignored"
            ));
            continue;
        }
        for (name, item) in actions.iter() {
            let id = format!("{context}.{name}");
            let Some(action) = find(&id) else {
                problems.push(format!("{id} isn't a known action, so it's ignored"));
                continue;
            };
            match bindings_of(item, action) {
                Ok(bindings) => {
                    overrides.insert(action.id, bindings);
                }
                Err(problem) => problems.push(format!("{problem}. Using the default.")),
            }
        }
    }
    overrides
}

/// Reads how long a sequence waits, when the file says.
pub fn parse_timeout(section: &Item, problems: &mut Vec<String>) -> Option<u32> {
    let item = section.as_table_like()?.get(SEQUENCE_TIMEOUT_KEY)?;
    let ms = item
        .as_integer()
        .and_then(|ms| u32::try_from(ms).ok())
        .filter(|ms| (100..=MAX_HOLD_MS).contains(ms));
    if ms.is_none() {
        problems.push(format!(
            "keybinds.{SEQUENCE_TIMEOUT_KEY} has to be from 100 to {MAX_HOLD_MS}. Using the default."
        ));
    }
    ms
}

/// Applies the file on top of the defaults, including what follows `nav`.
pub fn resolve(overrides: &HashMap<&'static str, Vec<Binding>>) -> Vec<ResolvedAction> {
    let bindings_for = |action: &Action| -> Vec<Binding> {
        overrides
            .get(action.id)
            .cloned()
            .unwrap_or_else(|| match action.inherits.and_then(find) {
                Some(nav) => overrides
                    .get(nav.id)
                    .cloned()
                    .unwrap_or_else(|| default_bindings(nav)),
                None => default_bindings(action),
            })
    };
    ACTIONS
        .iter()
        .map(|action| ResolvedAction {
            id: action.id,
            group: action.id.split('.').next().unwrap_or_default(),
            label: action.label,
            bindings: bindings_for(action),
            customized: overrides.contains_key(action.id),
            inherits: action.inherits,
            repeat: default_repeat(action),
        })
        .collect()
}

/// The shorter and longer of two keys, when one is how the other starts.
fn prefix_pair<'a>(a: &'a String, b: &'a String) -> Option<(&'a String, &'a String)> {
    if b.starts_with(&format!("{a} ")) {
        Some((a, b))
    } else if a.starts_with(&format!("{b} ")) {
        Some((b, a))
    } else {
        None
    }
}

/// Pressing this in a text box would type, rather than be available as a shortcut.
fn types_a_character(step: &str) -> bool {
    let (modifiers, key) = step.rsplit_once('+').unwrap_or(("", step));
    let is_function_key = key.starts_with('f') && key.len() > 1;
    !is_function_key
        && !["mod", "cmd", "ctrl", "alt"]
            .iter()
            .any(|m| modifiers.contains(m))
}

fn share_keyboard(a: &str, b: &str) -> bool {
    a == b
        || (a == "global" && SHARED_WITH_GLOBAL.contains(&b))
        || (b == "global" && SHARED_WITH_GLOBAL.contains(&a))
        || SHARED.contains(&(a, b))
        || SHARED.contains(&(b, a))
}

/// Keys that can't do what they say. These are warnings: the file is still applied, and
/// the first action checked wins.
pub fn warnings(resolved: &[ResolvedAction]) -> Vec<String> {
    let mut warnings = Vec::new();
    // `nav` only provides defaults, so its keys don't do anything on their own.
    let live: Vec<&ResolvedAction> = resolved.iter().filter(|a| a.group != "nav").collect();
    for (index, first) in live.iter().enumerate() {
        let keys: HashSet<String> = first
            .bindings
            .iter()
            .map(|binding| effective_key(&binding.key))
            .collect();
        for second in &live[index + 1..] {
            if !share_keyboard(first.group, second.group) {
                continue;
            }
            for binding in &second.bindings {
                let key = effective_key(&binding.key);
                if keys.contains(&key) {
                    warnings.push(format!(
                        "{} is on both {} and {}",
                        binding.key, first.id, second.id
                    ));
                }
                for other in &keys {
                    if let Some((short, long)) = prefix_pair(other, &key) {
                        warnings.push(format!(
                            "{short} on {} starts {long} on {}, so the longer one never fires",
                            if short == other { first.id } else { second.id },
                            if short == other { second.id } else { first.id },
                        ));
                    }
                }
            }
        }
    }
    for action in live.iter().filter(|a| TEXT_GROUPS.contains(&a.group)) {
        for binding in &action.bindings {
            if binding.key.contains(' ') && binding.key.split(' ').any(types_a_character) {
                warnings.push(format!(
                    "{} on {} never fires, because a sequence can't take keys from a text box. Give each key cmd, ctrl or alt",
                    binding.key, action.id
                ));
            }
        }
    }
    // The palette and pickers take the keyboard from every global key but the palette's own.
    let toggle: HashSet<String> = resolved
        .iter()
        .filter(|action| action.id == "global.palette")
        .flat_map(|action| &action.bindings)
        .map(|binding| effective_key(&binding.key))
        .collect();
    for action in live
        .iter()
        .filter(|action| action.group == "palette" || action.group == "picker")
    {
        for binding in &action.bindings {
            if toggle.contains(&effective_key(&binding.key)) {
                warnings.push(format!(
                    "{} is on both global.palette and {}, and opening or closing the palette wins",
                    binding.key, action.id
                ));
            }
        }
    }
    if cfg!(target_os = "macos") {
        for action in &live {
            for binding in &action.bindings {
                let effective = effective_key(&binding.key);
                if effective
                    .split(' ')
                    .any(|step| MACOS_MENU_KEYS.contains(&step))
                {
                    warnings.push(format!(
                        "{} on {} may never fire, because the macOS app menu uses it",
                        binding.key, action.id
                    ));
                }
            }
        }
    }
    warnings
}

/// What the UI writes back. The short form when nothing differs from the action's
/// default, so the file stays pleasant to read.
pub fn value_of(action: &Action, bindings: &[Binding]) -> Value {
    let repeat = default_repeat(action);
    let simple = bindings
        .iter()
        .all(|binding| binding.hold_ms == 0 && binding.repeat == repeat);
    let one = |binding: &Binding| -> Value {
        if simple {
            return Value::from(binding.key.as_str());
        }
        let mut table = InlineTable::new();
        table.insert("key", Value::from(binding.key.as_str()));
        if binding.hold_ms != 0 {
            table.insert("hold_ms", Value::from(i64::from(binding.hold_ms)));
        }
        if binding.repeat != repeat {
            table.insert("repeat", Value::from(binding.repeat));
        }
        Value::InlineTable(table)
    };
    if let [only] = bindings {
        return one(only);
    }
    let mut array = Array::new();
    for binding in bindings {
        array.push(one(binding));
    }
    Value::Array(array)
}

/// The `[keybinds.*]` tables of the reference file.
pub fn reference() -> String {
    let mut out = format!(
        "[keybinds]\n# How long a sequence waits for its next key, like Vim's timeoutlen.\n{SEQUENCE_TIMEOUT_KEY} = {DEFAULT_SEQUENCE_TIMEOUT_MS}\n\n"
    );
    for group in GROUPS {
        out.push_str(&format!("# {}. {}\n", group.title, group.about));
        out.push_str(&format!("[keybinds.{}]\n", group.id));
        for action in ACTIONS
            .iter()
            .filter(|action| action.id.split('.').next() == Some(group.id))
        {
            let name = action
                .id
                .split_once('.')
                .map_or(action.id, |(_, name)| name);
            match action.inherits.and_then(find) {
                Some(nav) => {
                    let keys = quoted(nav.defaults);
                    out.push_str(&format!(
                        "# {name} = {keys} # {}. Follows {}\n",
                        action.label, nav.id
                    ));
                }
                None => {
                    let keys = quoted(action.defaults);
                    let repeats = if action.repeat {
                        ", repeats when held"
                    } else {
                        ""
                    };
                    out.push_str(&format!("{name} = {keys} # {}{repeats}\n", action.label));
                }
            }
        }
        out.push('\n');
    }
    out
}

fn quoted(keys: &[&str]) -> String {
    let keys: Vec<String> = keys.iter().map(|key| format!("\"{key}\"")).collect();
    match keys.as_slice() {
        [only] => only.clone(),
        many => format!("[{}]", many.join(", ")),
    }
}

/// Gets the table at `name` in `parent`, making it if needed, without a header of its own
/// when only its children have keys.
pub fn subtable<'a>(parent: &'a mut Table, name: &str) -> Result<&'a mut Table, String> {
    parent
        .entry(name)
        .or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        })
        .as_table_mut()
        .ok_or_else(|| format!("{name} isn't written as a section, so it can't be edited here"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_normalize_to_one_form() {
        for (written, expected) in [
            ("mod+k", "mod+k"),
            ("Shift+CMD+K", "cmd+shift+k"),
            ("option+up", "alt+up"),
            ("Esc", "escape"),
            ("return", "enter"),
            ("ArrowDown", "down"),
            ("mod+,", "mod+,"),
            ("mod+[", "mod+["),
            ("/", "/"),
            ("mod++", "mod+plus"),
            ("+", "plus"),
            ("f12", "f12"),
            ("mod+K", "mod+k"),
            ("G", "shift+g"),
            ("g g", "g g"),
            ("  z   z ", "z z"),
            ("g T", "g shift+t"),
            ("ctrl+w W", "ctrl+w shift+w"),
            ("CTRL+W esc", "ctrl+w escape"),
        ] {
            assert_eq!(canonical_key(written).as_deref(), Ok(expected), "{written}");
        }
    }

    #[test]
    fn bad_keys_say_what_is_wrong() {
        for (written, expected) in [
            ("", "which key"),
            ("mod+", "plus"),
            ("hyper+k", "unknown modifier"),
            ("mod+mod+k", "twice"),
            ("mod+shift", "no key"),
            ("nope", "unknown key"),
            ("f99", "unknown key"),
            ("gg", "needs spaces"),
            ("g g g g g", "at most"),
            ("shift + enter", "no key"),
        ] {
            match canonical_key(written) {
                // A trailing + means the plus key, so "mod+" is valid.
                Ok(key) => assert!(key.contains(expected), "{written} became {key}"),
                Err(problem) => assert!(problem.contains(expected), "{written}: {problem}"),
            }
        }
    }

    #[test]
    fn every_default_is_valid_and_canonical() {
        for action in ACTIONS {
            for key in action.defaults {
                assert_eq!(canonical_key(key).as_deref(), Ok(*key), "{}", action.id);
            }
        }
    }

    #[test]
    fn every_action_is_in_a_known_group_and_follows_only_nav() {
        let mut seen = HashSet::new();
        for action in ACTIONS {
            assert!(seen.insert(action.id), "{} is listed twice", action.id);
            let (group, _) = action.id.split_once('.').expect("ids are group.name");
            assert!(GROUPS.iter().any(|g| g.id == group), "{}", action.id);
            if let Some(nav) = action.inherits {
                assert!(nav.starts_with("nav."), "{} follows {nav}", action.id);
                assert!(find(nav).is_some(), "{} follows {nav}", action.id);
                assert!(action.defaults.is_empty(), "{}", action.id);
            }
        }
    }

    #[test]
    fn the_defaults_have_no_conflicts() {
        assert_eq!(warnings(&resolve(&HashMap::new())), Vec::<String>::new());
    }

    /// The webview can't share this table, so its list of IDs is checked against it.
    #[test]
    fn the_webview_lists_the_same_actions() {
        let source = include_str!("../../../apps/dsk/src/keybinds.ts");
        let start = source
            .find("export type ActionId =")
            .expect("keybinds.ts declares ActionId");
        let block = &source[start..];
        let block = &block[..block.find(';').expect("ActionId ends with a semicolon")];
        let listed: HashSet<&str> = block.split('"').skip(1).step_by(2).collect();
        let actual: HashSet<&str> = ACTIONS.iter().map(|action| action.id).collect();
        let mut missing: Vec<_> = actual.difference(&listed).collect();
        let mut extra: Vec<_> = listed.difference(&actual).collect();
        missing.sort();
        extra.sort();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "keybinds.ts is missing {missing:?} and has extra {extra:?}"
        );
    }
}
