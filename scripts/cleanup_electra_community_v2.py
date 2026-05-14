#!/usr/bin/env python3
"""Second curation pass on `devices/electra-community/` after the private
import dumped 130 more files there. Same shape as v1 (delete vs move)."""
from __future__ import annotations
import json, re, sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
COMMUNITY = REPO_ROOT / "devices" / "electra-community"
DEVICES = REPO_ROOT / "devices"

DELETE = {
    "compressors", "controllers", "equalizers_v02", "config_horizontale",
    "lfo_eg", "remote", "remote_wip", "sp_lua_testing", "woody_sysex_test",
    "stand_alone", "studio", "studio_2024", "studio_control",
    "studio_control_master", "ultimate_studio_control", "repeaters",
    "fst_ctrl", "alec_troniq_live", "ableton_live_etfy",
}

MOVE: dict[str, str] = {
    # 1010music
    "bluebox_mpe_lfo": "1010music", "boutique_bluebox_set": "1010music",
    # Akai (some already moved earlier by rebucket; anything left here)
    # Arturia
    "minifreakv_1_0": "Arturia",
    # Novation Bass Station 2
    "bs2_third_osc": "Novation",
    # Chase Bliss
    "cba_mood_mkii": "Chase Bliss",
    # Oberheim
    "clone_of_matrix_6": "Oberheim", "mark_matrix_6_1000": "Oberheim",
    # Dreadbox
    "clone_of_dreadbox_typhon": "Dreadbox",
    # Yamaha
    "clone_of_yamaha_pss480_jg": "Yamaha",
    "revision_49_of_yamaha_dx_7_ii": "Yamaha",
    "tx_81z_base": "Yamaha",
    "montage_7_part_ask": "Yamaha", "montage_m_controls": "Yamaha",
    # Modal
    "craft_synth_2": "Modal Electronics",
    # Steinberg
    "cubase_12_v2": "Steinberg",
    # Synthstrom
    "delugezoom": "Synthstrom Audible", "doubluge": "Synthstrom Audible",
    # Expert Sleepers
    "disting_nt_standard": "Expert Sleepers",
    # Waldorf
    "doug_kyra_ch_1_2": "Waldorf", "pulse_nig_v1_4": "Waldorf",
    # E-MU
    "emu_e4_editor_v3": "E-MU",
    # Native Instruments
    "fm8": "Native Instruments", "reaktor_mf": "Native Instruments",
    # Imaginando
    "imaginando_frms_v01": "Imaginando",
    # Korg
    "imported_01r_w": "Korg", "nts_3_editor": "Korg",
    "rout_18_korg_nts_3": "Korg",
    "volca": "Korg", "volca_sample_2": "Korg",
    "ios_kg_mono_mc02": "Korg", "ios_kg_mono_mc03": "Korg",
    "ios_kg_mono_mc05": "Korg", "ios_kg_poly_mc06": "Korg",
    # Inphonik
    "inphonik_rym2612_v02_live": "Inphonik",
    # Roland
    "juno60minerva_ab": "Roland", "mc_505_part_a": "Roland",
    # Kawai
    "k4r_v7_p2c11": "Kawai",
    # Kemper
    "kemper": "Kemper",
    # Kinotone
    "kinotone_ribbons": "Kinotone",
    # Lexicon
    "lxp_1_v0_40": "Lexicon",
    # Sonic Potions
    "lxr_02_dave": "Sonic Potions",
    # Alesis
    "micron_beta": "Alesis",
    # Blokas
    "midihub_willie": "Blokas",
    # Modor
    "midisynth_modor_nf_1m": "Modor", "modor_nf_1m": "Modor",
    # Strymon
    "mobius": "Strymon",
    # Elektron
    "octaplacktform": "Elektron", "ot_a4": "Elektron",
    "all_elektrons_tr8": "Elektron",
    "exc_te_snare_drum_x3": "Elektron",
    "torso_oct_rytm_tb": "Elektron",
    # Teenage Engineering
    "opzilla_v01": "Teenage Engineering",
    # Orla
    "orla_dse_12": "Orla",
    # PreenFM
    "preen_fm2_beta": "PreenFM",
    # PWM
    "pwm_mantis_v3": "PWM",
    # Softube
    "softube_heartbeat_v01": "Softube",
    # Source Audio
    "ventris": "Source Audio",
    # Access
    "virus_ti_mod": "Access",
    # Vital
    "vital_v01": "Vital Audio",
    # Behringer
    "x32_fx_ch_9_16_plus": "Behringer",
    # BeepStreet
    "zeeon": "BeepStreet",
    # AIR Music Tech
    "air_drumsynth_500_v01": "AIR Music Tech",
    # Creamware
    "cw_minimax_asb": "Creamware",
    # Generalmusic
    "gem_rpx": "Generalmusic",
    # Dekrispator / Synthesia-ish — skip (unknown)
    # Misc
    "viper": "Lamberhuus",  # Viper synth by Lamberhuus (guess, low-confidence; could skip)
}


def vslug(v: str) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", v.lower().strip()).strip("-")
    return s or "unknown"


def main():
    dry = "--dry-run" in sys.argv
    files = sorted(COMMUNITY.glob("*.json"))
    stems = {p.stem for p in files}
    missing = (DELETE | set(MOVE)) - stems
    if missing:
        print(f"WARN: not in community dir: {sorted(missing)}")
    to_del = [p for p in files if p.stem in DELETE]
    to_mv = [(p, MOVE[p.stem]) for p in files if p.stem in MOVE]
    kept = [p for p in files if p.stem not in DELETE and p.stem not in MOVE]
    print(f"delete: {len(to_del)}   move: {len(to_mv)}   keep: {len(kept)}")
    print("\n--- remaining electra-community ---")
    for p in kept:
        print(f"  {p.stem}")
    if dry:
        return
    for p in to_del:
        p.unlink()
    for src, vendor in to_mv:
        dest_dir = DEVICES / vslug(vendor)
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / src.name
        if dest.exists():
            print(f"SKIP collision: {src.name} → {dest}"); continue
        d = json.loads(src.read_text())
        d.setdefault("device", {})["vendor"] = vendor
        dest.write_text(json.dumps(d, indent=2) + "\n")
        src.unlink()
    print(f"\ndone. remaining: {len(list(COMMUNITY.glob('*.json')))}")


if __name__ == "__main__":
    main()
