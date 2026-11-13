#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let mut warnings = slicer_core::Diagnostics::new();
    let _ = slicer_formats::parse_3mf_plate(data, None, &mut warnings);
});
