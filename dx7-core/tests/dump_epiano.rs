#[test]
fn dump_epiano1_params() {
    dx7_core::tables::init_tables(44100);
    dx7_core::lfo::init_lfo(44100);
    dx7_core::pitchenv::init_pitchenv(44100);
    
    let v = dx7_core::preset::e_piano_1();
    eprintln!("=== E.PIANO 1 ===");
    eprintln!("Algorithm: {} (display: {})", v.algorithm, v.algorithm + 1);
    eprintln!("Feedback: {}", v.feedback);
    eprintln!("Osc Key Sync: {}", v.osc_key_sync);
    eprintln!("Transpose: {}", v.transpose);
    
    for i in 0..6 {
        let op = &v.operators[i];
        let op_num = 6 - i;
        let carrier = dx7_core::voice::is_carrier(v.algorithm as usize, i);
        eprintln!("OP{} [idx={}] {}{}:",
            op_num, i,
            if carrier { "CARRIER " } else { "MOD " },
            if i == 0 { "(fb)" } else { "" }
        );
        eprintln!("  OL={} Coarse={} Fine={} Detune={} Mode={}",
            op.output_level, op.osc_freq_coarse, op.osc_freq_fine,
            op.osc_detune, op.osc_mode);
        eprintln!("  EG R={:?} L={:?}",
            op.eg.rates, op.eg.levels);
        eprintln!("  KVS={} AMS={} KRS={}",
            op.key_velocity_sensitivity, op.amp_mod_sensitivity,
            op.kbd_rate_scaling);
    }
}
