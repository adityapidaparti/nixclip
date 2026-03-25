use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;
use libadwaita as adw;

use nixclip_core::config::Config;
use nixclip_core::{ContentClass, RestoreMode};

use crate::ipc_client::UiIpcClient;
use crate::window::PopupWindow;

#[derive(Clone, Default)]
struct QueryState {
    text: Option<String>,
    class: Option<ContentClass>,
}

struct UiHandle {
    ipc: Rc<UiIpcClient>,
    popup: Rc<PopupWindow>,
    query_state: Rc<RefCell<QueryState>>,
}

impl UiHandle {
    fn new(app: &adw::Application) -> Self {
        let config = Config::load_or_default();
        let socket_path = Config::socket_path();

        let ipc = Rc::new(UiIpcClient::new(&socket_path));
        let popup = Rc::new(PopupWindow::new(app, &config));
        let query_state = Rc::new(RefCell::new(QueryState::default()));

        setup_actions(&popup, &ipc, &query_state);
        setup_search(&popup, &ipc, &query_state);
        setup_filters(&popup, &ipc, &query_state);

        Self {
            ipc,
            popup,
            query_state,
        }
    }

    fn present(&self, activation_token: Option<&str>) {
        self.query_state.replace(QueryState::default());
        self.popup.search_bar().clear();
        self.popup.set_filter(None);
        load_entries(&self.popup, &self.ipc, &self.query_state);

        if let Some(token) = activation_token {
            self.popup.window.set_startup_id(token);
        }

        self.popup.window.present();
        self.popup.search_bar().entry.grab_focus();
    }
}

pub(crate) fn activate(
    app: &adw::Application,
    state: &Rc<RefCell<Option<UiHandle>>>,
    activation_token: Option<&str>,
) {
    let mut state = state.borrow_mut();
    if state.is_none() {
        *state = Some(UiHandle::new(app));
    }

    if let Some(ui) = state.as_ref() {
        ui.present(activation_token);
    }
}

fn setup_actions(
    popup: &Rc<PopupWindow>,
    ipc: &Rc<UiIpcClient>,
    query_state: &Rc<RefCell<QueryState>>,
) {
    let win = &popup.window;

    add_action(win, "restore-original", None, {
        let p = popup.clone();
        let i = ipc.clone();
        move |_, _| restore_selected_entry(&p, &i, RestoreMode::Original, "restore failed")
    });

    add_action(win, "restore-plain", None, {
        let p = popup.clone();
        let i = ipc.clone();
        move |_, _| restore_selected_entry(&p, &i, RestoreMode::PlainText, "restore plain failed")
    });

    add_action(win, "delete-entry", None, {
        let p = popup.clone();
        let i = ipc.clone();
        let q = query_state.clone();
        move |_, _| {
            if let Some(entry) = p.get_selected_entry() {
                let pp = p.clone();
                let ii = i.clone();
                let qq = q.clone();
                i.delete(entry.id, move |result| {
                    if let Err(error) = result {
                        tracing::warn!(error = %error, "delete failed");
                    } else {
                        load_entries(&pp, &ii, &qq);
                    }
                });
            }
        }
    });

    add_action(win, "toggle-pin", None, {
        let p = popup.clone();
        let i = ipc.clone();
        let q = query_state.clone();
        move |_, _| {
            if let Some(entry) = p.get_selected_entry() {
                let pp = p.clone();
                let ii = i.clone();
                let qq = q.clone();
                i.pin(entry.id, !entry.pinned, move |result| {
                    if let Err(error) = result {
                        tracing::warn!(error = %error, "pin toggle failed");
                    } else {
                        load_entries(&pp, &ii, &qq);
                    }
                });
            }
        }
    });

    add_action(win, "clear-all", None, {
        let p = popup.clone();
        let i = ipc.clone();
        let q = query_state.clone();
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
            let qq = q.clone();
            dialog.connect_response(None, move |_dlg, response| {
                if response == "clear" {
                    let pp = pp.clone();
                    let ii_cb = ii.clone();
                    let qq = qq.clone();
                    ii.clear_unpinned(move |result| {
                        if let Err(error) = result {
                            tracing::warn!(error = %error, "clear all failed");
                        } else {
                            load_entries(&pp, &ii_cb, &qq);
                        }
                    });
                }
            });
            dialog.present();
        }
    });

    add_action(win, "open-settings", None, {
        let p = popup.clone();
        let i = ipc.clone();
        let q = query_state.clone();
        move |_, _| {
            let app = p.window.application().expect("window missing application");
            let adw_app: adw::Application = app.downcast().expect("not an adw::Application");

            let pp = p.clone();
            let ii = i.clone();
            let qq = q.clone();
            i.get_config(move |result| {
                let config = match result {
                    Ok(config) => config,
                    Err(error) => {
                        tracing::warn!(error = %error, "failed to fetch daemon config");
                        Config::load_or_default()
                    }
                };

                let on_changed: Rc<dyn Fn(Config)> = {
                    let ipc = ii.clone();
                    Rc::new(move |new_config: Config| {
                        ipc.set_config(new_config, |result| {
                            if let Err(error) = result {
                                tracing::warn!(error = %error, "settings update failed");
                            }
                        });
                    })
                };

                let on_clear_history: Rc<dyn Fn()> = {
                    let p = pp.clone();
                    let i = ii.clone();
                    let q = qq.clone();
                    Rc::new(move || {
                        let pp = p.clone();
                        let ii = i.clone();
                        let qq = q.clone();
                        i.clear_unpinned(move |result| {
                            if let Err(error) = result {
                                tracing::warn!(
                                    error = %error,
                                    "clear history from settings failed"
                                );
                            } else {
                                load_entries(&pp, &ii, &qq);
                            }
                        });
                    })
                };

                let settings_win = crate::settings::build_settings_window(
                    &adw_app,
                    config,
                    on_changed,
                    on_clear_history,
                );
                settings_win.set_transient_for(Some(&pp.window));
                settings_win.present();
            });
        }
    });

    add_action(win, "filter", Some(glib::VariantTy::INT32), {
        let p = popup.clone();
        let i = ipc.clone();
        let q = query_state.clone();
        move |_, param| {
            let idx = param.and_then(|value| value.get::<i32>()).unwrap_or(0);
            let class = match idx {
                1 => Some(ContentClass::Text),
                2 => Some(ContentClass::Image),
                3 => Some(ContentClass::Files),
                4 => Some(ContentClass::Url),
                _ => None,
            };

            p.set_filter(class);
            q.borrow_mut().class = class;
            load_entries(&p, &i, &q);
        }
    });
}

