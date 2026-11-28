//! The gates check the checkout `cargo xtask` runs in, even when another checkout (a git worktree
//! sharing the target directory) built the binary: they read `CARGO_MANIFEST_DIR` as `cargo run`
//! sets it, not the directory compiled in (this fork, TAB.WINDOW.1 W3).

#[test]
fn the_gates_read_the_checkout_cargo_names_at_run_time() {
    let fake = std::env::temp_dir().join(format!("xtask-checkout-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&fake);
    std::fs::create_dir_all(fake.join("xtask").join("src")).unwrap();
    std::fs::write(fake.join("xtask").join("Cargo.toml"), "").unwrap();
    std::fs::write(fake.join("xtask").join("src").join("gates.rs"), "").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("assets")
        .env("CARGO_MANIFEST_DIR", fake.join("xtask"))
        .current_dir(&fake)
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    // The fake checkout has no ATTRIBUTION.toml: the gate must fail on it, naming it, rather than
    // check the checkout this binary was built in.
    assert!(!out.status.success(), "checked another checkout: {text}");
    assert!(text.contains(&fake.join("ATTRIBUTION.toml").display().to_string()), "{text}");
    let _ = std::fs::remove_dir_all(&fake);
}
