fn main() {
    println!("cargo:rerun-if-changed=../../assets/faint-light-eye.ico");

    // Explorer, the taskbar and Alt-Tab read the icon from the executable's
    // resource section, not from anything the program does at run time (the
    // GUI separately sets the same artwork on its window from the PNG).
    // A missing resource compiler is not worth failing a build over.
    #[cfg(windows)]
    {
        let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/faint-light-eye.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon(&ico.to_string_lossy());
        res.set("ProductName", "Faint Light");
        res.set("FileDescription", "Faint Light plate solver");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded (no resource compiler?): {e}");
        }
    }
}
