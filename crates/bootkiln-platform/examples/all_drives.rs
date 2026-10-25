use bootkiln_core::DiskBackend;
fn main() {
    let b = bootkiln_platform::NativeBackend;
    for d in b.list_drives().unwrap() {
        println!("{d:?}");
    }
    for p in std::env::args().skip(1) {
        println!("{p}: {:?}", b.disk_numbers_for_path(std::path::Path::new(&p)));
    }
    println!("locale {:?} elevated {}", bootkiln_platform::host_locale(), b.is_elevated());
}
