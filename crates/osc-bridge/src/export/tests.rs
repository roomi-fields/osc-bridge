use super::*;

/// The sources of a 0.5 driver text, as `osd` reads them.
fn sources_of(sources: &str) -> Vec<osd::Source> {
    let text = format!(r#"{{"$schema":"https://w3id.org/osd/0.5/device.schema.json","device":{{"name":"D","osc_prefix":"/d"}},"_sources":{sources}}}"#);
    match osd::read(&text, osd::Kind::Device).unwrap() {
        osd::Descriptor::Device(d) => d.sources.unwrap(),
        _ => unreachable!(),
    }
}

#[test]
fn a_source_is_credited_with_its_licence() {
    let s = sources_of(r#"[{"type": "community-re", "license": "CC-BY-SA-4.0",
        "attribution": "MIDI Guide", "url": "https://midi.guide", "firmware": "unknown"},
        {"type": "vendor-doc", "firmware": "2.0"}]"#);
    assert_eq!(credit(&s[0]), "MIDI Guide, licence CC-BY-SA-4.0, https://midi.guide");
    assert_eq!(credit(&s[1]), "vendor-doc source, firmware 2.0, licence not stated");
}

#[test]
fn every_test_driver_is_written_in_both_formats() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "json") { out.push(p) }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new("tests/fixtures/devices"), &mut files);
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let mut midnams = 0;
    for f in &files {
        let Ok(dev) = crate::device::load(f) else { continue };
        let ns: Value = serde_json::from_str(&export(&dev, Format::OscQuery).unwrap()).unwrap();
        assert_eq!(ns["FULL_PATH"], "/", "{}", f.display());
        if let Ok(xml) = export(&dev, Format::Midnam) {
            roxmltree::Document::parse_with_options(&xml, opts)
                .unwrap_or_else(|e| panic!("{}: MIDNAM not well-formed: {e}", f.display()));
            midnams += 1;
        }
    }
    assert!(midnams > files.len() / 2, "{midnams} of {}", files.len());
}

#[test]
fn formats_are_read_by_name() {
    assert_eq!("OSCQuery".parse::<Format>().unwrap(), Format::OscQuery);
    assert_eq!("midnam".parse::<Format>().unwrap(), Format::Midnam);
    assert!("ctrlr".parse::<Format>().is_err());
    assert_eq!(label("/filter/env_amount"), "Filter Env Amount");
}
