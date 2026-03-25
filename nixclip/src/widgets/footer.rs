use gtk::prelude::*;
use gtk4 as gtk;

pub struct Footer {
    pub container: gtk::Box,
    result_count_label: gtk::Label,
}

impl Footer {
    pub fn new() -> Self {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        container.set_margin_top(6);
        container.set_margin_bottom(6);
        container.set_margin_start(12);
        container.set_margin_end(12);

        let hints = gtk::Label::new(Some(
            "Enter = restore  \u{2502}  \u{21e7}Enter = plain  \u{2502}  \u{232b} = delete  \u{2502}  ^P = pin  \u{2502}  ^, = settings",
        ));
        hints.add_css_class("dim-label");
        hints.set_halign(gtk::Align::Center);
        hints.set_hexpand(true);

        let result_count_label = gtk::Label::new(None);
        result_count_label.add_css_class("dim-label");
        result_count_label.add_css_class("caption");
        result_count_label.set_halign(gtk::Align::End);
        result_count_label.set_visible(false);

        container.append(&hints);
        container.append(&result_count_label);

        Self {
            container,
            result_count_label,
        }
    }

    pub fn set_result_count(&self, shown: usize, total: u32) {
        if total == 0 {
            self.result_count_label.set_visible(false);
            return;
        }
        self.result_count_label
            .set_label(&format!("Showing {shown} of {total}"));
        self.result_count_label.set_visible(true);
    }

    pub fn clear_result_count(&self) {
        self.result_count_label.set_visible(false);
    }
}
