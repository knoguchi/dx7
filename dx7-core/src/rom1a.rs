//! DX7 ROM1A factory preset data -- the first 32 voices shipped with every DX7.
//!
//! This module provides the ROM1A factory presets in the standard DX7 packed
//! format (128 bytes per voice, 32 voices = 4096 bytes total).
//!
//! Packed format per voice (128 bytes):
//!   Bytes   0-16:  OP6 (17 bytes per operator, packed)
//!   Bytes  17-33:  OP5
//!   Bytes  34-50:  OP4
//!   Bytes  51-67:  OP3
//!   Bytes  68-84:  OP2
//!   Bytes  85-101: OP1
//!   Byte  102-105: Pitch EG Rates (R1-R4)
//!   Byte  106-109: Pitch EG Levels (L1-L4)
//!   Byte  110:     Algorithm (0-31)
//!   Byte  111:     OscKeySync[3] | Feedback[2:0]
//!   Byte  112:     LFO Speed (0-99)
//!   Byte  113:     LFO Delay (0-99)
//!   Byte  114:     LFO Pitch Mod Depth (0-99)
//!   Byte  115:     LFO Amp Mod Depth (0-99)
//!   Byte  116:     PitchModSens[6:4] | LFO Wave[3:1] | LFO Sync[0]
//!   Byte  117:     Transpose (0-48, 24=C3)
//!   Bytes 118-127: Voice Name (10 ASCII characters)
//!
//! Per-operator packed format (17 bytes):
//!   Byte  0-3:   EG Rates R1-R4 (0-99)
//!   Byte  4-7:   EG Levels L1-L4 (0-99)
//!   Byte  8:     Kbd Level Scaling Break Point (0-99)
//!   Byte  9:     Kbd Level Scaling Left Depth (0-99)
//!   Byte  10:    Kbd Level Scaling Right Depth (0-99)
//!   Byte  11:    Right Curve[3:2] | Left Curve[1:0]
//!   Byte  12:    Detune[6:3] | Rate Scaling[2:0]
//!   Byte  13:    Key Vel Sens[4:2] | Amp Mod Sens[1:0]
//!   Byte  14:    Output Level (0-99)
//!   Byte  15:    Freq Coarse[5:1] | Osc Mode[0]
//!   Byte  16:    Freq Fine (0-99)
//!
//! Voice list:
//!   1.  BRASS   1       2.  BRASS   2       3.  BRASS   3
//!   4.  STRINGS 1       5.  STRINGS 2       6.  STRINGS 3
//!   7.  ORCHESTRA       8.  PIANO   1       9.  PIANO   2
//!  10.  PIANO   3      11.  E.PIANO 1      12.  E.PIANO 2
//!  13.  E.PIANO 3      14.  E.PIANO 4      15.  CLAV   1
//!  16.  HARPSICH       17.  VIBES  1       18.  MARIMBA
//!  19.  KOTO           20.  FLUTE  1       21.  ORCH CHIME
//!  22.  TUB BELLS      23.  STEEL DRM      24.  TIMPANI
//!  25.  SYN-LEAD 1     26.  GUITAR  1      27.  GUITAR  2
//!  28.  ELEC GTR       29.  FUNKY  BS      30.  BASS   1
//!  31.  BASS   2       32.  BASS   3

use crate::patch::DxVoice;

/// Encode one operator in packed format (17 bytes).
#[allow(clippy::too_many_arguments)]
const fn pack_op(
    r1: u8, r2: u8, r3: u8, r4: u8, // EG rates
    l1: u8, l2: u8, l3: u8, l4: u8, // EG levels
    bp: u8, ld: u8, rd: u8,          // breakpoint, left/right depth
    lc: u8, rc: u8,                  // left/right curve (0-3)
    rs: u8, det: u8,                 // rate scaling (0-7), detune (0-14, 7=center)
    ams: u8, kvs: u8,               // amp mod sens (0-3), key vel sens (0-7)
    ol: u8,                          // output level (0-99)
    mode: u8, fc: u8, ff: u8,       // osc mode, freq coarse, freq fine
) -> [u8; 17] {
    [
        r1, r2, r3, r4,
        l1, l2, l3, l4,
        bp, ld, rd,
        (rc << 2) | (lc & 0x03),
        (det << 3) | (rs & 0x07),
        (kvs << 2) | (ams & 0x03),
        ol,
        (fc << 1) | (mode & 0x01),
        ff,
    ]
}

