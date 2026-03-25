use gtk4 as gtk;
use gtk::prelude::*;

/// Build the keyboard-hints footer label shown at the bottom of the popup.
pub fn build_footer() -> gtk::Label {
    let label = gtk::Label::new(Some(
        "Enter = restore  \u{2502}  \u{21e7}Enter = plain  \u{2502}  \u{232b} = delete  \u{2502}  ^P = pin  \u{2502}  ^, = settings",
    ));
    label.add_css_class("dim-label");
    label.set_halign(gtk::Align::Center);
    label.set_margin_top(6);
    label.set_margin_bottom(6);
    label.set_margin_start(12);
    label.set_margin_end(12);
    label
}
