use std::fs;
use std::path::{Path, PathBuf};

fn collect_template_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        panic!(
            "failed to read template directory {}: {error}",
            directory.display()
        )
    });

    for entry in entries {
        let entry = entry.expect("failed to read template directory entry");
        let path = entry.path();
        let file_type = entry
            .file_type()
            .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", path.display()));
        if file_type.is_dir() {
            collect_template_files(root, &path, files);
        } else if file_type.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

fn generate_notebook_template_assets() {
    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let template_root = manifest_dir.join("../../.flowix/templates/notebook-templates");
    let template_root = template_root
        .canonicalize()
        .unwrap_or_else(|error| panic!("notebook template source is unavailable: {error}"));
    println!("cargo:rerun-if-changed={}", template_root.display());

    let mut files = Vec::new();
    collect_template_files(&template_root, &template_root, &mut files);
    files.sort();

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let generated_file = out_dir.join("notebook_template_assets.rs");
    let mut generated = String::from("pub const NOTEBOOK_TEMPLATE_ASSETS: &[(&str, &[u8])] = &[\n");
    for relative in files {
        let absolute = template_root.join(&relative);
        generated.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            relative.to_string_lossy().replace('\\', "/"),
            absolute.to_string_lossy(),
        ));
    }
    generated.push_str("];\n");
    fs::write(generated_file, generated).expect("failed to write generated template asset list");
}

fn main() {
    generate_notebook_template_assets();
    let dev_mode = std::env::var_os("FLOWIX_DEV_MODE").is_some();
    println!("cargo:rerun-if-env-changed=FLOWIX_DEV_MODE");
    println!("cargo:rustc-env=FLOWIX_DEV_MODE={}", if dev_mode { "1" } else { "0" });

    // `tauri dev` launches the macOS executable directly instead of an
    // `.app` bundle, so the bundle plist configured in tauri.conf.json is
    // unavailable to AppKit/WebKit while developing. Embed the localization
    // metadata in the executable as well; this is also harmless for release
    // builds, whose packaged bundle has its own generated Info.plist.
    #[cfg(target_os = "macos")]
    {
        let manifest_dir = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"),
        );
        let info_plist = if dev_mode {
            let source = std::fs::read_to_string(manifest_dir.join("Info.plist"))
                .expect("failed to read Info.plist");
            let dev = source.replace("<string>flowix</string>", "<string>flowix-dev</string>")
                .replace("com.flowix.app.deeplink", "com.flowix.app.dev.deeplink");
            let generated = std::path::PathBuf::from(
                std::env::var_os("OUT_DIR").expect("OUT_DIR is set"),
            ).join("Flowix-Dev-Info.plist");
            std::fs::write(&generated, dev).expect("failed to write dev Info.plist");
            generated
        } else {
            manifest_dir.join("Info.plist")
        };
        println!("cargo:rerun-if-changed={}", info_plist.display());
        println!("cargo:rustc-link-arg-bin=flowix-desktop=-sectcreate");
        println!("cargo:rustc-link-arg-bin=flowix-desktop=__TEXT");
        println!("cargo:rustc-link-arg-bin=flowix-desktop=__info_plist");
        println!(
            "cargo:rustc-link-arg-bin=flowix-desktop={}",
            info_plist.display()
        );
    }

    // cargo 的 lib 单测二进制不带 tauri-build 为 bin 嵌入的 manifest，Windows
    // 上装载器会把 comctl32.dll 解析到 v5（缺 TaskDialogIndirect /
    // SetWindowSubclass 入口点），lib test 启动即 STATUS_ENTRYPOINT_NOT_FOUND。
    // rustc-link-arg 覆盖所有目标（含单测）；对 bin 而言内嵌 manifest 优先，
    // 此处追加的依赖声明无行为影响。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' \
             name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
             publicKeyToken='6595b64144ccf1df' language='*' processorArchitecture='*'"
        );
    }

    tauri_build::build()
}