/// Encode global parameters (bytes 102-117).
#[allow(clippy::too_many_arguments)]
const fn pack_global(
    pr1: u8, pr2: u8, pr3: u8, pr4: u8, // pitch EG rates
    pl1: u8, pl2: u8, pl3: u8, pl4: u8, // pitch EG levels
    alg: u8,                              // algorithm (0-31)
    fb: u8, oks: u8,                      // feedback (0-7), osc key sync (0-1)
    lspd: u8, ldly: u8,                  // LFO speed, delay
    lpmd: u8, lamd: u8,                  // LFO pitch/amp mod depth
    lsyn: u8, lwav: u8,                  // LFO sync (0-1), waveform (0-5)
    pms: u8,                              // pitch mod sensitivity (0-7)
    trp: u8,                              // transpose (0-48, 24=C3)
) -> [u8; 16] {
    [
        pr1, pr2, pr3, pr4,
        pl1, pl2, pl3, pl4,
        alg & 0x1F,                                                        // byte 110
        ((oks & 0x01) << 3) | (fb & 0x07),                                // byte 111
        lspd, ldly, lpmd, lamd,                                            // bytes 112-115
        ((pms & 0x07) << 4) | ((lwav & 0x07) << 1) | (lsyn & 0x01),     // byte 116
        trp,                                                               // byte 117
    ]
}

/// Build a 128-byte packed voice from operators + global + name.
const fn pack_voice(
    op6: [u8; 17], op5: [u8; 17], op4: [u8; 17],
    op3: [u8; 17], op2: [u8; 17], op1: [u8; 17],
    glob: [u8; 16], name: [u8; 10],
) -> [u8; 128] {
    let mut v = [0u8; 128];
    let mut i = 0;
    while i < 17 { v[i] = op6[i]; i += 1; }
    i = 0; while i < 17 { v[17 + i] = op5[i]; i += 1; }
    i = 0; while i < 17 { v[34 + i] = op4[i]; i += 1; }
    i = 0; while i < 17 { v[51 + i] = op3[i]; i += 1; }
    i = 0; while i < 17 { v[68 + i] = op2[i]; i += 1; }
    i = 0; while i < 17 { v[85 + i] = op1[i]; i += 1; }
    i = 0; while i < 16 { v[102 + i] = glob[i]; i += 1; }
    i = 0; while i < 10 { v[118 + i] = name[i]; i += 1; }
    v
}

/// Flatten 32 voices into a single 4096-byte array.
const fn flatten(voices: [[u8; 128]; 32]) -> [u8; 4096] {
    let mut d = [0u8; 4096];
    let mut v = 0;
    while v < 32 {
        let mut b = 0;
        while b < 128 { d[v * 128 + b] = voices[v][b]; b += 1; }
        v += 1;
    }
    d
}

/// The 32 ROM1A factory voices (4096 bytes total).
pub const ROM1A_VOICE_DATA: [u8; 4096] = flatten(ROM1A_VOICES);

