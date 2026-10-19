fn main() {
    use sha2::{Digest,Sha256};
    println!("cargo:rerun-if-changed=../frontend-dist");
    let manifest=std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let original=manifest.join("../../original/local-planner.exe");
    println!("cargo:rerun-if-changed={}",original.display());
    let generated=if original.is_file() {
        let bytes=std::fs::read(&original).expect("Cannot read owner's original executable");
        let actual=format!("{:x}",Sha256::digest(&bytes));
        assert_eq!(actual,"7cd7820f7cae063982c30d3cbd9901152b8eee12173470ad830a322e43830dbd","Original executable identity mismatch");
        let hash=std::str::from_utf8(&bytes[13019636..13019668]).expect("Invalid original client configuration");
        assert!(hash.bytes().all(|c|c.is_ascii_hexdigit()),"Invalid original client hash");
        // API ID corroborated at three instruction locations in the same authenticated EXE.
        // The original client hash is read locally during compilation and never printed.
        format!("pub const ORIGINAL_API_ID:i32=38_275_116;pub const ORIGINAL_API_HASH:Option<&str>=Some({hash:?});")
    } else {
        println!("cargo:warning=Original executable absent; configure SFERY_TELEGRAM_API_ID and SFERY_TELEGRAM_API_HASH at runtime.");
        "pub const ORIGINAL_API_ID:i32=0;pub const ORIGINAL_API_HASH:Option<&str>=None;".to_owned()
    };
    std::fs::write(std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("original_client.rs"),generated).unwrap();
    tauri_build::build()
}
