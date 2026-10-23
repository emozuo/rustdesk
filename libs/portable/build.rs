fn main() {
    #[cfg(windows)]
    {
        use std::io::Write;
        let mut res = winres::WindowsResource::new();
        res.set_icon("../../res/icon.ico")
            .set_language(winapi::um::winnt::MAKELANGID(
                winapi::um::winnt::LANG_ENGLISH,
                winapi::um::winnt::SUBLANG_ENGLISH_US,
            ))
            .set_manifest_file("../../res/manifest.xml");
        // 产品版（dd-brand）的安装包：版本信息里的产品名 / 原始文件名改成我们的，按原始文件名匹配的安全软件规则才不会把它当 rustdesk.exe
        if std::env::var_os("CARGO_FEATURE_DD_BRAND").is_some() {
            res.set("ProductName", "DreamDesk")
                .set("FileDescription", "DreamDesk Remote Desktop")
                .set("OriginalFilename", "dreamdesk-pro.exe");
        }
        match res.compile() {
            Err(e) => {
                write!(std::io::stderr(), "{}", e).unwrap();
                std::process::exit(1);
            }
            Ok(_) => {}
        }
    }
}
