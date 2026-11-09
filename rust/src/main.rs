//! Tools PDF: barra laterale per comprimere, dividere, unire documenti e renderne selezionabile il testo.
#![cfg_attr(not(test), windows_subsystem = "windows")]

mod settings;
mod tools;
mod ui;
mod update;

fn main() {
    ui::run(std::env::args_os().skip(1).collect());
}
