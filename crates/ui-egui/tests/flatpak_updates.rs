// Modified by the PdfCraft Overlay fork (sefr-14/pdfcraft); changes are recorded in its Git history.
//! Release and source Flatpaks must agree on their permissions. Upstream's also allow release
//! checks and its host update helper (craft-update); this fork leaves craft-update out (SYNC.13),
//! so its Flatpaks update through flatpak and are granted neither (the manifests say why).
fn finish_args(manifest: &str) -> Vec<&str> {
    manifest
        .split("finish-args:")
        .nth(1)
        .unwrap_or_default()
        .lines()
        .take_while(|line| line.is_empty() || line.starts_with(' ') || line.starts_with('#'))
        .filter_map(|line| line.trim().strip_prefix("- "))
        .map(|arg| arg.trim_matches(['\'', '"']))
        .collect()
}

#[test]
fn flatpak_update_permissions() {
    let manifests = [
        include_str!("../../../packaging/linux/flatpak/ai.storyteller.pdfcraft.bundle.yml"),
        include_str!("../../../packaging/linux/flatpak/ai.storyteller.pdfcraft.yml"),
    ];
    let args = manifests.map(finish_args);
    assert_eq!(args[0], args[1], "source and release permissions must agree");
    // This fork (SYNC.17): upstream's updater permissions stay out with its updater.
    for permissions in args {
        for upstream_only in ["--share=network", "--talk-name=org.freedesktop.Flatpak"] {
            assert!(!permissions.contains(&upstream_only), "upstream's update permission {upstream_only} without its updater");
        }
    }
}
