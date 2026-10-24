fn main() {
    // Windows: embed the icon and version info into the .exe (host = Windows only).
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icons/ProArt.ico");
        res.set("ProductName", "ProArt");
        res.set("FileDescription", "ProArt - ASUS ProArt monitor control");
        let _ = res.compile();
    }
    println!("cargo:rerun-if-changed=../../assets/icons/ProArt.ico");
}
