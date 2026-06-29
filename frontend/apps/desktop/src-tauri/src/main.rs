// Prevent a console window on Windows release builds. No-op elsewhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    myelin_desktop_lib::run();
}
