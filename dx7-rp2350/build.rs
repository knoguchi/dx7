fn main() {
    // Make memory.x available to the linker
    println!(
        "cargo:rustc-link-search={}",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    );
    println!("cargo:rerun-if-changed=memory.x");

    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");

    #[cfg(feature = "ble-midi")]
    cyw43_firmware::ensure_present();
}

/// CYW43439 firmware blobs for the Pico 2 W radio.
///
/// The blobs are committed under `cyw43-firmware/`; this only downloads when
/// one is missing. The download is pinned to a specific embassy commit and
/// verified against the SHA-256 of the committed copies, so a moved file, a
/// changed upstream blob or an HTML error page can never end up embedded as
/// radio firmware.
#[cfg(feature = "ble-midi")]
mod cyw43_firmware {
    use sha2::{Digest, Sha256};
    use std::path::Path;

    /// embassy-rs/embassy commit the blobs were taken from.
    const EMBASSY_COMMIT: &str = "047ddf53e02d3077673fb2504b710331caf1a92c";

    /// (file name, SHA-256 of the committed blob — `shasum -a 256`).
    const FILES: [(&str, &str); 3] = [
        (
            "43439A0.bin",
            "5555e0261da2610a500d68c18d895cace0152bbefbf76f4aa683ebce77e3d7eb",
        ),
        (
            "43439A0_btfw.bin",
            "ce1992c1a6a16ae51bc012439486e9fb212623eca92d9e82a8090c2acf7ef1df",
        ),
        (
            "43439A0_clm.bin",
            "e712b3d218e8b1e2747b092e03b8b0afcb8c8c8e355d2a4a0d47b493800f3f89",
        ),
    ];

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut s = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write;
            write!(s, "{:02x}", b).unwrap();
        }
        s
    }

    pub fn ensure_present() {
        let fw_dir =
            Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("cyw43-firmware");
        std::fs::create_dir_all(&fw_dir).unwrap();

        let client = reqwest::blocking::Client::new();

        for (file, expected) in FILES {
            let dest = fw_dir.join(file);
            println!("cargo:rerun-if-changed={}", dest.display());
            if dest.exists() {
                continue;
            }

            let url = format!(
                "https://raw.githubusercontent.com/embassy-rs/embassy/{}/cyw43-firmware/{}",
                EMBASSY_COMMIT, file
            );
            println!("cargo:warning=Downloading CYW43 firmware {} from {}", file, url);

            let bytes = client
                .get(&url)
                .send()
                .and_then(|resp| resp.error_for_status())
                .and_then(|resp| resp.bytes())
                .unwrap_or_else(|e| panic!("Failed to download {}: {}", file, e));

            let actual = sha256_hex(&bytes);
            if actual != expected {
                panic!(
                    "SHA-256 mismatch for {}:\n  expected {}\n  got      {}\n\
                     Refusing to write it. Check EMBASSY_COMMIT / FILES in build.rs.",
                    file, expected, actual
                );
            }

            std::fs::write(&dest, &bytes).unwrap();
        }
    }
}
