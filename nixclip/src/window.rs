//! The popup window -- the core of the NixClip UI.

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk::prelude::*;
use libadwaita as adw;

use nixclip_core::{ContentClass, EntrySummary};

use crate::widgets::entry_row::EntryRow;
use crate::widgets::filter_tabs::FilterTabs;
use crate::widgets::footer::build_footer;
use crate::widgets::search_bar::SearchBar;

use std::cell::RefCell;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Custom CSS for content-class badges
// ---------------------------------------------------------------------------

const BADGE_CSS: &str = r#"
.badge-text { background-color: @blue_3; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
.badge-richtext { background-color: @purple_3; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
.badge-image { background-color: @green_4; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
.badge-files { background-color: @orange_3; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
.badge-url { background-color: @teal_3; color: white; padding: 2px 6px; border-radius: 4px; font-size: 0.8em; font-weight: bold; }
"#;

// ---------------------------------------------------------------------------
// PopupWindow
// ---------------------------------------------------------------------------

/// The main clipboard-history popup window.
pub struct PopupWindow {
    pub window: adw::Window,
    search_bar: SearchBar,
    filter_tabs: FilterTabs,
    list_box: gtk::ListBox,
    scrolled: gtk::ScrolledWindow,
    empty_label: gtk::Label,
    spinner: gtk::Spinner,
    /// The entry rows currently displayed, kept so we can retrieve selection data.
    rows: Rc<RefCell<Vec<EntryRow>>>,
}

impl PopupWindow {
    pub fn new(app: &adw::Application) -> Self {
        // --- Load custom CSS -------------------------------------------------
        load_badge_css();

        // --- Window ----------------------------------------------------------
        let window = adw::Window::new();
        window.set_application(Some(app));
        window.set_decorated(false);
        window.set_default_size(680, 500);
        window.set_resizable(false);
        // Ask the window manager not to show this in the taskbar.
        // (Note: hide-on-close keeps the window object alive after close.)
        window.set_hide_on_close(true);

        // --- Layout ----------------------------------------------------------
        let outer_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // Wrap content in an Adwaita Clamp for nice max-width behaviour.
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(680);
        clamp.set_tightening_threshold(600);

        let inner_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // Search bar.
        let search_bar = SearchBar::new();
        inner_box.append(&search_bar.entry);

        // Filter tabs.
        let filter_tabs = FilterTabs::new();
        inner_box.append(&filter_tabs.container);

        // Separator.
        inner_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // Empty / error state label (hidden by default).
        let empty_label = gtk::Label::new(None);
        empty_label.add_css_class("dim-label");
        empty_label.set_justify(gtk::Justification::Center);
        empty_label.set_wrap(true);
        empty_label.set_margin_top(24);
        empty_label.set_margin_bottom(24);
        empty_label.set_margin_start(24);
        empty_label.set_margin_end(24);
        empty_label.set_visible(false);

        // Loading spinner (hidden by default).
        let spinner = gtk::Spinner::new();
        spinner.set_halign(gtk::Align::Center);
        spinner.set_margin_top(24);
        spinner.set_margin_bottom(24);
        spinner.set_visible(false);

        // Scrollable list.
        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::Single);
        list_box.add_css_class("boxed-list");

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_vexpand(true);
        // Roughly 8 rows * 60px = 480px minus header/footer.
        scrolled.set_min_content_height(320);
        scrolled.set_child(Some(&list_box));

        inner_box.append(&empty_label);
        inner_box.append(&spinner);
        inner_box.append(&scrolled);

        // Separator before footer.
        inner_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // Footer.
        let footer = build_footer();
        inner_box.append(&footer);

        clamp.set_child(Some(&inner_box));
        outer_box.append(&clamp);
        window.set_content(Some(&outer_box));

        // --- Position at top-center ------------------------------------------
        position_top_center(&window);

        // --- Build struct before wiring signals ------------------------------
        let rows: Rc<RefCell<Vec<EntryRow>>> = Rc::new(RefCell::new(Vec::new()));

        let popup = Self {
            window,
            search_bar,
            filter_tabs,
            list_box,
            scrolled,
            empty_label,
            spinner,
            rows,
        };

        // --- Keyboard handling -----------------------------------------------
        popup.setup_key_controller();

        // --- Focus-out handling ----------------------------------------------
        popup.setup_focus_out();

        popup
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Populate the list with the given entries, replacing any existing rows.
    pub fn populate(&self, entries: Vec<EntrySummary>) {
        // Clear existing rows.
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

        // Select the first row.
        if let Some(first) = self.list_box.row_at_index(0) {
            self.list_box.select_row(Some(&first));
        }
    }

    /// Return the `EntrySummary` for the currently selected row, if any.
    pub fn get_selected_entry(&self) -> Option<EntrySummary> {
        let selected_row = self.list_box.selected_row()?;
        let idx = selected_row.index() as usize;
        let rows = self.rows.borrow();
        rows.get(idx).map(|r| r.entry.clone())
    }

    /// Set the content-class filter (updates the tab UI).
    pub fn set_filter(&self, class: Option<ContentClass>) {
        self.filter_tabs.set_active(class);
    }

    /// Show a centered message in the empty/informational state.
    pub fn show_empty_state(&self, message: &str) {
        self.scrolled.set_visible(false);
        self.spinner.set_visible(false);
        self.spinner.set_spinning(false);
        self.empty_label.set_label(message);
        self.empty_label.set_visible(true);
    }

    /// Show an error message.
    pub fn show_error_state(&self, message: &str) {
        self.show_empty_state(message);
    }

    /// Show a loading spinner.
    pub fn show_loading(&self) {
        self.scrolled.set_visible(false);
        self.empty_label.set_visible(false);
        self.spinner.set_visible(true);
        self.spinner.set_spinning(true);
    }

    /// Access the search bar for wiring external callbacks.
    pub fn search_bar(&self) -> &SearchBar {
        &self.search_bar
    }

    /// Access the filter tabs for wiring external callbacks.
    pub fn filter_tabs(&self) -> &FilterTabs {
        &self.filter_tabs
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    /// Remove all rows from the list box.
    fn clear_list(&self) {
        // Remove children from the list box.
        while let Some(child) = self.list_box.row_at_index(0) {
            self.list_box.remove(&child);
        }
        self.rows.borrow_mut().clear();
    }

    /// Wire the `EventControllerKey` for keyboard shortcuts.
    fn setup_key_controller(&self) {
        let controller = gtk::EventControllerKey::new();
        let window = self.window.clone();
        let list_box = self.list_box.clone();
        let search_entry = self.search_bar.entry.clone();

        controller.connect_key_pressed(move |_ctrl, keyval, _keycode, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

            match keyval {
                // Escape -> close popup.
                gdk::Key::Escape => {
                    window.close();
                    glib::Propagation::Stop
                }

                // Return -> restore selected entry.
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    if shift {
                        // Shift+Return: plain-text restore.
                        // The actual restore is handled by app.rs via a signal;
                        // we emit an action on the window.
                        window.activate_action("win.restore-plain", None);
                    } else {
                        window.activate_action("win.restore-original", None);
                    }
                    glib::Propagation::Stop
                }

                // Ctrl+BackSpace -> delete selected entry.
                gdk::Key::BackSpace if ctrl => {
                    window.activate_action("win.delete-entry", None);
                    glib::Propagation::Stop
                }

                // Ctrl+P -> pin/unpin.
                gdk::Key::p | gdk::Key::P if ctrl => {
                    window.activate_action("win.toggle-pin", None);
                    glib::Propagation::Stop
                }

                // Ctrl+Shift+Delete -> clear all.
                gdk::Key::Delete if ctrl && shift => {
                    window.activate_action("win.clear-all", None);
                    glib::Propagation::Stop
                }

                // Ctrl+comma -> open settings.
                gdk::Key::comma if ctrl => {
                    window.activate_action("win.open-settings", None);
                    glib::Propagation::Stop
                }

                // Ctrl+1..5 -> filter tabs.
                gdk::Key::_1 if ctrl => {
                    window.activate_action("win.filter", Some(&0i32.to_variant()));
                    glib::Propagation::Stop
                }
                gdk::Key::_2 if ctrl => {
                    window.activate_action("win.filter", Some(&1i32.to_variant()));
                    glib::Propagation::Stop
                }
                gdk::Key::_3 if ctrl => {
                    window.activate_action("win.filter", Some(&2i32.to_variant()));
                    glib::Propagation::Stop
                }
                gdk::Key::_4 if ctrl => {
                    window.activate_action("win.filter", Some(&3i32.to_variant()));
                    glib::Propagation::Stop
                }
                gdk::Key::_5 if ctrl => {
                    window.activate_action("win.filter", Some(&4i32.to_variant()));
                    glib::Propagation::Stop
                }

                // Any printable character -> redirect to search entry.
                _ if !ctrl && !shift => {
                    if let Some(ch) = keyval.to_unicode() {
                        if ch.is_alphanumeric() || ch.is_ascii_punctuation() || ch == ' ' {
                            // Only grab focus if the search entry doesn't already have it.
                            if !search_entry.has_focus() {
                                search_entry.grab_focus();
                                // Insert the character.
                                let pos = search_entry.text().len() as i32;
                                search_entry
                                    .editable()
                                    .insert_text(&ch.to_string(), &mut pos.clone());
                                search_entry
                                    .editable()
                                    .set_position(pos + 1);
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

    /// Close the window when it loses focus (with a small grace period to avoid
    /// transient focus losses, e.g., when opening a dialog).
    fn setup_focus_out(&self) {
        let window = self.window.clone();
        let is_active = window.property::<bool>("is-active");
        let _ = is_active; // we read it below via notify

        self.window.connect_notify_local(Some("is-active"), move |win, _| {
            if !win.is_active() {
                // Small delay so that transient focus changes (e.g., opening
                // a confirmation dialog) don't dismiss us immediately.
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load the badge CSS via a CssProvider so Adwaita named colors resolve.
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

/// Attempt to position the window at the top-center of the primary monitor.
fn position_top_center(window: &adw::Window) {
    // GTK4 on Wayland does not allow arbitrary window positioning by the
    // client.  On X11 the window manager handles placement.  We set a top
    // margin so that the content at least looks visually top-biased inside
    // the window, and rely on the WM / layer-shell for actual screen
    // positioning.  If a surface/layer-shell protocol is available, the
    // caller can use it.
    window.set_margin_top(48);
}