fn add_action(
    window: &adw::ApplicationWindow,
    name: &str,
    parameter_type: Option<&glib::VariantTy>,
    callback: impl Fn(&gio::SimpleAction, Option<&glib::Variant>) + 'static,
) {
    let action = gio::SimpleAction::new(name, parameter_type);
    action.connect_activate(callback);
    window.add_action(&action);
}

fn setup_search(
    popup: &Rc<PopupWindow>,
    ipc: &Rc<UiIpcClient>,
    query_state: &Rc<RefCell<QueryState>>,
) {
    let p = popup.clone();
    let i = ipc.clone();
    let q = query_state.clone();
    popup.search_bar().connect_search_changed(move |text| {
        q.borrow_mut().text = if text.is_empty() { None } else { Some(text) };
        load_entries(&p, &i, &q);
    });
}

fn setup_filters(
    popup: &Rc<PopupWindow>,
    ipc: &Rc<UiIpcClient>,
    query_state: &Rc<RefCell<QueryState>>,
) {
    let p = popup.clone();
    let i = ipc.clone();
    let q = query_state.clone();
    popup.filter_tabs().connect_filter_changed(move |class| {
        q.borrow_mut().class = class;
        load_entries(&p, &i, &q);
    });
}

fn load_entries(
    popup: &Rc<PopupWindow>,
    ipc: &Rc<UiIpcClient>,
    query_state: &Rc<RefCell<QueryState>>,
) {
    let state = query_state.borrow().clone();
    let empty_message = match (state.text.as_deref(), state.class) {
        (Some(query), _) => format!("No matches for '{query}'. Try a different search."),
        (None, Some(_)) => "No clipboard history for the selected filter.".to_string(),
        (None, None) => "No clipboard history yet.\nCopy something to get started.".to_string(),
    };

    let update_tabs = state.class.is_none();
    popup.show_loading();

    let p = popup.clone();
    ipc.query(state.text, state.class, 50, move |result| match result {
        Ok((entries, total)) => {
            if entries.is_empty() {
                p.show_empty_state(&empty_message);
                p.clear_result_count();
            } else {
                if update_tabs {
                    let mut counts: HashMap<ContentClass, u32> = HashMap::new();
                    for entry in &entries {
                        *counts.entry(entry.content_class).or_insert(0) += 1;
                    }
                    p.update_visible_tabs(&counts);
                }

                let shown = entries.len();
                p.populate(entries);
                p.update_result_count(shown, total);
            }
        }
        Err(error) => {
            p.clear_result_count();
            p.show_empty_state(&format!(
                "Cannot connect to nixclipd.\n\
                 Is the daemon running?\n\n\
                 Run 'nixclip doctor' for diagnostics.\n\n\
                 Error: {error}"
            ));
        }
    });
}

fn restore_selected_entry(
    popup: &Rc<PopupWindow>,
    ipc: &Rc<UiIpcClient>,
    mode: RestoreMode,
    error_message: &'static str,
) {
    let Some(entry) = popup.get_selected_entry() else {
        return;
    };

    let popup = popup.clone();
    ipc.restore(entry.id, mode, move |result| {
        if let Err(error) = result {
            tracing::warn!(error = %error, "{error_message}");
        }
    });
    popup.window.close();
}
