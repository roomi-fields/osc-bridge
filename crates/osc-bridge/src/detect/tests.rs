use super::*;

mod consent;

#[test]
fn an_input_is_paired_with_its_device_output() {
    let outs = vec!["Midi Through Port-0".to_string(), "minilogue xd MIDI 2".to_string()];
    assert_eq!(output_for("minilogue xd MIDI 1", &outs).as_deref(), Some("minilogue xd MIDI 2"));
    assert_eq!(output_for("Midi Through Port-0", &outs).as_deref(), Some("Midi Through Port-0"));
    assert_eq!(output_for("TR-8S", &outs), None);
    assert_eq!(output_for("TR-8S", &outs[1..]).as_deref(), Some("minilogue xd MIDI 2"), "the only output");
}

const CARDS: &str = " 0 [PCH            ]: HDA-Intel - HDA Intel PCH
                      HDA Intel PCH at 0xf7e10000 irq 130
 1 [Minilab3       ]: USB-Audio - Minilab3
                      Arturia Minilab3 at usb-0000:00:14.0-2, full speed
 2 [Minilab3_1     ]: USB-Audio - Minilab3
                      Arturia Minilab3 at usb-0000:00:14.0-3, full speed
 3 [Pad            ]: USB-Audio - Pad
                      Acme Pad at usb-0000:00:14.0-4, full speed
";

fn cards() -> Vec<(String, u16, u16)> {
    usb_cards(CARDS, &[(1, "1c75:0288\n"), (2, "1c75:0288\n"), (3, "1234:5678\n")])
}

#[test]
fn usb_cards_lists_the_short_name_and_ids_of_each_usb_card() {
    assert_eq!(cards(), vec![
        ("Minilab3".to_string(), 0x1c75, 0x0288),
        ("Minilab3".to_string(), 0x1c75, 0x0288),
        ("Pad".to_string(), 0x1234, 0x5678),
    ]);
}

#[test]
fn usb_cards_leaves_out_a_card_without_usbid_and_ignores_unreadable_lines() {
    assert_eq!(usb_cards(CARDS, &[]), vec![], "no usbid file: not USB");
    let only_pad = usb_cards(CARDS, &[(3, "1234:5678")]);
    assert_eq!(only_pad, vec![("Pad".to_string(), 0x1234, 0x5678)]);
    assert_eq!(usb_cards("garbage\n\n [ : \n", &[(0, "zz")]), vec![]);
    assert_eq!(usb_cards(CARDS, &[(1, "not an id"), (3, "1234:5678")]), only_pad, "an unreadable usbid is skipped");
    assert_eq!(usb_cards("", &[]), vec![]);
}

#[test]
fn usb_cards_name_a_card_by_its_name_not_its_id() {
    let listing = " 2 [BCR32          ]: USB-Audio - Behringer BCR-32\n                      Behringer BCR-32 at usb-0000:00:14.0-1\n 5 [Foo            ]: USB-Audio - Foo - Bar - Baz\n 7 [Odd            ]: USB-Audio\n";
    let got = usb_cards(listing, &[(2, "1397:12cf"), (5, "0001:0002"), (7, "0003:0004")]);
    assert_eq!(got, vec![
        ("Behringer BCR-32".to_string(), 0x1397, 0x12cf),
        ("Foo - Bar - Baz".to_string(), 0x0001, 0x0002),
    ], "name = text after the first ' - '; a line without it is left out");
}

const BENCH: &str = " 2 [BCR32          ]: USB-Audio - Behringer BCR-32
                      Behringer Behringer BCR-32 at usb-0000:00:14.0-1, full speed
 3 [Controller     ]: USB-Audio - Electra Controller
                      Electra Electra Controller at usb-0000:00:14.0-2, full speed
 6 [Minilab3       ]: USB-Audio - Minilab3
                      Arturia Minilab3 at usb-0000:00:14.0-3, full speed
";

#[test]
fn every_usb_device_of_the_bench_listing_gets_its_ids_whatever_its_card_id() {
    let c = usb_cards(BENCH, &[(2, "1397:12cf\n"), (3, "1fc9:82cf\n"), (6, "1c75:020b\n")]);
    assert_eq!(usb_of_port("Behringer BCR-32:Behringer BCR-32 BCR-32 CTRL 24:0", &c), Some((0x1397, 0x12cf)));
    assert_eq!(usb_of_port("Electra Controller:Electra Controller Electra Port 28:0", &c), Some((0x1fc9, 0x82cf)));
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 40:0", &c), Some((0x1c75, 0x020b)));
    assert_eq!(usb_of_port("BCR32:BCR32 MIDI 1 24:0", &c), None, "the card id names nothing");
    assert_eq!(usb_of_port("Controller:x 1:0", &c), None, "the card id names nothing");
}

#[test]
fn cards_whose_names_squash_alike_share_one_name() {
    let listing = " 0 [A ]: USB-Audio - Mini Lab 3\n 1 [B ]: USB-Audio - MiniLab-3\n";
    let differ = usb_cards(listing, &[(0, "1:2"), (1, "3:4")]);
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 40:0", &differ), None);
    let same = usb_cards(listing, &[(0, "1:2"), (1, "1:2")]);
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 40:0", &same), Some((1, 2)));
}

