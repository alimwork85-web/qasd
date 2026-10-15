fn main() {
    // The app icon and manifest (DPI awareness, modern controls), in both
    // qasd.exe and qasdw.exe.
    embed_resource::compile("assets/qasd.rc", embed_resource::NONE).manifest_optional().unwrap();
}
