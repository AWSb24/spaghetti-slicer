#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = slicer_formats::parse_stl(data);
});
