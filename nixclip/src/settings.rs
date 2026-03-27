use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use nixclip_core::config::{Config, Retention};

pub fn build_settings_window(
    app: &adw::Application,
    config: Config,
    on_changed: Rc<dyn Fn(Config)>,
    on_clear_history: Rc<dyn Fn()>,
) -> adw::PreferencesWindow {
    let window = adw::PreferencesWindow::new();
    window.set_application(Some(app));
    window.set_title(Some("NixClip Settings"));
    window.set_default_size(500, 600);
    window.set_search_enabled(true);

    let state = Rc::new(RefCell::new(config));

    window.add(&build_general_page(
        &window,
        &state,
        on_changed.clone(),
        on_clear_history.clone(),
    ));
    window.add(&build_privacy_page(&state, on_changed.clone()));
    window.add(&build_about_page(&state));

    window
}

fn build_general_page(
    window: &adw::PreferencesWindow,
    state: &Rc<RefCell<Config>>,
    on_changed: Rc<dyn Fn(Config)>,
    on_clear_history: Rc<dyn Fn()>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("General");
    page.set_icon_name(Some("preferences-other-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title("History");

    let retention_row = adw::ComboRow::new();
    retention_row.set_title("Keep History For");
    let labels = gtk::StringList::new(&[
        "7 days",
        "30 days",
        "3 months",
        "6 months",
        "1 year",
        "Unlimited",
    ]);
    retention_row.set_model(Some(&labels));
    retention_row.set_selected(retention_to_index(&state.borrow().general.retention));
    {
        let s = state.clone();
        let cb = on_changed.clone();
        retention_row.connect_selected_notify(move |row| {
            let retention = index_to_retention(row.selected());
            s.borrow_mut().general.retention = retention;
            cb(s.borrow().clone());
        });
    }
    group.add(&retention_row);

    let adj = gtk::Adjustment::new(
        state.borrow().general.max_entries as f64,
        100.0,
        50_000.0,
        100.0,
        1000.0,
        0.0,
    );
    let max_row = adw::SpinRow::new(Some(&adj), 100.0, 0);
    max_row.set_title("Maximum Entries");
    {
        let s = state.clone();
        let cb = on_changed.clone();
        max_row.connect_value_notify(move |row| {
            s.borrow_mut().general.max_entries = row.value() as u32;
            cb(s.borrow().clone());
        });
    }
    group.add(&max_row);

    let adj2 = gtk::Adjustment::new(
        state.borrow().general.max_blob_size_mb as f64,
        100.0,
        10_000.0,
        100.0,
        500.0,
        0.0,
    );
    let blob_row = adw::SpinRow::new(Some(&adj2), 100.0, 0);
    blob_row.set_title("Maximum Storage (MB)");
    {
        let s = state.clone();
        let cb = on_changed.clone();
        blob_row.connect_value_notify(move |row| {
            s.borrow_mut().general.max_blob_size_mb = row.value() as u32;
            cb(s.borrow().clone());
        });
    }
    group.add(&blob_row);

    page.add(&group);

    let danger = adw::PreferencesGroup::new();
    danger.set_title("Danger Zone");

    let clear_row = adw::ActionRow::new();
    clear_row.set_title("Clear All History");
    clear_row.set_subtitle("Pinned items will be preserved");

    let clear_btn = gtk::Button::with_label("Clear");
    clear_btn.add_css_class("destructive-action");
    clear_btn.set_valign(gtk::Align::Center);
    {
        let window = window.clone();
        let on_clear_history = on_clear_history.clone();
        clear_btn.connect_clicked(move |_| {
            let dialog = adw::MessageDialog::new(
                Some(&window),
                Some("Clear All History?"),
                Some(
                    "All unpinned clipboard entries will be permanently deleted.\n\
                     Pinned items will be preserved.",
                ),
            );
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("clear", "Clear All");
            dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let on_clear_history = on_clear_history.clone();
            dialog.connect_response(None, move |_dialog, response| {
                if response == "clear" {
                    on_clear_history();
                }
            });
            dialog.present();
        });
    }
    clear_row.add_suffix(&clear_btn);
    clear_row.set_activatable_widget(Some(&clear_btn));

    danger.add(&clear_row);
    page.add(&danger);

    page
}

fn build_privacy_page(
    state: &Rc<RefCell<Config>>,
    on_changed: Rc<dyn Fn(Config)>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Privacy");
    page.set_icon_name(Some("security-high-symbolic"));

    let apps_group = adw::PreferencesGroup::new();
    apps_group.set_title("Ignored Applications");
    apps_group.set_description(Some(
        "Clipboard content from these applications will never be stored.",
    ));

    for app_id in &state.borrow().ignore.apps {
        let row = adw::ActionRow::new();
        row.set_title(app_id);

        let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        remove_btn.set_valign(gtk::Align::Center);
        remove_btn.add_css_class("flat");
        row.add_suffix(&remove_btn);

        apps_group.add(&row);
    }

    page.add(&apps_group);

    let pat_group = adw::PreferencesGroup::new();
    pat_group.set_title("Ignored Patterns");
    pat_group.set_description(Some(
        "Content matching these regex patterns will be flagged as ephemeral\n\
         and auto-deleted after first use.",
    ));

    for pattern in &state.borrow().ignore.patterns {
        let row = adw::ActionRow::new();
        row.set_title(pattern);

        let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        remove_btn.set_valign(gtk::Align::Center);
        remove_btn.add_css_class("flat");
        row.add_suffix(&remove_btn);

        pat_group.add(&row);
    }

    page.add(&pat_group);

    let hints_group = adw::PreferencesGroup::new();
    hints_group.set_title("Sensitive Content");

    let hints_row = adw::SwitchRow::new();
    hints_row.set_title("Respect Clipboard Sensitive Hints");
    hints_row.set_subtitle("Automatically ignore content marked as sensitive by apps");
    hints_row.set_active(state.borrow().ignore.respect_sensitive_hints);
    {
        let s = state.clone();
        let cb = on_changed.clone();
        hints_row.connect_active_notify(move |row| {
            s.borrow_mut().ignore.respect_sensitive_hints = row.is_active();
            cb(s.borrow().clone());
        });
    }
    hints_group.add(&hints_row);

    page.add(&hints_group);

    page
}

fn build_about_page(state: &Rc<RefCell<Config>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("About");
    page.set_icon_name(Some("help-about-symbolic"));

    let info_group = adw::PreferencesGroup::new();
    info_group.set_title("About NixClip");

    let version_row = adw::ActionRow::new();
    version_row.set_title("Version");
    version_row.set_subtitle(env!("CARGO_PKG_VERSION"));
    info_group.add(&version_row);

    let license_row = adw::ActionRow::new();
    license_row.set_title("License");
    license_row.set_subtitle("MIT");
    info_group.add(&license_row);

    page.add(&info_group);

    let shortcut_group = adw::PreferencesGroup::new();
    shortcut_group.set_title("Shortcuts");

    let formatted_row = adw::ActionRow::new();
    formatted_row.set_title("Paste with Formatting");
    formatted_row.set_subtitle(&state.borrow().keybind.open_formatted);
    shortcut_group.add(&formatted_row);

    let plain_row = adw::ActionRow::new();
    plain_row.set_title("Paste without Formatting");
    plain_row.set_subtitle(&state.borrow().keybind.open_plain);
    shortcut_group.add(&plain_row);

    let configure_row = adw::ActionRow::new();
    configure_row.set_title("Configure Shortcut");
    configure_row.set_subtitle("Open GNOME Settings → Keyboard → Shortcuts");
    configure_row.set_activatable(true);
    configure_row.connect_activated(|_| {
        // Try to open GNOME Settings keyboard shortcuts page.
        if let Err(e) = std::process::Command::new("gnome-control-center")
            .arg("keyboard")
            .spawn()
        {
            tracing::warn!(error = %e, "could not open GNOME Settings");
        }
    });
    shortcut_group.add(&configure_row);

    page.add(&shortcut_group);

    page
}

fn retention_to_index(r: &Retention) -> u32 {
    match r {
        Retention::Days7 => 0,
        Retention::Days30 => 1,
        Retention::Months3 => 2,
        Retention::Months6 => 3,
        Retention::Year1 => 4,
        Retention::Unlimited => 5,
    }
}

fn index_to_retention(idx: u32) -> Retention {
    match idx {
        0 => Retention::Days7,
        1 => Retention::Days30,
        2 => Retention::Months3,
        3 => Retention::Months6,
        4 => Retention::Year1,
        _ => Retention::Unlimited,
    }
}
