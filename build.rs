use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");

    println!("cargo:rerun-if-env-changed=NGTERM_SKIP_FRONTEND_BUILD");

    // Docker / CI build the frontend in a separate stage and copy `dist` in;
    // cargo overrides PROFILE for build scripts, so an explicit opt-out is
    // the only reliable way to skip the pnpm step.
    let skip = std::env::var("NGTERM_SKIP_FRONTEND_BUILD").is_ok_and(|v| v == "1");
    if skip {
        if !std::path::Path::new("frontend/dist/index.html").exists() {
            panic!(
                "NGTERM_SKIP_FRONTEND_BUILD=1 but frontend/dist/index.html is missing; \
                 build the frontend first or unset the variable"
            );
        }
        return;
    }

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
