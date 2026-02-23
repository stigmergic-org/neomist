use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

const UI_PATHS: &[&str] = &[
    "assets/icon.svg",
    "assets/icon-dark.svg",
    "ui/src",
    "ui/public",
    "ui/index.html",
    "ui/package.json",
    "ui/package-lock.json",
    "ui/tailwind.config.js",
    "ui/postcss.config.js",
    "ui/vite.config.js",
];

const UI_ASSETS: &[(&str, &str)] = &[
    ("assets/icon.svg", "ui/public/icon.svg"),
    ("assets/icon-dark.svg", "ui/public/icon-dark.svg"),
];

fn main() {
    for path in UI_PATHS {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=NEOMIST_SKIP_UI_BUILD");

    if env::var_os("NEOMIST_SKIP_UI_BUILD").is_some() {
        println!("cargo:warning=Skipping UI build (NEOMIST_SKIP_UI_BUILD set)");
        return;
    }

    let ui_dir = Path::new("ui");
    let dist_index = ui_dir.join("dist").join("index.html");

    let latest_input = latest_input_mtime();
    let dist_mtime = file_mtime(&dist_index);

    let needs_build = match (latest_input, dist_mtime) {
        (Some(input_time), Some(dist_time)) => input_time > dist_time,
        (Some(_), None) => true,
        _ => !dist_index.exists(),
    };

    let needs_build = needs_build || UI_ASSETS.iter().any(|(_, dest)| !Path::new(dest).exists());

    if !needs_build {
        return;
    }

    for (source, dest) in UI_ASSETS {
        sync_asset(Path::new(source), Path::new(dest));
    }

    if needs_npm_install(ui_dir) {
        run_npm(ui_dir, &["install"]);
    }
    run_npm(ui_dir, &["run", "build"]);

    if !dist_index.exists() {
        panic!("UI build failed: dist/index.html missing");
    }
}

fn latest_input_mtime() -> Option<SystemTime> {
    let mut latest = None;
    for path in UI_PATHS {
        let path = Path::new(path);
        if path.is_dir() {
            latest = max_time(latest, dir_mtime(path));
        } else {
            latest = max_time(latest, file_mtime(path));
        }
    }

    latest
}

fn max_time(current: Option<SystemTime>, next: Option<SystemTime>) -> Option<SystemTime> {
    match (current, next) {
        (Some(a), Some(b)) => Some(if a > b { a } else { b }),
        (None, Some(b)) => Some(b),
        (Some(a), None) => Some(a),
        (None, None) => None,
    }
}

fn dir_mtime(dir: &Path) -> Option<SystemTime> {
    let mut latest = file_mtime(dir);
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|name| name == "node_modules" || name == "dist")
        {
            continue;
        }
        if path.is_dir() {
            latest = max_time(latest, dir_mtime(&path));
        } else {
            latest = max_time(latest, file_mtime(&path));
        }
    }
    latest
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

fn needs_npm_install(ui_dir: &Path) -> bool {
    let node_modules = ui_dir.join("node_modules");
    if !node_modules.exists() {
        return true;
    }

    let lock_path = ui_dir.join("package-lock.json");
    let lock_mtime = file_mtime(&lock_path);
    let node_mtime = file_mtime(&node_modules);

    match (lock_mtime, node_mtime) {
        (Some(lock), Some(node)) => lock > node,
        (Some(_), None) => true,
        _ => false,
    }
}

fn run_npm(ui_dir: &Path, args: &[&str]) {
    let npm = if cfg!(target_os = "windows") {
        "npm.cmd"
    } else {
        "npm"
    };
    let status = Command::new(npm)
        .args(args)
        .current_dir(ui_dir)
        .status()
        .expect("Failed to run npm");

    if !status.success() {
        panic!("npm command failed: {args:?}");
    }
}

fn sync_asset(source: &Path, dest: &Path) {
    let source_bytes =
        fs::read(source).unwrap_or_else(|err| panic!("Failed to read {source:?}: {err}"));
    let needs_copy = match fs::read(dest) {
        Ok(dest_bytes) => dest_bytes != source_bytes,
        Err(_) => true,
    };

    if needs_copy {
        if let Some(parent) = dest.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                panic!("Failed to create {parent:?}: {err}");
            }
        }
        if let Err(err) = fs::write(dest, source_bytes) {
            panic!("Failed to write {dest:?}: {err}");
        }
    }
}