#[test]
fn a_port_takes_the_ids_of_the_card_its_alsa_client_names() {
    let c = cards();
    assert_eq!(usb_of_port("Pad:Pad MIDI 1 28:0", &c), Some((0x1234, 0x5678)));
    assert_eq!(usb_of_port("pad:Pad MIDI 1 28:0", &c), Some((0x1234, 0x5678)), "squashed: case");
    assert_eq!(usb_of_port("P-a d:Pad MIDI 1 28:0", &c), Some((0x1234, 0x5678)), "squashed: punctuation");
}

#[test]
fn two_identical_devices_give_their_ports_the_shared_ids() {
    let c = cards();
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 1 24:0", &c), Some((0x1c75, 0x0288)));
    assert_eq!(usb_of_port("Minilab3:Minilab3 MIDI 1 32:0", &c), Some((0x1c75, 0x0288)));
}

#[test]
fn cards_of_one_name_with_different_ids_give_no_ids() {
    let c = vec![("Box".to_string(), 1, 2), ("Box".to_string(), 3, 4)];
    assert_eq!(usb_of_port("Box:Box MIDI 1 20:0", &c), None);
}

#[test]
fn a_port_that_matches_no_card_has_no_usb_ids() {
    let c = cards();
    assert_eq!(usb_of_port("Midi Through:Midi Through Port-0 14:0", &c), None);
    assert_eq!(usb_of_port("Minilab:Minilab MIDI 1 24:0", &c), None, "never guessed by likeness");
    assert_eq!(usb_of_port("Pad Pro:Pad Pro MIDI 1 24:0", &c), None);
    assert_eq!(usb_of_port("Pad", &[]), None);
    assert_eq!(usb_of_port("PCH:x 1:0", &c), None, "a card that is not USB");
}

#[test]
fn a_port_name_without_client_separator_is_read_whole() {
    assert_eq!(usb_of_port("Pad", &cards()), Some((0x1234, 0x5678)));
}

// ---- the kind of the client of a port, read from the list of clients ----

const FLUID_PORT: &str = "FLUID Synth (2267):Synth input port (2267:0) 128:0";

fn clients_with(entry_name: &str, tail: &str) -> String {
    format!("Client info\n  cur  clients : 2\n\nClient   0 : \"System\" [Kernel]\n  Port   0 : \"Timer\" (Rwe-)\n\
             Client 128 : \"{entry_name}\" {tail}\n  Port   0 : \"Synth input port (2267:0)\" (RWe-)\n")
}

#[test]
fn a_client_whose_first_word_in_brackets_is_user_is_a_program_whatever_follows_or_the_case() {
    for tail in ["[User Legacy]", "[User]", "[user]", "[USER Legacy]", "[User  Legacy extra]"] {
        assert_eq!(client_kind(&clients_with("FLUID Synth (2267)", tail), FLUID_PORT), ClientKind::User, "{tail}");
    }
}

#[test]
fn a_client_whose_first_word_in_brackets_is_kernel_is_a_device_of_the_system() {
    for tail in ["[Kernel]", "[Kernel Legacy]", "[kernel]", "[KERNEL Legacy]"] {
        assert_eq!(client_kind(&clients_with("FLUID Synth (2267)", tail), FLUID_PORT), ClientKind::Kernel, "{tail}");
    }
}

#[test]
fn a_client_whose_first_word_in_brackets_is_another_word_has_the_kind_unknown() {
    for tail in ["[Other Legacy]", "[Usermode]", "[Kernelish]", "[]", "[ ]", "no brackets"] {
        assert_eq!(client_kind(&clients_with("FLUID Synth (2267)", tail), FLUID_PORT), ClientKind::Unknown, "{tail}");
    }
}

#[test]
fn the_port_name_loses_one_final_number_in_parentheses_to_meet_the_name_of_its_client() {
    let list = clients_with("FLUID Synth", "[User Legacy]");
    assert_eq!(client_kind(&list, FLUID_PORT), ClientKind::User);
    assert_eq!(client_kind(&list, "FLUID Synth:Synth input port 128:0"), ClientKind::User, "an exact name meets as before");
    let kernel = clients_with("Minilab3", "[Kernel]");
    assert_eq!(client_kind(&kernel, "Minilab3 (7):Minilab3 MIDI 128:0"), ClientKind::Kernel);
}

#[test]
fn any_other_gap_between_the_port_name_and_the_name_of_the_client_has_the_kind_unknown() {
    let list = clients_with("FLUID Synth", "[User Legacy]");
    for port in ["FLUID Synth (2267) (1):Synth 128:0", "FLUID Synth (abc):Synth 128:0", "FLUID Synth ():Synth 128:0",
                 "FLUID Synth (22a7):Synth 128:0", "FLUID Synth 2267:Synth 128:0", "FLUID Other (2267):Synth 128:0"] {
        assert_eq!(client_kind(&list, port), ClientKind::Unknown, "{port}");
    }
    // The name of the entry is never stripped: only the port's.
    assert_eq!(client_kind(&clients_with("FLUID Synth (2267)", "[User Legacy]"), "FLUID Synth:Synth 128:0"), ClientKind::Unknown);
}

#[test]
fn a_port_with_no_entry_for_its_client_number_or_two_entries_has_the_kind_unknown() {
    let list = clients_with("FLUID Synth", "[User Legacy]");
    assert_eq!(client_kind(&list, "FLUID Synth (2267):Synth input port 129:0"), ClientKind::Unknown);
    assert_eq!(client_kind("", FLUID_PORT), ClientKind::Unknown);
    let twice = format!("{list}Client 128 : \"FLUID Synth (2267)\" [Kernel]\n");
    assert_eq!(client_kind(&twice, FLUID_PORT), ClientKind::Unknown);
}
