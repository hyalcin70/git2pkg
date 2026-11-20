//! Eingebettete Schriftarten mit vollständiger Abdeckung für
//! Deutsch, Türkisch, Spanisch und Französisch.
//!
//! Noto Sans und Noto Sans Mono enthalten alle türkischen Sonderzeichen
//! (İ, ı, ğ, Ğ, ş, Ş, ç, ö, ü) sowie die spanischen und französischen
//! Akzente. Das Einbetten macht die Darstellung unabhängig von den auf dem
//! System installierten Fonts.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

const PROPORTIONAL_NAME: &str = "NotoSans";
const MONOSPACE_NAME: &str = "NotoSansMono";

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        PROPORTIONAL_NAME.to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/NotoSans-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        MONOSPACE_NAME.to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../assets/NotoSansMono-Regular.ttf"
        ))),
    );

    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, PROPORTIONAL_NAME.to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, MONOSPACE_NAME.to_owned());

    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    #[test]
    fn fonts_sind_eingebettet() {
        assert!(include_bytes!("../../assets/NotoSans-Regular.ttf").len() > 100_000);
        assert!(include_bytes!("../../assets/NotoSansMono-Regular.ttf").len() > 100_000);
    }
}
