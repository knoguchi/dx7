fn main() {
    // For QEMU builds, override linker aliases to put all code/rodata in RAM
    // instead of flash-mapped addresses (which require cache/MMU setup that
    // QEMU's ROM bootloader can't do).
    #[cfg(feature = "qemu")]
    {
        let out = std::env::var("OUT_DIR").unwrap();
        std::fs::write(
            format!("{out}/alias.x"),
            r#"
            REGION_ALIAS("ROTEXT", iram_seg);
            REGION_ALIAS("RWTEXT", iram_seg);
            REGION_ALIAS("RODATA", dram_seg);
            REGION_ALIAS("RWDATA", dram_seg);
            REGION_ALIAS("RTC_FAST_RWTEXT", rtc_fast_seg);
            REGION_ALIAS("RTC_FAST_RWDATA", rtc_fast_seg);
            "#,
        )
        .unwrap();
        println!("cargo:rustc-link-search={out}");
    }
}
