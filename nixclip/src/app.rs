//! Application state and lifecycle for the NixClip popup UI.
//!
//! Handles window creation, IPC connection, and wiring keyboard-shortcut
//! actions to the daemon.

use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use libadwaita as adw;

use nixclip_core::config::Config;
use nixclip_core::{ContentClass, RestoreMode};

use crate::ipc_client::UiIpcClient;
use crate::window::PopupWindow;

// ---------------------------------------------------------------------------
// Activation entry-point
// ---------------------------------------------------------------------------

/// Called by `Application::connect_activate`.  Creates the popup window,
/// connects to the daemon, wires actions and callbacks, and presents.
pub fn activate(app: &adw::Application) {
    let config = Config::load_or_default();
    let socket_path = Config::socket_path();

    let ipc = Rc::new(UiIpcClient::new(&socket_path));
    let popup = Rc::new(PopupWindow::new(app, &config));

    setup_actions(&popup, &ipc);
    setup_search(&popup, &ipc);
    setup_filters(&popup, &ipc);

    // Initial load.
    load_entries(&popup, &ipc, None, None);

    popup.window.present();
}

// ---------------------------------------------------------------------------
// Actions (installed on the window so `win.xxx` works from key controller)
// ---------------------------------------------------------------------------

fn setup_actions(popup: &Rc<PopupWindow>, ipc: &Rc<UiIpcClient>) {
    let win = &popup.window;

    // --- restore-original ---------------------------------------------------
    add_action(win, "restore-original", None, {
        let p = popup.clone();
        let i = ipc.clone();
        move |_, _| {
            if let Some(entry) = p.get_selected_entry() {
                i.restore(entry.id, RestoreMode::Original, |r| {
                    if let Err(e) = r {
                        tracing::warn!(error = %e, "restore failed");
                    }
                });
                p.window.close();
            }
        }
    });

    // --- restore-plain ------------------------------------------------------
    add_action(win, "restore-plain", None, {
        let p = popup.clone();
        let i = ipc.clone();
        move |_, _| {
            if let Some(entry) = p.get_selected_entry() {
                i.restore(entry.id, RestoreMode::PlainText, |r| {
                    if let Err(e) = r {
                        tracing::warn!(error = %e, "restore plain failed");
                    }
                });
                p.window.close();
            }
        }
    });

    // --- delete-entry -------------------------------------------------------
    add_action(win, "delete-entry", None, {
        let p = popup.clone();
        let i = ipc.clone();
        move |_, _| {
            if let Some(entry) = p.get_selected_entry() {
                let pp = p.clone();
                let ii = i.clone();
                i.delete(entry.id, move |r| {
                    if let Err(e) = r {
                        tracing::warn!(error = %e, "delete failed");
                    } else {
                        load_entries(&pp, &ii, None, None);
                    }
                });
            }
        }
    });

    // --- toggle-pin ---------------------------------------------------------
    add_action(win, "toggle-pin", None, {
        let p = popup.clone();
        let i = ipc.clone();
        move |_, _| {
            if let Some(entry) = p.get_selected_entry() {
                let pp = p.clone();
                let ii = i.clone();
                i.pin(entry.id, !entry.pinned, move |r| {
                    if let Err(e) = r {
                        tracing::warn!(error = %e, "pin toggle failed");
                    } else {
                        load_entries(&pp, &ii, None, None);
                    }
                });
            }
        }
    });

    // --- clear-all ----------------------------------------------------------
    add_action(win, "clear-all", None, {
        let p = popup.clone();
        let i = ipc.clone();
        move |_, _| {
            let dialog = adw::MessageDialog::new(
                Some(&p.window),
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

            let pp = p.clone();
            let ii = i.clone();
            dialog.connect_response(None, move |_dlg, response| {
                if response == "clear" {
                    let ppp = pp.clone();
                    let iii = ii.clone();
                    ii.clear_unpinned(move |r| {
                        if let Err(e) = r {
                            tracing::warn!(error = %e, "clear all failed");
                        } else {
                            load_entries(&ppp, &iii, None, None);
                        }
                    });
                }
            });
            dialog.present();
        }
    });

    // --- open-settings ------------------------------------------------------
    add_action(win, "open-settings", None, {
        let p = popup.clone();
        move |_, _| {
            let app = p.window.application().expect("window missing application");
            let adw_app: adw::Application = app.downcast().expect("not an adw::Application");
            let config = Config::load_or_default();

            let settings_win =
                crate::settings::build_settings_window(&adw_app, config, |_new_config| {
                    // In a full implementation, send SetConfig IPC to the daemon.
                    tracing::info!("settings updated");
                });
            settings_win.set_transient_for(Some(&p.window));
            settings_win.present();
        }
    });

    // --- filter (parameterised: i32 index) ----------------------------------
    add_action(win, "filter", Some(&i32::static_variant_type()), {
        let p = popup.clone();
        let i = ipc.clone();
        move |_, param| {
            let idx = param.and_then(|v| v.get::<i32>()).unwrap_or(0);
            let class = match idx {
                1 => Some(ContentClass::Text),
                2 => Some(ContentClass::Image),
                3 => Some(ContentClass::Files),
                4 => Some(ContentClass::Url),
                _ => None, // 0 or unknown = All
            };
            p.set_filter(class);
            load_entries(&p, &i, None, class);
        }
    });
}

/// Helper: create a `SimpleAction`, connect its `activate` signal, and add it
/// to the given window's action group.
fn add_action(
    window: &adw::Window,
    name: &str,
    parameter_type: Option<&glib::VariantType>,
    callback: impl Fn(&gio::SimpleAction, Option<&glib::Variant>) + 'static,
) {
    let action = gio::SimpleAction::new(name, parameter_type);
    action.connect_activate(callback);
    window.add_action(&action);
}

// ---------------------------------------------------------------------------
// Search & filter callbacks
// ---------------------------------------------------------------------------

fn setup_search(popup: &Rc<PopupWindow>, ipc: &Rc<UiIpcClient>) {
    let p = popup.clone();
    let i = ipc.clone();
    popup.search_bar().connect_search_changed(move |text| {
        let query = if text.is_empty() { None } else { Some(text) };
        load_entries(&p, &i, query, None);
    });
}

fn setup_filters(popup: &Rc<PopupWindow>, ipc: &Rc<UiIpcClient>) {
    let p = popup.clone();
    let i = ipc.clone();
    popup.filter_tabs().connect_filter_changed(move |class| {
        load_entries(&p, &i, None, class);
    });
}

// ---------------------------------------------------------------------------
// Data loading
// ---------------------------------------------------------------------------

fn load_entries(
    popup: &Rc<PopupWindow>,
    ipc: &Rc<UiIpcClient>,
    query: Option<String>,
    class: Option<ContentClass>,
) {
    let p = popup.clone();
    let update_tabs = class.is_none();
    ipc.query(query, class, 50, move |result| {
        match result {
            Ok((entries, total)) => {
                if entries.is_empty() {
                    p.show_empty_state(
                        "No clipboard history yet.\nCopy something to get started.",
                    );
                    p.clear_result_count();
                } else {
                    // When loading the unfiltered view, update which filter
                    // tabs are visible based on the content classes present.
                    if update_tabs {
                        let mut counts: HashMap<ContentClass, u32> = HashMap::new();
                        for e in &entries {
                            *counts.entry(e.content_class).or_insert(0) += 1;
                        }
                        p.update_visible_tabs(&counts);
                    }

                    let shown = entries.len();
                    p.populate(entries);
                    p.update_result_count(shown, total);
                }
            }
            Err(e) => {
                p.clear_result_count();
                p.show_error_state(&format!(
                    "Cannot connect to nixclipd.\n\
                     Is the daemon running?\n\n\
                     Run 'nixclip doctor' for diagnostics.\n\n\
                     Error: {e}"
                ));
            }
        }
    });
}
