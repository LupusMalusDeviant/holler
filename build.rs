//! Erzeugt das Exe-Icon aus src/icon.rs und bettet es samt Versionsinfo ein.

#[path = "src/icon.rs"]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let ico = std::path::Path::new(&out).join("holler.ico");
    std::fs::write(&ico, icon::ico(&[16, 24, 32, 48, 64, 128, 256], icon::GREEN)).expect("Icon schreiben");
    // Kopie ins Repo, damit sie auch ausserhalb des Builds verwendbar ist.
    let _ = std::fs::create_dir_all("assets");
    let _ = std::fs::write("assets/holler.ico", icon::ico(&[16, 24, 32, 48, 64, 128, 256], icon::GREEN));
    winresource::WindowsResource::new()
        .set_icon(ico.to_str().expect("Pfad"))
        .set("ProductName", "Holler")
        .set("FileDescription", "Holler – LAN-Funk ohne Umwege")
        .set("CompanyName", "Lupus Malus Deviant")
        .set("LegalCopyright", "(c) 2026 Lupus Malus Deviant")
        .set("OriginalFilename", "holler.exe")
        .set("InternalName", "holler")
        .compile()
        .expect("Ressourcen einbetten");
}
