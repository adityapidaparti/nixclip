use adw::prelude::*;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;

use std::collections::HashMap;

use nixclip_core::config::Config;
use nixclip_core::{ContentClass, EntrySummary};

use crate::widgets::entry_row::EntryRow;
use crate::widgets::filter_tabs::FilterTabs;
use crate::widgets::footer::Footer;
use crate::widgets::search_bar::SearchBar;

use std::cell::RefCell;
use std::rc::Rc;

const BADGE_CSS: &str = r#"
.badge-text { background-color: @blue_3; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
.badge-richtext { background-color: @purple_3; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
.badge-image { background-color: @green_4; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
.badge-files { background-color: @orange_3; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
.badge-url { background-color: @teal_3; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
"#;

pub struct PopupWindow {
    pub window: adw::ApplicationWindow,
    search_bar: SearchBar,
    filter_tabs: FilterTabs,
    list_box: gtk::ListBox,
    scrolled: gtk::ScrolledWindow,
    empty_label: gtk::Label,
    spinner: gtk::Spinner,
    footer: Footer,
    rows: Rc<RefCell<Vec<EntryRow>>>,
}

impl PopupWindow {
    pub fn new(app: &adw::Application, config: &Config) -> Self {
        load_badge_css();

        let width = config.ui.width.clamp(480, 1600) as i32;
        let visible_rows = config.ui.max_visible_entries.clamp(1, 16) as i32;
        let min_list_height = (visible_rows * 60).max(180);

        let window = adw::ApplicationWindow::new(app);
        window.set_decorated(false);
        window.set_default_size(width, 500);
        window.set_resizable(false);
        window.set_hide_on_close(true);

        let outer_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(width);
        clamp.set_tightening_threshold(600);

        let inner_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let search_bar = SearchBar::new();
        inner_box.append(&search_bar.entry);

        let filter_tabs = FilterTabs::new();
        inner_box.append(&filter_tabs.container);

        inner_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let empty_label = gtk::Label::new(None);
        empty_label.add_css_class("dim-label");
        empty_label.set_justify(gtk::Justification::Center);
        empty_label.set_wrap(true);
        empty_label.set_margin_top(24);
        empty_label.set_margin_bottom(24);
        empty_label.set_margin_start(24);
        empty_label.set_margin_end(24);
        empty_label.set_visible(false);

        let spinner = gtk::Spinner::new();
        spinner.set_halign(gtk::Align::Center);
        spinner.set_margin_top(24);
        spinner.set_margin_bottom(24);
        spinner.set_visible(false);

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::Single);
        list_box.add_css_class("boxed-list");

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(min_list_height);
        scrolled.set_child(Some(&list_box));

        inner_box.append(&empty_label);
        inner_box.append(&spinner);
        inner_box.append(&scrolled);

        inner_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let footer = Footer::new();
        inner_box.append(&footer.container);

        clamp.set_child(Some(&inner_box));
        outer_box.append(&clamp);
        window.set_content(Some(&outer_box));

        position_top_center(&window);

        let rows: Rc<RefCell<Vec<EntryRow>>> = Rc::new(RefCell::new(Vec::new()));

        let popup = Self {
            window,
            search_bar,
            filter_tabs,
            list_box,
            scrolled,
            empty_label,
            spinner,
            footer,
            rows,
        };

        popup.setup_key_controller();

        popup.setup_focus_out();

        popup
    }

    pub fn populate(&self, entries: Vec<EntrySummary>) {
        self.clear_list();

        if entries.is_empty() {
            self.show_empty_state("No clipboard history yet.\nCopy something to get started.");
            return;
        }

        self.empty_label.set_visible(false);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.scrolled.set_visible(true);

        let mut row_vec = self.rows.borrow_mut();
        for entry in &entries {
            let entry_row = EntryRow::new(entry);
            self.list_box.append(&entry_row.container);
            row_vec.push(entry_row);
        }

        if let Some(first) = self.list_box.row_at_index(0) {
            self.list_box.select_row(Some(&first));
        }
    }

    pub fn get_selected_entry(&self) -> Option<EntrySummary> {
        let selected_row = self.list_box.selected_row()?;
        let idx = selected_row.index() as usize;
        let rows = self.rows.borrow();
        rows.get(idx).map(|r| r.entry.clone())
    }

    pub fn set_filter(&self, class: Option<ContentClass>) {
        self.filter_tabs.set_active(class);
    }

    pub fn show_empty_state(&self, message: &str) {
        self.scrolled.set_visible(false);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.empty_label.set_label(message);
        self.empty_label.set_visible(true);
    }

    pub fn show_loading(&self) {
        self.scrolled.set_visible(false);
        self.empty_label.set_visible(false);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
    }

    pub fn search_bar(&self) -> &SearchBar {
        &self.search_bar
    }

    pub fn filter_tabs(&self) -> &FilterTabs {
        &self.filter_tabs
    }

    pub fn update_result_count(&self, shown: usize, total: u32) {
        self.footer.set_result_count(shown, total);
    }

    pub fn clear_result_count(&self) {
        self.footer.clear_result_count();
    }

    pub fn update_visible_tabs(&self, counts: &HashMap<ContentClass, u32>) {
        self.filter_tabs.update_visible_tabs(counts);
    }

    fn clear_list(&self) {
        while let Some(child) = self.list_box.row_at_index(0) {
            self.list_box.remove(&child);
        }
        self.rows.borrow_mut().clear();
    }

    fn setup_key_controller(&self) {
        let controller = gtk::EventControllerKey::new();
        let window = self.window.clone();
        let search_entry = self.search_bar.entry.clone();

        controller.connect_key_pressed(move |_ctrl, keyval, _keycode, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

            match keyval {
                gdk::Key::Escape => {
                    window.close();
                    glib::Propagation::Stop
                }

                gdk::Key::Return | gdk::Key::KP_Enter => {
                    if shift {
                        let _ = gtk::prelude::WidgetExt::activate_action(
                            &window,
                            "win.restore-plain",
                            None,
                        );
                    } else {
                        let _ = gtk::prelude::WidgetExt::activate_action(
                            &window,
                            "win.restore-original",
                            None,
                        );
                    }
                    glib::Propagation::Stop
                }

                gdk::Key::BackSpace if ctrl => {
                    let _ =
                        gtk::prelude::WidgetExt::activate_action(&window, "win.delete-entry", None);
                    glib::Propagation::Stop
                }

                gdk::Key::p | gdk::Key::P if ctrl => {
                    let _ =
                        gtk::prelude::WidgetExt::activate_action(&window, "win.toggle-pin", None);
                    glib::Propagation::Stop
                }

                gdk::Key::Delete if ctrl && shift => {
                    let _ =
                        gtk::prelude::WidgetExt::activate_action(&window, "win.clear-all", None);
                    glib::Propagation::Stop
                }

                gdk::Key::comma if ctrl => {
                    let _ = gtk::prelude::WidgetExt::activate_action(
                        &window,
                        "win.open-settings",
                        None,
                    );
                    glib::Propagation::Stop
                }

                gdk::Key::_1 if ctrl => {
                    let _ = gtk::prelude::WidgetExt::activate_action(
                        &window,
                        "win.filter",
                        Some(&0i32.to_variant()),
                    );
                    glib::Propagation::Stop
                }
                gdk::Key::_2 if ctrl => {
                    let _ = gtk::prelude::WidgetExt::activate_action(
                        &window,
                        "win.filter",
                        Some(&1i32.to_variant()),
                    );
                    glib::Propagation::Stop
                }
                gdk::Key::_3 if ctrl => {
                    let _ = gtk::prelude::WidgetExt::activate_action(
                        &window,
                        "win.filter",
                        Some(&2i32.to_variant()),
                    );
                    glib::Propagation::Stop
                }
                gdk::Key::_4 if ctrl => {
                    let _ = gtk::prelude::WidgetExt::activate_action(
                        &window,
                        "win.filter",
                        Some(&3i32.to_variant()),
                    );
                    glib::Propagation::Stop
                }
                gdk::Key::_5 if ctrl => {
                    let _ = gtk::prelude::WidgetExt::activate_action(
                        &window,
                        "win.filter",
                        Some(&4i32.to_variant()),
                    );
                    glib::Propagation::Stop
                }

                _ if !ctrl && !shift => {
                    if let Some(ch) = keyval.to_unicode() {
                        if ch.is_alphanumeric() || ch.is_ascii_punctuation() || ch == ' ' {
                            if !search_entry.has_focus() {
                                search_entry.grab_focus();
                                let mut pos = search_entry.text().len() as i32;
                                search_entry.insert_text(&ch.to_string(), &mut pos);
                                search_entry.set_position(pos);
                                return glib::Propagation::Stop;
                            }
                        }
                    }
                    glib::Propagation::Proceed
                }

                _ => glib::Propagation::Proceed,
            }
        });

        self.window.add_controller(controller);
    }

    fn setup_focus_out(&self) {
        self.window
            .connect_notify_local(Some("is-active"), move |win, _| {
                if !win.is_active() {
                    let w = win.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(150),
                        move || {
                            if !w.is_active() {
                                w.close();
                            }
                        },
                    );
                }
            });
    }
}

fn load_badge_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(BADGE_CSS);

    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn position_top_center(window: &adw::ApplicationWindow) {
    window.set_margin_top(48);
}
