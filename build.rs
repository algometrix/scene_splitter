//! Puts the Cutaway icon and its name into scene_splitter.exe, so Explorer,
//! the taskbar and shortcuts show the mark instead of a blank program.

const ICON: &str = "assets/icon.ico";

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(ICON)
        .set("ProductName", "Cutaway")
        .set("FileDescription", "Cutaway");
    // A build script has no other way to report why it failed.
    resource.compile().unwrap_or_else(|error| {
        panic!("embed the icon in scene_splitter.exe (needs the Windows SDK's rc.exe): {error}")
    });
}
