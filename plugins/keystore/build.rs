fn main() {
    tauri_plugin::Builder::new(&["secret_save", "secret_load", "secret_delete"])
        .android_path("android")
        .build();
}
