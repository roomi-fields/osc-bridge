//! The consent that names ports: by index or by a part of the name.

use super::*;

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_consent_fragment_names_one_port_by_a_part_of_its_name_in_any_case() {
    let names = strings(&["Minilab3 MIDI 40:0", "Digitakt 41:0"]);
    assert_eq!(consent_indexes(&names, &strings(&["digi"])).unwrap(), vec![1]);
    assert_eq!(consent_indexes(&names, &strings(&["MINILAB", "digitakt"])).unwrap(), vec![0, 1]);
    assert!(consent_indexes(&names, &[]).unwrap().is_empty());
}

#[test]
fn an_exact_index_names_one_port() {
    let names = strings(&["Box A 20:0", "Box B 21:0", "Box C 22:0"]);
    assert_eq!(consent_indexes(&names, &strings(&["1"])).unwrap(), vec![1]);
    assert_eq!(consent_indexes(&names, &strings(&["0", "2"])).unwrap(), vec![0, 2]);
}

#[test]
fn a_consent_fragment_matching_several_ports_is_refused_naming_them() {
    let names = strings(&["Box A 20:0", "Box B 21:0", "Digitakt 22:0"]);
    let err = consent_indexes(&names, &strings(&["box"])).expect_err("ambiguous");
    assert!(err.contains("Box A 20:0") && err.contains("Box B 21:0") && !err.contains("Digitakt"), "{err}");
}

#[test]
fn a_consent_fragment_matching_no_port_is_refused_naming_it_and_the_ports_there_are() {
    let names = strings(&["Box A 20:0", "Digitakt 22:0"]);
    for bad in ["zzz", "7"] {
        let err = consent_indexes(&names, &strings(&[bad])).expect_err("no port");
        assert!(err.contains(bad) && err.contains("Box A 20:0") && err.contains("Digitakt 22:0"), "{err}");
    }
}

#[test]
fn one_bad_consent_fragment_refuses_the_whole_list() {
    let names = strings(&["Box A 20:0", "Digitakt 22:0"]);
    assert!(consent_indexes(&names, &strings(&["digi", "nothing"])).is_err());
}
