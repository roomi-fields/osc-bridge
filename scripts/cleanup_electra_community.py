#!/usr/bin/env python3
"""Manual curation pass on the leftover `devices/electra-community/`.

Two buckets:
  - DELETE: Electra-platform-only content with no value as an osc-bridge
    driver (Lua examples, CV/MIDI utility presets, platform demos, tests,
    generic FX template presets).
  - MOVE: presets that ARE a real device driver but whose filename the
    regex heuristic missed — explicit vendor assignment.

Run with --dry-run first to sanity-check.
"""
from __future__ import annotations
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
COMMUNITY = REPO_ROOT / "devices" / "electra-community"
DEVICES = REPO_ROOT / "devices"


# === DELETE: pure Electra-platform content, no device behind it ===
DELETE = {
    # Lua scripting examples (Electra preset scripting docs)
    "lua_closures", "lua_compatibility", "lua_control_api", "lua_dynamic_lists",
    "lua_graphics", "lua_groups_api", "lua_handling_simple_cc", "lua_hello_world",
    "lua_hidden_page", "lua_info_text", "lua_inheritance", "lua_message_api",
    "lua_modulation", "lua_overlays", "lua_page_api", "lua_parametermap",
    "lua_persisted_data", "lua_preset_init", "lua_require", "lua_touch_pad",
    "lua_value_api", "lua_value_formatting", "lua_value_ranges", "lua_xy_pad",
    # CV utility presets (Electra CV interface)
    "cvclockin", "cvclockout", "cvenvelopefoll", "cvin", "cvlfo",
    "cvshaper", "cvutility",
    # Generic MIDI effect presets — not device drivers
    "midiarpeggiator", "midicccontrol", "midichord", "midichord_12",
    "midinotelength", "midinotelength_12", "midipitcher", "midipitcher_12",
    "midirandom", "midirandom_12", "midiscale", "midivelocity",
    # Electra platform demos / tests / templates / bug repros
    "32_channels_demo", "all_midi_ccs", "cc_demo", "colours_and_controls",
    "config", "control_setslot_bug", "custom_with_2_parms", "default_preset",
    "demo_preset", "double_tap_template", "e1_fader_forum", "jumpy_parameters",
    "mixed_controls", "multi_demo", "multi_persist_recall", "name_editor",
    "no_midi_callback", "note_graphing_synth", "note_on_pot_touch",
    "poc_smart_controls", "pots_touch_freeze_e1", "pottouchcomboactions",
    "setactivecontrolset", "setslot_not_working", "sysex_example",
    "sysex_problem", "sysex_template_demo", "test_send_value",
    "timer_cc", "toggle_button", "touch_default_cc", "touch_test",
    # Generic MPE / expression / LFO / meta — Electra user tooling
    "mpecontrol", "expressioncont", "bounds", "reverse_bi_polar",
    "random", "random_lfo", "metronomes", "lfo", "cube_lfo",
    "mini_cube_lfo", "send_lfo", "vst_controller", "tone_projects",
    "ms_1_settings",           # Electra MK1 settings panel
    "ldk_live", "lk_ctrl",     # Ableton Live-control templates, not a device
    "outgrowth_auv3",          # generic AUv3 plugin control
    "notesplitter_v4_newignis",# Electra scripting example
}


