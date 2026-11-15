#![no_main]

// The input is the model part of a package that is valid otherwise, so the
// reader of the model is reached every time and not only when the fuzzer
// happens to produce a valid archive.
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(model) = std::str::from_utf8(data) else {
        return;
    };
    let types = "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>";
    let Ok(package) = slicer_formats::write_package(&[
        ("[Content_Types].xml", types),
        ("3D/3dmodel.model", model),
    ]) else {
        return;
    };
    let mut warnings = slicer_core::Diagnostics::new();
    let _ = slicer_formats::parse_3mf_plate(&package, None, &mut warnings);
});
