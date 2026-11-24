fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/rustify.ico");
        res.set("ProductName", "Rustify");
        res.set("FileDescription", "Rustify");
        res.set("CompanyName", "Ilay-dev");
        res.set("OriginalFilename", "Rustify.exe");
        res.set("InternalName", "Rustify");
        res.set("LegalCopyright", "Copyright (c) Ilay-dev");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Failed to compile Windows resources: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/rustify.ico");
    println!("cargo:rerun-if-changed=assets/rustify_256.png");
}