# === MOVE: explicit vendor assignments the regex pass missed ===
MOVE: dict[str, str] = {
    # Expressive E Osmose
    "dual_lecaine_osmose": "Expressive E",
    "dual_sinebank_osmose": "Expressive E",
    "dual_wavebank_cont": "Expressive E",
    "dual_wavebank_osmose": "Expressive E",
    "osmose_ctrl_10_52_v2": "Expressive E",
    "osmose_macros": "Expressive E",
    # Expert Sleepers Disting EX
    "disting_ex_algos": "Expert Sleepers",
    "disting_ex_mi_algos": "Expert Sleepers",
    # Missed-by-heuristic
    "hydrasynthv1": "ASM",
    "dx7ii_e1_editor": "Yamaha",
    "plg150an": "Yamaha",
    "equator2_basic": "ROLI",
    "micron_v4": "Alesis",
    "lxr_01_drum_synth_bc0_37": "Sonic Potions",
    "pwm_mantis_v4": "PWM",
    "reverb_plate_140": "Arturia",
    "sammichfm": "Midibox",
    "sammichfm_drums": "Midibox",
    "baloran_the_river": "Baloran",
    "beatbot_tt_78": "Beatbot",
    # Eventide H90 algorithm presets
    "100_diatonic_shift": "Eventide",
    "101_layered_shift": "Eventide",
    "102_dual_shift": "Eventide",
    "103_stereo_shift": "Eventide",
    "104_reverse_shift": "Eventide",
    "105_swept_combs": "Eventide",
    "106_swept_reverb": "Eventide",
    "107_reverb_factory": "Eventide",
    "108_ultra_tap_wip": "Eventide",
    "109_long_digiplex": "Eventide",
    "110_dual_digiplex": "Eventide",
    "111_patch_factory": "Eventide",
    "112_stutter_wip": "Eventide",
    "114_dense_room": "Eventide",
    "115_vocoder": "Eventide",
    "116_multi_shift": "Eventide",
    "117_band_delays": "Eventide",
    # Ableton Live built-in devices
    "autofilter": "Ableton", "autofilter2_borked": "Ableton",
    "autopan": "Ableton", "autopan2": "Ableton", "autoshift": "Ableton",
    "beatrepeat": "Ableton", "channeleq": "Ableton", "chorus": "Ableton",
    "chorus2": "Ableton", "compressor2": "Ableton", "delay": "Ableton",
    "delay_v2": "Ableton", "delay_v3": "Ableton", "drift": "Ableton",
    "drumcell": "Ableton", "electro_2": "Ableton", "erosion": "Ableton",
    "filter_factory": "Ableton", "filterbank_2": "Ableton",
    "filtereq3": "Ableton", "gate": "Ableton", "graindelay": "Ableton",
    "limiter": "Ableton", "looper_v2": "Ableton", "noisy_2": "Ableton",
    "originalsimpler": "Ableton", "phasernew": "Ableton",
    "saturator": "Ableton", "saturator_12_1": "Ableton",
    "stereogain": "Ableton", "tremotron": "Ableton", "vocalfreak": "Ableton",
}


def vslug(v: str) -> str:
    import re
    s = v.lower().strip()
    s = re.sub(r"[^a-z0-9]+", "-", s).strip("-")
    return s or "unknown"


def main():
    dry = "--dry-run" in sys.argv
    files = sorted(p for p in COMMUNITY.glob("*.json"))
    stems = {p.stem for p in files}
    # Sanity-check: referenced files exist
    missing = (DELETE | set(MOVE.keys())) - stems
    if missing:
        print(f"WARN: referenced files not in community dir: {sorted(missing)[:10]}")
    to_delete = [p for p in files if p.stem in DELETE]
    to_move = [(p, MOVE[p.stem]) for p in files if p.stem in MOVE]
    kept = [p for p in files if p.stem not in DELETE and p.stem not in MOVE]
    print(f"delete: {len(to_delete)}   move: {len(to_move)}   keep: {len(kept)}")
    print("\n--- KEEP (remaining in electra-community) ---")
    for p in kept:
        print(f"  {p.stem}")
    if dry:
        return
    for p in to_delete:
        p.unlink()
    for src, vendor in to_move:
        dest_dir = DEVICES / vslug(vendor)
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / src.name
        if dest.exists():
            print(f"SKIP collision: {src.name} → {dest}")
            continue
        d = json.loads(src.read_text())
        d.setdefault("device", {})["vendor"] = vendor
        dest.write_text(json.dumps(d, indent=2) + "\n")
        src.unlink()
    print(f"\ndone. remaining: {len(list(COMMUNITY.glob('*.json')))}")


if __name__ == "__main__":
    main()
