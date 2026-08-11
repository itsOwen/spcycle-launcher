// keeps a console window from flashing behind the launcher on windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    spcycle_lib::run()
}
