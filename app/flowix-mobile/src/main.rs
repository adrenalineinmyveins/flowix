// 桌面调试入口；安卓走 lib.rs 里的 mobile_entry_point。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    flowix_mobile::run()
}
