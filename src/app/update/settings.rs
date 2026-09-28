//! Settings panel actions: opening / resetting `config.toml` and
//! `keybindings.toml`.

use super::*;

/// Opens the user config file as an editor tab (see `config_file`).
pub(super) fn open_config(model: &mut Model) -> Vec<Cmd> {
    config_file(model).map_or_else(Vec::new, |p| open_path(model, p))
}

/// Opens the keybindings file as an editor tab (see `keybindings_file`). Bound
/// to Alt+7 ("Shortcuts").
pub(super) fn open_keybindings(model: &mut Model) -> Vec<Cmd> {
    keybindings_file(model).map_or_else(Vec::new, |p| open_path(model, p))
}

/// The user config file, created (with the current defaults) first if it
/// doesn't exist yet so there is always something to edit.
pub(super) fn config_file(model: &mut Model) -> Option<PathBuf> {
    let Some(path) = crate::services::config::config_path() else {
        model.notify("No config path available".to_string());
        return None;
    };
    if !path.exists() {
        if let Err(e) = crate::services::config::save(&model.config_snapshot()) {
            model.notify(format!("Could not create {}: {e}", path.display()));
            return None;
        }
    }
    Some(path)
}

/// The keybindings file, seeded with the current shortcuts first if it
/// doesn't exist yet.
pub(super) fn keybindings_file(model: &mut Model) -> Option<PathBuf> {
    let Some(path) = crate::services::keybindings::keybindings_path() else {
        model.notify("No config path available".to_string());
        return None;
    };
    if !path.exists() {
        if let Err(e) = crate::services::keybindings::save(&model.keybindings) {
            model.notify(format!("Could not create {}: {e}", path.display()));
            return None;
        }
    }
    Some(path)
}

/// Runs the Settings panel's selected row: reset actions open a confirmation
/// dialog; the edit rows open the file in the editor. Shared by keyboard Enter
/// and mouse click.
pub(super) fn activate_settings(model: &mut Model) -> Vec<Cmd> {
    let Some(&item) = crate::ui::sidebar::SETTINGS_ITEMS.get(model.sidebar.settings_selected)
    else {
        return Vec::new();
    };
    use crate::ui::sidebar::SettingsItem;
    match item {
        SettingsItem::ResetKeybindings => {
            model.dialog = Some(Dialog::ask(
                "Reset keybindings".to_string(),
                "All shortcuts will be restored to their defaults, discarding your \
                 customizations. Are you sure?"
                    .to_string(),
                DialogAction::ResetKeybindings,
            ));
            Vec::new()
        }
        SettingsItem::ResetConfig => {
            model.dialog = Some(Dialog::ask(
                "Reset config".to_string(),
                "Theme, editor settings and language tooling will be restored to their \
                 defaults, discarding your customizations. Are you sure?"
                    .to_string(),
                DialogAction::ResetConfig,
            ));
            Vec::new()
        }
        SettingsItem::AsciiIcons => {
            model.ascii_icons = !model.ascii_icons;
            persist_config(model)
        }
        SettingsItem::EditKeybindings => open_keybindings(model),
        SettingsItem::EditConfig => open_config(model),
    }
}

/// Restores the built-in keybindings, overwriting `keybindings.toml`, and syncs
/// the open tab (if any) even if it has unsaved edits — reset discards them.
pub(super) fn reset_keybindings(model: &mut Model) -> Vec<Cmd> {
    model.keybindings = crate::services::keybindings::Keybindings::default();
    let Some(path) = crate::services::keybindings::keybindings_path() else {
        model.notify("No config path available".to_string());
        return Vec::new();
    };
    let contents = crate::services::keybindings::to_toml(&model.keybindings);
    force_replace_open_buffer(model, &path, &contents);
    model.notify("Keybindings reset to defaults".to_string());
    vec![Cmd::WriteFile { path, contents }]
}

/// Restores the seeded config (theme, editor settings, starter languages),
/// re-applies it live, overwrites `config.toml`, and syncs the open tab.
pub(super) fn reset_config(model: &mut Model) -> Vec<Cmd> {
    let cfg = crate::services::config::seed();
    model.apply_config(&cfg);
    let Some(path) = crate::services::config::config_path() else {
        model.notify("No config path available".to_string());
        return Vec::new();
    };
    let contents = crate::services::config::to_toml(&cfg);
    force_replace_open_buffer(model, &path, &contents);
    model.notify("Config reset to defaults".to_string());
    vec![
        Cmd::WriteFile { path, contents },
        Cmd::CheckTools(model.extensions.tool_commands()),
    ]
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::ui::sidebar::SettingsItem;

    #[test]
    fn ascii_icons_toggle_flips_and_persists() {
        let mut model = Model::new(std::env::temp_dir());
        assert!(!model.ascii_icons, "default is Nerd Font icons");
        let idx = crate::ui::sidebar::SETTINGS_ITEMS
            .iter()
            .position(|i| *i == SettingsItem::AsciiIcons)
            .unwrap();
        model.sidebar.settings_selected = idx;

        let cmds = activate_settings(&mut model);
        assert!(model.ascii_icons, "first activation turns it on");
        assert!(cmds.iter().any(|c| matches!(c, Cmd::SaveConfig(_))));

        let cmds = activate_settings(&mut model);
        assert!(!model.ascii_icons, "second activation turns it back off");
        assert!(cmds.iter().any(|c| matches!(c, Cmd::SaveConfig(_))));
    }
}
