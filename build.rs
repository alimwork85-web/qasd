// The app icon, the manifest (DPI awareness, modern controls) and the
// version information Windows shows in a file's Properties > Details
// (company, copyright, product, version), in both qasd.exe and qasdw.exe.
// The resource script is written here so the version follows Cargo.toml.

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version.split(['.', '-']).filter_map(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets");
    let path = |f: &str| dir.join(f).display().to_string().replace('\\', "\\\\");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {v0},{v1},{v2},{v3}
PRODUCTVERSION {v0},{v1},{v2},{v3}
FILEFLAGSMASK 0x3f
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Ali Alhumedi"
      VALUE "FileDescription", "Qasd - whole-disk file search"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "qasd"
      VALUE "LegalCopyright", "Copyright (C) 2026 Ali Alhumedi"
      VALUE "ProductName", "Qasd"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = path("qasd.ico"),
        manifest = path("qasd.manifest"),
        v0 = parts[0],
        v1 = parts[1],
        v2 = parts[2],
        v3 = parts[3],
    );
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("qasd.rc");
    std::fs::write(&out, rc).unwrap();
    println!("cargo:rerun-if-changed=assets/qasd.ico");
    println!("cargo:rerun-if-changed=assets/qasd.manifest");
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
}
