//! macOS reads the icon from an app bundle. A bare binary from `cargo run`
//! has none, so set the Dock icon at runtime from the embedded icns.

const ICON: &[u8] = include_bytes!("../assets/AppIcon.icns");

#[cfg(target_os = "macos")]
pub fn set_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(ICON);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else { return };

    let app = NSApplication::sharedApplication(mtm);
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(not(target_os = "macos"))]
pub fn set_icon() {
    let _ = ICON;
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use objc2::AllocAnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSData;

    #[test]
    fn icon_decodes() {
        let data = NSData::with_bytes(super::ICON);
        let image = NSImage::initWithData(NSImage::alloc(), &data).expect("icns decodes");
        assert!(image.size().width > 0.);
    }
}
