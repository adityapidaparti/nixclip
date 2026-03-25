use gtk4 as gtk;
use gtk::prelude::*;
use nixclip_core::ContentClass;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A row of toggle buttons for filtering entries by content class.
pub struct FilterTabs {
    pub container: gtk::Box,
    active_filter: Rc<RefCell<Option<ContentClass>>>,
    btn_all: gtk::ToggleButton,
    btn_text: gtk::ToggleButton,
    btn_images: gtk::ToggleButton,
    btn_files: gtk::ToggleButton,
    btn_links: gtk::ToggleButton,
}

impl FilterTabs {
    pub fn new() -> Self {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        container.set_halign(gtk::Align::Center);
        container.set_margin_start(12);
        container.set_margin_end(12);
        container.set_margin_bottom(6);
        container.add_css_class("linked");

        let btn_all = gtk::ToggleButton::with_label("All");
        let btn_text = gtk::ToggleButton::with_label("Text");
        let btn_images = gtk::ToggleButton::with_label("Images");
        let btn_files = gtk::ToggleButton::with_label("Files");
        let btn_links = gtk::ToggleButton::with_label("Links");

        // Group all buttons so only one can be active at a time.
        btn_text.set_group(Some(&btn_all));
        btn_images.set_group(Some(&btn_all));
        btn_files.set_group(Some(&btn_all));
        btn_links.set_group(Some(&btn_all));

        // "All" starts active.
        btn_all.set_active(true);

        container.append(&btn_all);
        container.append(&btn_text);
        container.append(&btn_images);
        container.append(&btn_files);
        container.append(&btn_links);

        Self {
            container,
            active_filter: Rc::new(RefCell::new(None)),
            btn_all,
            btn_text,
            btn_images,
            btn_files,
            btn_links,
        }
    }

    /// Connect a callback that fires whenever the selected filter changes.
    /// The callback receives `None` for "All" or `Some(ContentClass)` for a specific class.
    pub fn connect_filter_changed(&self, callback: impl Fn(Option<ContentClass>) + 'static) {
        let callback = Rc::new(callback);
        let active = self.active_filter.clone();

        let wire_button =
            |btn: &gtk::ToggleButton, class: Option<ContentClass>| {
                let cb = callback.clone();
                let active = active.clone();
                btn.connect_toggled(move |b| {
                    if b.is_active() {
                        *active.borrow_mut() = class;
                        cb(class);
                    }
                });
            };

        wire_button(&self.btn_all, None);
        wire_button(&self.btn_text, Some(ContentClass::Text));
        wire_button(&self.btn_images, Some(ContentClass::Image));
        wire_button(&self.btn_files, Some(ContentClass::Files));
        wire_button(&self.btn_links, Some(ContentClass::Url));
    }

    /// Show or hide filter tab buttons based on which content classes have
    /// entries.  The "All" tab is always visible.  If the currently active
    /// filter becomes hidden, automatically switch back to "All".
    pub fn update_visible_tabs(&self, counts: &HashMap<ContentClass, u32>) {
        let has_text = counts.get(&ContentClass::Text).copied().unwrap_or(0)
            + counts.get(&ContentClass::RichText).copied().unwrap_or(0);
        let has_images = counts.get(&ContentClass::Image).copied().unwrap_or(0);
        let has_files = counts.get(&ContentClass::Files).copied().unwrap_or(0);
        let has_links = counts.get(&ContentClass::Url).copied().unwrap_or(0);

        self.btn_text.set_visible(has_text > 0);
        self.btn_images.set_visible(has_images > 0);
        self.btn_files.set_visible(has_files > 0);
        self.btn_links.set_visible(has_links > 0);

        // If the currently active filter tab has been hidden, revert to "All".
        let active = *self.active_filter.borrow();
        let active_hidden = match active {
            None => false, // "All" is always visible
            Some(ContentClass::Text) | Some(ContentClass::RichText) => has_text == 0,
            Some(ContentClass::Image) => has_images == 0,
            Some(ContentClass::Files) => has_files == 0,
            Some(ContentClass::Url) => has_links == 0,
        };
        if active_hidden {
            self.btn_all.set_active(true);
            *self.active_filter.borrow_mut() = None;
        }
    }

    /// Programmatically select a filter tab.
    pub fn set_active(&self, class: Option<ContentClass>) {
        match class {
            None => self.btn_all.set_active(true),
            Some(ContentClass::Text) | Some(ContentClass::RichText) => {
                self.btn_text.set_active(true);
            }
            Some(ContentClass::Image) => self.btn_images.set_active(true),
            Some(ContentClass::Files) => self.btn_files.set_active(true),
            Some(ContentClass::Url) => self.btn_links.set_active(true),
        }
        *self.active_filter.borrow_mut() = class;
    }
}
