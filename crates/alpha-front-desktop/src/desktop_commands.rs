use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopCommand {
    NewSession,
    ResumeSession,
    Model,
    Effort,
    Mode,
    Providers,
    Capabilities,
    ReviewChanges,
    Diagnostics,
    Help,
}

#[derive(Clone, Copy)]
pub(crate) struct CommandPaletteEntry {
    pub(crate) command: DesktopCommand,
    pub(crate) group: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) shortcut: Option<&'static str>,
}

pub(crate) const DESKTOP_COMMANDS: &[CommandPaletteEntry] = &[
    CommandPaletteEntry {
        command: DesktopCommand::NewSession,
        group: "Session",
        title: "New session",
        description: "Start an unpersisted session draft",
        shortcut: Some("Ctrl N"),
    },
    CommandPaletteEntry {
        command: DesktopCommand::ResumeSession,
        group: "Session",
        title: "Resume a session",
        description: "Browse, switch, or delete stored sessions",
        shortcut: Some("Ctrl R"),
    },
    CommandPaletteEntry {
        command: DesktopCommand::Model,
        group: "Route",
        title: "Switch model",
        description: "Change the provider route and apply a default",
        shortcut: Some("Ctrl L"),
    },
    CommandPaletteEntry {
        command: DesktopCommand::Effort,
        group: "Route",
        title: "Change reasoning effort",
        description: "Set the requested reasoning level",
        shortcut: Some("Ctrl T"),
    },
    CommandPaletteEntry {
        command: DesktopCommand::Mode,
        group: "Route",
        title: "Change permission mode",
        description: "Choose read, auto, reviewed, or free mode",
        shortcut: None,
    },
    CommandPaletteEntry {
        command: DesktopCommand::Providers,
        group: "Configuration",
        title: "Provider settings",
        description: "Manage provider instances and credentials",
        shortcut: Some("Ctrl P"),
    },
    CommandPaletteEntry {
        command: DesktopCommand::Capabilities,
        group: "Configuration",
        title: "Set provider capabilities",
        description: "Choose provider-native tools for the current route",
        shortcut: None,
    },
    CommandPaletteEntry {
        command: DesktopCommand::ReviewChanges,
        group: "Workspace",
        title: "Review workspace changes",
        description: "Inspect files changed in this session",
        shortcut: None,
    },
    CommandPaletteEntry {
        command: DesktopCommand::Diagnostics,
        group: "Workspace",
        title: "Core diagnostics",
        description: "Show connection, process, and protocol details",
        shortcut: None,
    },
    CommandPaletteEntry {
        command: DesktopCommand::Help,
        group: "Help",
        title: "Keyboard shortcuts",
        description: "Open the desktop shortcut reference",
        shortcut: Some("?"),
    },
];

pub(crate) fn filtered_commands(query: &str) -> Vec<CommandPaletteEntry> {
    let query = query.trim().to_lowercase();
    let mut matches = DESKTOP_COMMANDS
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, entry)| {
            let score = command_score(&entry, &query);
            (score < 9).then_some((score, index, entry))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|(score, index, _)| (*score, *index));
    matches.into_iter().map(|(_, _, entry)| entry).collect()
}

pub(crate) fn command_score(entry: &CommandPaletteEntry, query: &str) -> u8 {
    if query.is_empty() {
        return 0;
    }
    let title = entry.title.to_lowercase();
    let description = entry.description.to_lowercase();
    let group = entry.group.to_lowercase();
    if title == query {
        0
    } else if has_exact_word(&title, query) {
        1
    } else if title.starts_with(query) {
        2
    } else if has_word_prefix(&title, query) {
        3
    } else if has_exact_word(&description, query) {
        4
    } else if has_word_prefix(&description, query) {
        5
    } else if title.contains(query) {
        6
    } else if description.contains(query) {
        7
    } else if group.contains(query) {
        8
    } else {
        9
    }
}

pub(crate) fn has_exact_word(text: &str, query: &str) -> bool {
    text.split(|ch: char| !ch.is_alphanumeric())
        .any(|word| word == query)
}

pub(crate) fn has_word_prefix(text: &str, query: &str) -> bool {
    text.split(|ch: char| !ch.is_alphanumeric())
        .any(|word| word.starts_with(query))
}

pub(crate) fn command_icon_for_desktop(command: DesktopCommand) -> IconName {
    match command {
        DesktopCommand::NewSession => IconName::MessageSquarePlus,
        DesktopCommand::ResumeSession => IconName::Clock,
        DesktopCommand::Model => IconName::Sparkles,
        DesktopCommand::Effort => IconName::Gauge,
        DesktopCommand::Mode => IconName::ShieldCheck,
        DesktopCommand::Providers => IconName::Server,
        DesktopCommand::Capabilities => IconName::SlidersHorizontal,
        DesktopCommand::ReviewChanges => IconName::FilePen,
        DesktopCommand::Diagnostics => IconName::Cpu,
        DesktopCommand::Help => IconName::Keyboard,
    }
}

#[cfg(test)]
mod tests {
    use super::{DesktopCommand, filtered_commands};

    #[test]
    fn command_palette_filters_titles_descriptions_and_groups() {
        assert_eq!(filtered_commands("")[0].command, DesktopCommand::NewSession);
        assert_eq!(filtered_commands("mode")[0].command, DesktopCommand::Mode);
        assert_eq!(filtered_commands("model")[0].command, DesktopCommand::Model);
        assert!(
            filtered_commands("credential")
                .iter()
                .any(|entry| entry.command == DesktopCommand::Providers)
        );
        assert!(
            filtered_commands("route")
                .iter()
                .any(|entry| entry.command == DesktopCommand::Model)
        );
        assert!(filtered_commands("not a desktop command").is_empty());
    }
}