const ROM1A_VOICES: [[u8; 128]; 32] = [
    // ===== Voice 1: BRASS   1 =====
    // Algorithm 22 (idx 21), Feedback 7, OKS off
    // 3 carrier/modulator pairs; modulators at 1:1 ratio with slow attack
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 99, 0, 1, 0), // OP6
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 58, 0, 1, 0), // OP5
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 99, 0, 1, 0), // OP4
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 58, 0, 1, 0), // OP3
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 99, 0, 1, 0), // OP2
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 78, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global(84, 95, 95, 60, 50, 50, 50, 50, 21, 7, 0, 35, 0,  5,  0,  0,  4,  3, 24),
        *b"BRASS   1 ",
    ),

    // ===== Voice 2: BRASS   2 =====
    // Algorithm 22 (idx 21), Feedback 7
    pack_voice(
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 99, 0, 1, 0),
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 72, 0, 1, 0),
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 99, 0, 1, 0),
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 72, 0, 1, 0),
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 99, 0, 1, 0),
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 85, 0, 1, 0),
        pack_global(84, 95, 95, 60, 50, 50, 50, 50, 21, 7, 0, 35, 0,  5,  0,  0,  4,  3, 24),
        *b"BRASS   2 ",
    ),

    // ===== Voice 3: BRASS   3 =====
    // Algorithm 22 (idx 21), Feedback 6
    pack_voice(
        pack_op(  62, 78, 35, 57, 96, 90, 88,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 89, 0, 1, 0),
        pack_op(  62, 78, 35, 57, 96, 90, 88,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 55, 0, 1, 0),
        pack_op(  62, 78, 35, 57, 96, 90, 88,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 92, 0, 1, 0),
        pack_op(  62, 78, 35, 57, 96, 90, 88,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 55, 0, 1, 0),
        pack_op(  62, 78, 35, 57, 96, 90, 88,  0,  0,  0,  0, 0, 0, 0, 7, 0, 2, 95, 0, 1, 0),
        pack_op(  49, 99, 28, 68, 98, 98, 91,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 82, 0, 1, 0),
        pack_global(84, 95, 95, 60, 50, 50, 50, 50, 21, 6, 0, 35, 33, 5,  0,  0,  4,  3, 24),
        *b"BRASS   3 ",
    ),

    // ===== Voice 4: STRINGS 1 =====
    // Algorithm 2 (idx 1), Feedback 2
    pack_voice(
        pack_op(  62, 50, 50, 72, 99, 97, 97,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 87, 0, 1, 0),
        pack_op(  72, 76, 55, 82, 99, 80, 82,  0, 39, 14,  0, 0, 0, 0, 7, 0, 3, 56, 0, 3, 0),
        pack_op(  62, 50, 50, 72, 99, 97, 97,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 87, 0, 1, 0),
        pack_op(  72, 76, 55, 82, 99, 80, 82,  0, 39, 14,  0, 0, 0, 0, 7, 0, 3, 56, 0, 3, 0),
        pack_op(  62, 50, 50, 72, 99, 97, 97,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 90, 0, 1, 0),
        pack_op(  62, 50, 50, 72, 99, 97, 97,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 95, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  1, 2, 0, 38, 0,  7,  0,  0,  4,  3, 24),
        *b"STRINGS 1 ",
    ),

    // ===== Voice 5: STRINGS 2 =====
    // Algorithm 2 (idx 1), Feedback 2, detuned for chorus
    pack_voice(
        pack_op(  62, 50, 50, 72, 99, 97, 97,  0, 39,  0,  0, 0, 0, 0, 5, 0, 1, 87, 0, 1, 0),
        pack_op(  72, 76, 55, 82, 99, 80, 82,  0, 39, 14,  0, 0, 0, 0, 9, 0, 3, 56, 0, 3, 0),
        pack_op(  62, 50, 50, 72, 99, 97, 97,  0, 39,  0,  0, 0, 0, 0, 5, 0, 1, 87, 0, 1, 0),
        pack_op(  72, 76, 55, 82, 99, 80, 82,  0, 39, 14,  0, 0, 0, 0, 9, 0, 3, 56, 0, 3, 0),
        pack_op(  62, 50, 50, 72, 99, 97, 97,  0, 39,  0,  0, 0, 0, 0, 5, 0, 1, 90, 0, 1, 0),
        pack_op(  62, 50, 50, 72, 99, 97, 97,  0, 39,  0,  0, 0, 0, 0, 9, 0, 1, 95, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  1, 2, 0, 38, 0,  7,  0,  0,  4,  3, 24),
        *b"STRINGS 2 ",
    ),

    // ===== Voice 6: STRINGS 3 =====
    // Algorithm 2 (idx 1), Feedback 3
    pack_voice(
        pack_op(  55, 55, 47, 68, 96, 93, 95,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 82, 0, 1, 0),
        pack_op(  65, 70, 50, 75, 96, 80, 88,  0, 39, 14,  0, 0, 0, 0, 7, 0, 3, 50, 0, 3, 0),
        pack_op(  55, 55, 47, 68, 96, 93, 95,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 82, 0, 1, 0),
        pack_op(  65, 70, 50, 75, 96, 80, 88,  0, 39, 14,  0, 0, 0, 0, 7, 0, 3, 50, 0, 3, 0),
        pack_op(  55, 55, 47, 68, 96, 93, 95,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 86, 0, 1, 0),
        pack_op(  52, 55, 47, 68, 96, 93, 95,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  1, 3, 0, 25, 0,  5,  0,  0,  4,  3, 24),
        *b"STRINGS 3 ",
    ),

    // ===== Voice 7: ORCHESTRA =====
    // Algorithm 2 (idx 1), Feedback 3
    pack_voice(
        pack_op(  55, 55, 47, 68, 96, 93, 95,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 80, 0, 1, 0),
        pack_op(  65, 70, 50, 75, 96, 80, 88,  0, 39, 14,  0, 0, 0, 0, 7, 0, 3, 50, 0, 3, 5),
        pack_op(  55, 55, 47, 68, 96, 93, 95,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 80, 0, 1, 0),
        pack_op(  65, 70, 50, 75, 96, 80, 88,  0, 39, 14,  0, 0, 0, 0, 7, 0, 3, 50, 0, 3, 5),
        pack_op(  55, 55, 47, 68, 96, 93, 95,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 84, 0, 1, 0),
        pack_op(  52, 55, 47, 68, 96, 93, 95,  0, 39,  0,  0, 0, 0, 0, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  1, 3, 0, 25, 0,  5,  0,  0,  4,  3, 24),
        *b"ORCHESTRA ",
    ),

    // ===== Voice 8: PIANO   1 =====
    // Algorithm 5 (idx 4), Feedback 5, OKS on
    pack_voice(
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 94, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 92, 99,  0, 39,  0, 14, 0, 0, 3, 7, 0, 6, 80, 0, 7, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 92, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 92, 99,  0, 39,  0, 14, 0, 0, 3, 7, 0, 6, 80, 0, 7, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 85, 0, 1, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 94, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 5, 1, 35, 0,  0,  0,  0,  0,  3, 24),
        *b"PIANO   1 ",
    ),

    // ===== Voice 9: PIANO   2 =====
    // Algorithm 5 (idx 4), Feedback 5, OKS on
    pack_voice(
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 91, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 85, 99,  0, 39,  0, 18, 0, 0, 3, 7, 0, 6, 75, 0, 9, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 90, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 85, 99,  0, 39,  0, 18, 0, 0, 3, 7, 0, 6, 75, 0, 9, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 82, 0, 1, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 5, 1, 35, 0,  0,  0,  0,  0,  3, 24),
        *b"PIANO   2 ",
    ),

    // ===== Voice 10: PIANO   3 =====
    // Algorithm 5 (idx 4), Feedback 4, OKS on
    pack_voice(
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 88, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 80, 99,  0, 39,  0, 22, 0, 0, 3, 7, 0, 6, 72, 0,10, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 86, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 80, 99,  0, 39,  0, 22, 0, 0, 3, 7, 0, 6, 72, 0,10, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 80, 0, 1, 0),
        pack_op(  96, 99, 28, 68, 99, 98, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 90, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 4, 1, 35, 0,  0,  0,  0,  0,  3, 24),
        *b"PIANO   3 ",
    ),

    // ===== Voice 11: E.PIANO 1 =====
    // Algorithm 5 (idx 4), Feedback 6, OKS on
    // THE iconic DX7 sound. 3 carrier/modulator pairs.
    // OP5 (modulator in pair 3) has freq coarse=14 -- the key to the bell-like attack.
    pack_voice(
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 98, 0, 1, 0), // OP6 carrier
        pack_op(  95, 50, 35, 78, 99, 75,  0,  0, 39,  0, 36, 0, 0, 3, 7, 0, 5, 60, 0,14, 0), // OP5 mod (FC=14!)
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 94, 0, 1, 0), // OP4 carrier
        pack_op(  95, 50, 35, 78, 99, 75,  0,  0, 39,  0, 36, 0, 0, 3, 7, 0, 5, 60, 0, 1, 0), // OP3 modulator
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 86, 0, 1, 0), // OP2 carrier
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 98, 0, 1, 0), // OP1 carrier
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 6, 1, 37, 0,  0,  0,  1,  0,  3, 24),
        *b"E.PIANO 1 ",
    ),

    // ===== Voice 12: E.PIANO 2 =====
    // Algorithm 5 (idx 4), Feedback 6, OKS on
    pack_voice(
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 95, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 72,  0,  0, 39,  0, 32, 0, 0, 3, 7, 0, 5, 58, 0,14, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 92, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 72,  0,  0, 39,  0, 32, 0, 0, 3, 7, 0, 5, 58, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 84, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 96, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 6, 1, 37, 0,  0,  0,  1,  0,  3, 24),
        *b"E.PIANO 2 ",
    ),

    // ===== Voice 13: E.PIANO 3 =====
    pack_voice(
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 92, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 68,  0,  0, 39,  0, 28, 0, 0, 3, 7, 0, 5, 56, 0,14, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 90, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 68,  0,  0, 39,  0, 28, 0, 0, 3, 7, 0, 5, 56, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 82, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 94, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 6, 1, 37, 0,  0,  0,  1,  0,  3, 24),
        *b"E.PIANO 3 ",
    ),

    // ===== Voice 14: E.PIANO 4 =====
    pack_voice(
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 89, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 65,  0,  0, 39,  0, 24, 0, 0, 3, 7, 0, 5, 54, 0,14, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 88, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 65,  0,  0, 39,  0, 24, 0, 0, 3, 7, 0, 5, 54, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 80, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 75,  0,  0, 39,  0,  0, 0, 0, 3, 7, 0, 2, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 5, 1, 37, 0,  0,  0,  1,  0,  3, 24),
        *b"E.PIANO 4 ",
    ),

    // ===== Voice 15: CLAV   1 =====
    // Algorithm 5 (idx 4), Feedback 6, OKS on
    pack_voice(
        pack_op(  99, 42, 28, 72, 99, 87, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 92, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 74, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 6, 78, 0, 3, 0),
        pack_op(  99, 42, 28, 72, 99, 87, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 92, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 74, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 6, 78, 0, 3, 0),
        pack_op(  99, 42, 28, 72, 99, 99, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 86, 0, 1, 0),
        pack_op(  99, 42, 28, 72, 99, 99, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 94, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 6, 1, 35, 0,  0,  0,  0,  0,  3, 24),
        *b"CLAV   1  ",
    ),

    // ===== Voice 16: HARPSICH =====
    // Algorithm 5 (idx 4), Feedback 6, OKS on
    pack_voice(
        pack_op(  99, 42, 28, 72, 99, 80, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 88, 0, 3, 0),
        pack_op(  99, 50, 28, 72, 99, 70, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 6, 74, 0, 4, 0),
        pack_op(  99, 42, 28, 72, 99, 80, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 88, 0, 3, 0),
        pack_op(  99, 50, 28, 72, 99, 70, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 6, 74, 0, 4, 0),
        pack_op(  99, 42, 28, 72, 99, 92, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 86, 0, 1, 0),
        pack_op(  99, 42, 28, 72, 99, 94, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 94, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 6, 1, 35, 0,  0,  0,  0,  0,  3, 24),
        *b"HARPSICH  ",
    ),

    // ===== Voice 17: VIBES  1 =====
    // Algorithm 5 (idx 4), Feedback 4, OKS on
    pack_voice(
        pack_op(  96, 40, 28, 68, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 87, 0, 1, 0),
        pack_op(  96, 50, 28, 72, 99, 72, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 3, 60, 0, 4, 0),
        pack_op(  96, 40, 28, 68, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 87, 0, 1, 0),
        pack_op(  96, 50, 28, 72, 99, 72, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 3, 60, 0, 4, 0),
        pack_op(  96, 40, 28, 68, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 80, 0, 1, 0),
        pack_op(  96, 40, 28, 68, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 4, 1, 35, 0,  0,  6,  0,  0,  0, 24),
        *b"VIBES  1  ",
    ),

    // ===== Voice 18: MARIMBA =====
    // Algorithm 5 (idx 4), Feedback 3, OKS on
    pack_voice(
        pack_op(  99, 35, 28, 62, 99, 80, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 86, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 48, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 3, 56, 0, 4, 0),
        pack_op(  99, 35, 28, 62, 99, 80, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 86, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 48, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 3, 56, 0, 4, 0),
        pack_op(  99, 35, 28, 62, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 78, 0, 1, 0),
        pack_op(  99, 35, 28, 62, 99, 88, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 3, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"MARIMBA   ",
    ),

    // ===== Voice 19: KOTO =====
    // Algorithm 5 (idx 4), Feedback 5, OKS on
    pack_voice(
        pack_op(  99, 38, 28, 65, 99, 64, 99,  0, 39,  0,  0, 0, 0, 2, 7, 0, 3, 88, 0, 2, 0),
        pack_op(  99, 50, 28, 72, 99, 48, 99,  0, 39,  0,  0, 0, 0, 2, 7, 0, 5, 70, 0, 5, 0),
        pack_op(  99, 38, 28, 65, 99, 64, 99,  0, 39,  0,  0, 0, 0, 2, 7, 0, 3, 88, 0, 2, 0),
        pack_op(  99, 50, 28, 72, 99, 48, 99,  0, 39,  0,  0, 0, 0, 2, 7, 0, 5, 70, 0, 5, 0),
        pack_op(  99, 38, 28, 65, 99, 88, 99,  0, 39,  0,  0, 0, 0, 2, 7, 0, 3, 82, 0, 1, 0),
        pack_op(  99, 38, 28, 65, 99, 92, 99,  0, 39,  0,  0, 0, 0, 2, 7, 0, 3, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 5, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"KOTO      ",
    ),

    // ===== Voice 20: FLUTE  1 =====
    // Algorithm 1 (idx 0), Feedback 6, OKS on
    pack_voice(
        pack_op(  72, 50, 50, 72, 99, 99, 99,  0, 39,  0,  0, 0, 0, 0, 7, 0, 0,  0, 0, 1, 0),
        pack_op(  72, 50, 50, 72, 99, 99, 99,  0, 39,  0,  0, 0, 0, 0, 7, 0, 0,  0, 0, 1, 0),
        pack_op(  72, 50, 50, 72, 99, 99, 99,  0, 39,  0,  0, 0, 0, 0, 7, 0, 0,  0, 0, 1, 0),
        pack_op(  72, 50, 50, 72, 99, 99, 99,  0, 39,  0,  0, 0, 0, 0, 7, 0, 0,  0, 0, 1, 0),
        pack_op(  62, 50, 50, 72, 99, 99, 99,  0, 39,  0,  0, 0, 0, 0, 7, 0, 0, 56, 0, 1, 0),
        pack_op(  76, 50, 50, 72, 99, 99, 99,  0, 39,  0,  0, 0, 0, 0, 7, 0, 0, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  0, 6, 1, 35, 0,  5,  0,  0,  4,  3, 24),
        *b"FLUTE  1  ",
    ),

    // ===== Voice 21: ORCH CHIME =====
    // Algorithm 5 (idx 4), Feedback 4, OKS on
    pack_voice(
        pack_op(  99, 35, 28, 55, 99, 50, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 86, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 30, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 56, 0, 4, 0),
        pack_op(  99, 35, 28, 55, 99, 64, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 86, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 40, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 56, 0, 4, 0),
        pack_op(  99, 35, 28, 55, 99, 80, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 78, 0, 1, 0),
        pack_op(  99, 35, 28, 55, 99, 85, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 4, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"ORCH CHIME",
    ),

    // ===== Voice 22: TUB BELLS =====
    // Algorithm 5 (idx 4), Feedback 4, OKS on
    pack_voice(
        pack_op(  99, 35, 28, 55, 99, 60, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 88, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 40, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 60, 0, 3,50),
        pack_op(  99, 35, 28, 55, 99, 60, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 88, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 40, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 60, 0, 3,50),
        pack_op(  99, 35, 28, 55, 99, 80, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 80, 0, 1, 0),
        pack_op(  99, 35, 28, 55, 99, 88, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 4, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"TUB BELLS ",
    ),

    // ===== Voice 23: STEEL DRM =====
    // Algorithm 5 (idx 4), Feedback 4, OKS on -- inharmonic freq fine values
    pack_voice(
        pack_op(  99, 35, 28, 55, 99, 48, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 84, 0, 1,60),
        pack_op(  99, 50, 28, 72, 99, 32, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 58, 0, 3,70),
        pack_op(  99, 35, 28, 55, 99, 56, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 84, 0, 1,30),
        pack_op(  99, 50, 28, 72, 99, 36, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 58, 0, 3,40),
        pack_op(  99, 35, 28, 55, 99, 76, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 80, 0, 1, 0),
        pack_op(  99, 35, 28, 55, 99, 88, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 4, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"STEEL DRM ",
    ),

    // ===== Voice 24: TIMPANI =====
    // Algorithm 5 (idx 4), Feedback 3, OKS on
    pack_voice(
        pack_op(  99, 28, 28, 48, 99, 40, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 84, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 24, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 56, 0, 1, 0),
        pack_op(  99, 28, 28, 48, 99, 40, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 84, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 24, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 56, 0, 1, 0),
        pack_op(  99, 28, 28, 48, 99, 72, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 78, 0, 1, 0),
        pack_op(  99, 28, 28, 48, 99, 88, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 3, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"TIMPANI   ",
    ),

    // ===== Voice 25: SYN-LEAD 1 =====
    // Algorithm 10 (idx 9), Feedback 7
    pack_voice(
        pack_op(  99, 99, 99, 75, 99, 99, 99,  0,  0,  0,  0, 0, 0, 0, 7, 0, 1, 87, 0, 1, 0),
        pack_op(  99, 99, 99, 75, 99, 99, 99,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 75, 0, 3, 0),
        pack_op(  99, 99, 99, 75, 99, 99, 99,  0,  0,  0,  0, 0, 0, 0, 7, 0, 1, 87, 0, 1, 0),
        pack_op(  99, 99, 99, 75, 99, 99, 99,  0,  0,  0,  0, 0, 0, 0, 7, 0, 3, 75, 0, 3, 0),
        pack_op(  99, 99, 99, 75, 99, 99, 99,  0,  0,  0,  0, 0, 0, 0, 7, 0, 1, 80, 0, 1, 0),
        pack_op(  99, 99, 99, 75, 99, 99, 99,  0,  0,  0,  0, 0, 0, 0, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  9, 7, 0, 35, 0,  0,  0,  0,  0,  3, 24),
        *b"SYN-LEAD 1",
    ),

    // ===== Voice 26: GUITAR  1 =====
    // Algorithm 5 (idx 4), Feedback 5, OKS on
    pack_voice(
        pack_op(  99, 42, 28, 72, 99, 80, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 88, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 56, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 5, 70, 0, 5, 0),
        pack_op(  99, 42, 28, 72, 99, 80, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 88, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 56, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 5, 70, 0, 5, 0),
        pack_op(  99, 42, 28, 72, 99, 90, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 82, 0, 1, 0),
        pack_op(  99, 42, 28, 72, 99, 94, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 5, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"GUITAR  1 ",
    ),

    // ===== Voice 27: GUITAR  2 =====
    // Algorithm 5 (idx 4), Feedback 5, OKS on
    pack_voice(
        pack_op(  99, 42, 28, 72, 99, 76, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 86, 0, 2, 0),
        pack_op(  99, 50, 28, 72, 99, 52, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 5, 68, 0, 6, 0),
        pack_op(  99, 42, 28, 72, 99, 76, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 86, 0, 2, 0),
        pack_op(  99, 50, 28, 72, 99, 52, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 5, 68, 0, 6, 0),
        pack_op(  99, 42, 28, 72, 99, 88, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 80, 0, 1, 0),
        pack_op(  99, 42, 28, 72, 99, 92, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 5, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"GUITAR  2 ",
    ),

    // ===== Voice 28: ELEC GTR =====
    // Algorithm 5 (idx 4), Feedback 6, OKS on
    pack_voice(
        pack_op(  99, 42, 28, 72, 99, 72, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 84, 0, 2, 0),
        pack_op(  99, 50, 28, 72, 99, 48, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 6, 66, 0, 6, 0),
        pack_op(  99, 42, 28, 72, 99, 72, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 84, 0, 2, 0),
        pack_op(  99, 50, 28, 72, 99, 48, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 6, 66, 0, 6, 0),
        pack_op(  99, 42, 28, 72, 99, 86, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 80, 0, 1, 0),
        pack_op(  99, 42, 28, 72, 99, 90, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 6, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"ELEC GTR  ",
    ),

    // ===== Voice 29: FUNKY  BS =====
    // Algorithm 5 (idx 4), Feedback 6, OKS on
    pack_voice(
        pack_op(  99, 42, 28, 72, 99, 74, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 86, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 50, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 6, 68, 0, 3, 0),
        pack_op(  99, 42, 28, 72, 99, 74, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 86, 0, 1, 0),
        pack_op(  99, 50, 28, 72, 99, 50, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 6, 68, 0, 3, 0),
        pack_op(  99, 42, 28, 72, 99, 88, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 80, 0, 1, 0),
        pack_op(  99, 42, 28, 72, 99, 94, 99,  0, 39,  0,  0, 0, 0, 3, 7, 0, 4, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 6, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"FUNKY  BS ",
    ),

    // ===== Voice 30: BASS   1 =====
    // Algorithm 5 (idx 4), Feedback 5, OKS on
    pack_voice(
        pack_op(  96, 25, 25, 67, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 68, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 70, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 68, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 70, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 84, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 94, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 5, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"BASS   1  ",
    ),

    // ===== Voice 31: BASS   2 =====
    // Algorithm 5 (idx 4), Feedback 5, OKS on
    pack_voice(
        pack_op(  96, 25, 25, 67, 99, 90, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 90, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 64, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 68, 0, 2, 0),
        pack_op(  96, 25, 25, 67, 99, 90, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 90, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 64, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 68, 0, 2, 0),
        pack_op(  96, 25, 25, 67, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 82, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 5, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"BASS   2  ",
    ),

    // ===== Voice 32: BASS   3 =====
    // Algorithm 5 (idx 4), Feedback 4, OKS on
    pack_voice(
        pack_op(  96, 25, 25, 67, 99, 86, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 88, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 60, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 66, 0, 3, 0),
        pack_op(  96, 25, 25, 67, 99, 86, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 88, 0, 1, 0),
        pack_op(  95, 50, 35, 78, 99, 60, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 4, 66, 0, 3, 0),
        pack_op(  96, 25, 25, 67, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 80, 0, 1, 0),
        pack_op(  96, 25, 25, 67, 99, 99, 99,  0, 39,  0,  0, 0, 0, 1, 7, 0, 1, 92, 0, 1, 0),
        pack_global(94, 67, 95, 60, 50, 50, 50, 50,  4, 4, 1, 35, 0,  0,  0,  0,  0,  0, 24),
        *b"BASS   3  ",
    ),
];

/// Name list for ROM1A voices.
pub const ROM1A_VOICE_NAMES: [&str; 32] = [
    "BRASS   1", "BRASS   2", "BRASS   3",
    "STRINGS 1", "STRINGS 2", "STRINGS 3",
    "ORCHESTRA",
    "PIANO   1", "PIANO   2", "PIANO   3",
    "E.PIANO 1", "E.PIANO 2", "E.PIANO 3", "E.PIANO 4",
    "CLAV   1",  "HARPSICH",
    "VIBES  1",  "MARIMBA",   "KOTO",       "FLUTE  1",
    "ORCH CHIME", "TUB BELLS", "STEEL DRM", "TIMPANI",
    "SYN-LEAD 1",
    "GUITAR  1", "GUITAR  2", "ELEC GTR",
    "FUNKY  BS",
    "BASS   1",  "BASS   2",  "BASS   3",
];

/// Load all 32 ROM1A factory voices.
#[cfg(feature = "std")]
pub fn load_rom1a() -> Vec<DxVoice> {
    let mut voices = Vec::with_capacity(32);
    for i in 0..32 {
        let start = i * 128;
        let mut voice_data = [0u8; 128];
        voice_data.copy_from_slice(&ROM1A_VOICE_DATA[start..start + 128]);
        voices.push(DxVoice::from_packed(&voice_data));
    }
    voices
}

/// Load a single ROM1A factory voice by index (0-31).
pub fn load_rom1a_voice(index: usize) -> Option<DxVoice> {
    if index >= 32 {
        return None;
    }
    let start = index * 128;
    let mut voice_data = [0u8; 128];
    voice_data.copy_from_slice(&ROM1A_VOICE_DATA[start..start + 128]);
    Some(DxVoice::from_packed(&voice_data))
}

/// Build a complete SysEx bulk dump message (4104 bytes).
/// Format: F0 43 00 09 20 00 <4096 bytes> <checksum> F7
#[cfg(feature = "std")]
pub fn rom1a_sysex_dump() -> Vec<u8> {
    let mut sysex = Vec::with_capacity(4104);
    sysex.push(0xF0);
    sysex.push(0x43);
    sysex.push(0x00);
    sysex.push(0x09);
    sysex.push(0x20);
    sysex.push(0x00);
    sysex.extend_from_slice(&ROM1A_VOICE_DATA);
    let sum: u8 = ROM1A_VOICE_DATA.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    let checksum = (!sum).wrapping_add(1) & 0x7F;
    sysex.push(checksum);
    sysex.push(0xF7);
    sysex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rom1a_data_size() {
        assert_eq!(ROM1A_VOICE_DATA.len(), 4096);
    }

    #[test]
    fn test_load_all_32_voices() {
        let voices = load_rom1a();
        assert_eq!(voices.len(), 32);
    }

    #[test]
    fn test_voice_names() {
        let voices = load_rom1a();
        for (i, voice) in voices.iter().enumerate() {
            let name = voice.name_str().trim();
            let expected = ROM1A_VOICE_NAMES[i].trim();
            assert_eq!(name, expected,
                "Voice {} name mismatch: got '{}', expected '{}'", i + 1, name, expected);
        }
    }

    #[test]
    fn test_brass1() {
        let v = load_rom1a_voice(0).unwrap();
        assert_eq!(v.name_str().trim(), "BRASS   1");
        assert_eq!(v.algorithm, 21, "BRASS 1 alg should be 21 (alg 22)");
        assert_eq!(v.feedback, 7, "BRASS 1 fb should be 7");
        assert!(!v.osc_key_sync, "BRASS 1 OKS should be off");
    }

    #[test]
    fn test_epiano1() {
        let v = load_rom1a_voice(10).unwrap();
        assert_eq!(v.name_str().trim(), "E.PIANO 1");
        assert_eq!(v.algorithm, 4, "E.PIANO 1 alg should be 4 (alg 5)");
        assert_eq!(v.feedback, 6, "E.PIANO 1 fb should be 6");
        assert!(v.osc_key_sync, "E.PIANO 1 OKS should be on");
        assert_eq!(v.transpose, 24, "E.PIANO 1 transpose should be 24 (C3)");
        // OP5 is at index 1 (index 0 = OP6, index 1 = OP5, ..., index 5 = OP1)
        assert_eq!(v.operators[1].osc_freq_coarse, 14,
            "OP5 freq coarse should be 14, got {}", v.operators[1].osc_freq_coarse);
        // OP6 at index 0 has OL=98 and KVS=2
        assert_eq!(v.operators[0].output_level, 98);
        assert_eq!(v.operators[0].key_velocity_sensitivity, 2);
    }

    #[test]
    fn test_strings1() {
        let v = load_rom1a_voice(3).unwrap();
        assert_eq!(v.name_str().trim(), "STRINGS 1");
        assert_eq!(v.algorithm, 1, "STRINGS 1 alg should be 1 (alg 2)");
        assert_eq!(v.feedback, 2);
    }

    #[test]
    fn test_bass1() {
        let v = load_rom1a_voice(29).unwrap();
        assert_eq!(v.name_str().trim(), "BASS   1");
        assert_eq!(v.algorithm, 4, "BASS 1 alg should be 4 (alg 5)");
        assert_eq!(v.feedback, 5);
    }

    #[test]
    fn test_synlead1() {
        let v = load_rom1a_voice(24).unwrap();
        assert_eq!(v.name_str().trim(), "SYN-LEAD 1");
        assert_eq!(v.algorithm, 9, "SYN-LEAD 1 alg should be 9 (alg 10)");
        assert_eq!(v.feedback, 7);
    }

    #[test]
    fn test_all_params_in_range() {
        let voices = load_rom1a();
        for (vi, voice) in voices.iter().enumerate() {
            for (oi, op) in voice.operators.iter().enumerate() {
                assert!(op.output_level <= 99, "V{} OP{} OL={}", vi+1, oi+1, op.output_level);
                assert!(op.osc_freq_coarse <= 31, "V{} OP{} FC={}", vi+1, oi+1, op.osc_freq_coarse);
                assert!(op.osc_freq_fine <= 99, "V{} OP{} FF={}", vi+1, oi+1, op.osc_freq_fine);
                assert!(op.osc_detune <= 14, "V{} OP{} DET={}", vi+1, oi+1, op.osc_detune);
                assert!(op.kbd_rate_scaling <= 7, "V{} OP{} RS={}", vi+1, oi+1, op.kbd_rate_scaling);
                assert!(op.amp_mod_sensitivity <= 3, "V{} OP{} AMS={}", vi+1, oi+1, op.amp_mod_sensitivity);
                assert!(op.key_velocity_sensitivity <= 7, "V{} OP{} KVS={}", vi+1, oi+1, op.key_velocity_sensitivity);
            }
            assert!(voice.algorithm <= 31, "V{} ALG={}", vi+1, voice.algorithm);
            assert!(voice.feedback <= 7, "V{} FB={}", vi+1, voice.feedback);
            assert!(voice.pitch_mod_sensitivity <= 7, "V{} PMS={}", vi+1, voice.pitch_mod_sensitivity);
        }
    }

    #[test]
    fn test_sysex_dump() {
        let sysex = rom1a_sysex_dump();
        assert_eq!(sysex.len(), 4104);
        assert_eq!(sysex[0], 0xF0);
        assert_eq!(sysex[1], 0x43);
        assert_eq!(sysex[3], 0x09);
        assert_eq!(*sysex.last().unwrap(), 0xF7);
    }

    #[test]
    fn test_single_voice_bounds() {
        assert!(load_rom1a_voice(0).is_some());
        assert!(load_rom1a_voice(31).is_some());
        assert!(load_rom1a_voice(32).is_none());
    }
}
