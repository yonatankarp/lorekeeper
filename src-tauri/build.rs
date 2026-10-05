fn main() {
    println!("cargo:rerun-if-env-changed=LOREKEEPER_GOOGLE_CLIENT_SECRET");
    tauri_build::build()
}
