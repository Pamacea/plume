fn main() {
    // icône de l'exé (explorateur, raccourcis, tray) — rc.exe du Windows SDK
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/plume.ico");
    res.set("ProductName", "Plume");
    res.set("FileDescription", "Plume — assistant d'écriture français");
    res.compile()
        .expect("ressource Windows : rc.exe introuvable (Windows SDK)");
}
