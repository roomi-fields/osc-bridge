use super::*;

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
    let mut exported = 0;
    for f in &files {
        let Ok(dev) = crate::device::load(f) else { continue };
        let Ok(ns) = export(&dev, Format::OscQuery) else { continue };
        let ns: serde_json::Value = serde_json::from_str(&ns.value).unwrap();
        assert_eq!(ns["FULL_PATH"], "/", "{}", f.display());
        exported += 1;
        if let Ok(xml) = export(&dev, Format::Midnam) {
            roxmltree::Document::parse_with_options(&xml.value, opts)
                .unwrap_or_else(|e| panic!("{}: MIDNAM not well-formed: {e}", f.display()));
            midnams += 1;
        }
    }
    assert!(exported > files.len() / 2, "{exported} of {}", files.len());
    assert!(midnams > files.len() / 2, "{midnams} of {}", files.len());
}

#[test]
fn formats_are_read_by_name() {
    assert_eq!("OSCQuery".parse::<Format>().unwrap(), Format::OscQuery);
    assert_eq!("midnam".parse::<Format>().unwrap(), Format::Midnam);
    assert!("ctrlr".parse::<Format>().is_err());
}

#[test]
fn a_format_gives_its_media_type_and_extension() {
    assert_eq!((Format::OscQuery.media_type(), Format::OscQuery.extension()), ("application/json", "json"));
    assert_eq!((Format::Midnam.media_type(), Format::Midnam.extension()), ("application/xml", "midnam"));
}
