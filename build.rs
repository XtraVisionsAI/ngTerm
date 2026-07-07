use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");

    if std::env::var("PROFILE").unwrap_or_default() == "release" {
        let status = Command::new("pnpm")
            .args(["--dir", "frontend", "build"])
            .status()
            .expect("Failed to run pnpm build. Is pnpm installed?");

        if !status.success() {
            panic!("Frontend build failed");
        }
    }
}
