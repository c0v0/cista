// Windows 下隐藏控制台窗口；Linux/macOS 无影响。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    cista_desktop_lib::run()
}
