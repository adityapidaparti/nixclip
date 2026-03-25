use gtk4 as gtk;
use gtk4::glib;
use gtk::prelude::*;

use std::cell::RefCell;
use std::rc::Rc;

/// Search input widget with built-in debounce.
pub struct SearchBar {
    pub entry: gtk::SearchEntry,
}

impl SearchBar {
    pub fn new() -> Self {
        let entry = gtk::SearchEntry::new();
        entry.set_placeholder_text(Some("Search clipboard history..."));
        entry.set_hexpand(true);
        entry.set_margin_start(12);
        entry.set_margin_end(12);
        entry.set_margin_top(12);
        entry.set_margin_bottom(6);
        Self { entry }
    }

    /// Connect a callback that fires when search text changes, with a 100ms debounce.
    pub fn connect_search_changed(&self, callback: impl Fn(String) + 'static) {
        let callback = Rc::new(callback);
        let timeout_id: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

        self.entry.connect_search_changed(move |entry| {
            let text = entry.text().to_string();
            let cb = callback.clone();

            // Cancel any pending debounce timer.
            if let Some(id) = timeout_id.borrow_mut().take() {
                id.remove();
            }

            let timeout_id_clone = timeout_id.clone();
            let id = glib::timeout_add_local_once(
                std::time::Duration::from_millis(100),
                move || {
                    cb(text);
                    // Clear the stored id since the timer has fired.
                    timeout_id_clone.borrow_mut().take();
                },
            );
            *timeout_id.borrow_mut() = Some(id);
        });
    }

    pub fn get_text(&self) -> String {
        self.entry.text().to_string()
    }

    pub fn clear(&self) {
        self.entry.set_text("");
    }
}
