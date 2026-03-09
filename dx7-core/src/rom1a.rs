//! DX7 ROM1A factory preset data -- the first 32 voices shipped with every DX7.
//!
//! This module provides the ROM1A factory presets in the standard DX7 packed
//! format (128 bytes per voice, 32 voices = 4096 bytes total).
//!
//! Generated from sysex/factory/rom1a.syx
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
//!   1.  BRASS   1     //!   2.  BRASS   2     //!   3.  BRASS   3
//!   4.  STRINGS 1     //!   5.  STRINGS 2     //!   6.  STRINGS 3
//!   7.  ORCHESTRA     //!   8.  PIANO   1     //!   9.  PIANO   2
//!  10.  PIANO   3     //!  11.  E.PIANO 1     //!  12.  GUITAR  1
//!  13.  GUITAR  2     //!  14.  SYN-LEAD 1    //!  15.  BASS    1
//!  16.  BASS    2     //!  17.  E.ORGAN 1     //!  18.  PIPES   1
//!  19.  HARPSICH 1    //!  20.  CLAV    1     //!  21.  VIBE    1
//!  22.  MARIMBA       //!  23.  KOTO          //!  24.  FLUTE   1
//!  25.  ORCH-CHIME    //!  26.  TUB BELLS     //!  27.  STEEL DRUM
//!  28.  TIMPANI       //!  29.  REFS WHISL    //!  30.  VOICE   1
//!  31.  TRAIN         //!  32.  TAKE OFF

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
    // Algorithm 22 (idx 21), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 49, 99, 28, 68, 98, 98, 91,  0, 39, 54, 50, 1,1, 4, 7, 0,2, 82, 0, 1, 0), // OP6
        pack_op( 77, 36, 41, 71, 99, 98, 98,  0, 39,  0,  0, 3,3, 0, 8, 0,2, 98, 0, 1, 0), // OP5
        pack_op( 77, 36, 41, 71, 99, 98, 98,  0, 39,  0,  0, 3,3, 0, 7, 0,2, 99, 0, 1, 0), // OP4
        pack_op( 77, 76, 82, 71, 99, 98, 98,  0, 39,  0,  0, 3,3, 0, 5, 0,2, 99, 0, 1, 0), // OP3
        pack_op( 62, 51, 29, 71, 82, 95, 96,  0, 27,  0,  7, 3,1, 0,14, 0,0, 86, 0, 0, 0), // OP2
        pack_op( 72, 76, 99, 71, 99, 88, 96,  0, 39,  0, 14, 3,3, 0,14, 0,0, 98, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 84, 95, 95, 60, 50, 50, 50, 50, 21, 7, 1, 37,  0,  5,  0,  0,  4,  3, 24),
        *b"BRASS   1 ",
    ),

    // ===== Voice 2: BRASS   2 =====
    // Algorithm 22 (idx 21), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 39, 32, 71, 99, 98, 88,  0, 51,  0,  0, 3,3, 0, 7, 0,0, 80, 0, 0, 0), // OP6
        pack_op( 99, 39, 32, 71, 99, 98, 81,  0, 39,  0,  0, 3,3, 0, 8, 0,0, 99, 0, 0, 0), // OP5
        pack_op( 99, 39, 32, 71, 99, 98, 81,  0, 39,  0,  0, 3,3, 0, 5, 0,0, 99, 0, 0, 0), // OP4
        pack_op( 99, 39, 32, 71, 99, 98, 81,  0, 39,  0,  0, 3,3, 0, 4, 0,0, 99, 0, 0, 0), // OP3
        pack_op( 99, 39, 32, 71, 99, 98, 80,  0, 51,  0, 38, 3,3, 0,14, 0,0, 84, 0, 0, 0), // OP2
        pack_op( 99, 39, 32, 71, 99, 98, 80,  0, 51,  0, 38, 3,3, 0,14, 0,0, 99, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 84, 95, 95, 60, 50, 50, 50, 50, 21, 7, 1, 37,  0,  0,  0,  0,  4,  3, 24),
        *b"BRASS   2 ",
    ),

    // ===== Voice 3: BRASS   3 =====
    // Algorithm 18 (idx 17), Feedback 6, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 77, 56, 20, 70, 99,  0,  0,  0,  0,  0,  0, 0,0, 7, 7, 0,0, 79, 0, 7,21), // OP6
        pack_op( 48, 55, 22, 50, 98, 61, 62,  0,  0,  0,  0, 0,0, 0, 6, 0,0, 70, 0, 3, 6), // OP5
        pack_op( 66, 92, 22, 50, 53, 61, 62,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 79, 0, 1, 0), // OP4
        pack_op( 46, 35, 22, 50, 99, 86, 86,  0,  0,  0,  0, 0,0, 1, 7, 0,1, 77, 0, 1, 0), // OP3
        pack_op( 37, 34, 15, 70, 85,  0,  0,  0,  0,  0,  0, 0,0, 2, 7, 0,1, 70, 0, 1, 0), // OP2
        pack_op( 55, 24, 19, 55, 99, 86, 86,  0,  0,  0,  0, 0,0, 2, 7, 0,2, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50, 17, 6, 1, 35,  0,  5,  0,  0,  0,  3, 12),
        *b"BRASS   3 ",
    ),

    // ===== Voice 4: STRINGS 1 =====
    // Algorithm 2 (idx 1), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 53, 19, 20, 54, 99, 92, 86,  0,  0,  0,  0, 0,0, 2, 7, 0,2, 53, 0,14, 0), // OP6
        pack_op( 53, 19, 20, 54, 86, 92, 86,  0,  0,  0,  0, 0,0, 2, 7, 0,2, 84, 0, 3, 0), // OP5
        pack_op( 96, 19, 20, 54, 99, 92, 86,  0,  0,  0,  0, 0,0, 2, 7, 0,2, 77, 0, 1, 0), // OP4
        pack_op( 44, 45, 20, 54, 99, 85, 82,  0, 56,  0, 97, 0,0, 0, 7, 0,7, 86, 0, 1, 0), // OP3
        pack_op( 75, 71, 17, 49, 82, 92, 62,  0, 54,  0,  0, 0,0, 1, 7, 0,0, 83, 0, 1, 0), // OP2
        pack_op( 45, 24, 20, 41, 99, 85, 70,  0,  0,  0,  0, 0,0, 2, 7, 0,3, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50,  1, 7, 1, 30,  0,  8,  0,  0,  0,  2, 24),
        *b"STRINGS 1 ",
    ),

    // ===== Voice 5: STRINGS 2 =====
    // Algorithm 2 (idx 1), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 72, 76, 10, 32, 99, 92,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 70, 0, 8, 0), // OP6
        pack_op( 76, 73, 10, 28, 99, 92,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 66, 0, 2, 0), // OP5
        pack_op( 49, 74, 10, 32, 98, 98, 36,  0, 98,  0,  0, 0,0, 0, 7, 0,0, 76, 0, 2, 0), // OP4
        pack_op( 51, 15, 10, 47, 99, 92,  0,  0,  0,  0,  0, 0,0, 0,13, 0,0, 92, 0, 2, 0), // OP3
        pack_op( 81, 13,  7, 25, 99, 92, 28,  0,  0,  0,  0, 0,0, 0, 1, 0,0, 74, 0, 2, 0), // OP2
        pack_op( 48, 56, 10, 47, 98, 98, 36,  0, 98,  0,  0, 0,0, 0, 7, 0,0, 92, 0, 2, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 84, 95, 95, 60, 50, 50, 50, 50,  1, 7, 1, 30, 81,  8,  0,  0,  4,  2, 12),
        *b"STRINGS 2 ",
    ),

    // ===== Voice 6: STRINGS 3 =====
    // Algorithm 15 (idx 14), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 53, 64, 44, 54, 99, 92, 56,  0, 55, 25,  0, 3,0, 2, 7, 0,2, 54, 0,14, 0), // OP6
        pack_op( 53, 67, 38, 54, 86, 92, 74,  0,  0,  0,  0, 0,0, 2, 7, 0,1, 84, 0, 3, 0), // OP5
        pack_op( 96, 19, 20, 54, 99, 92, 89,  0,  0,  0,  0, 0,0, 2, 7, 0,2, 75, 0, 1, 0), // OP4
        pack_op( 50, 52, 35, 41, 99, 92, 91,  0, 51, 98, 60, 3,0, 2, 7, 0,1, 99, 0, 1, 0), // OP3
        pack_op( 99, 71, 35, 51, 82, 92, 87,  0, 54,  0,  0, 0,0, 1, 7, 0,0, 86, 0, 1, 0), // OP2
        pack_op( 52, 30, 25, 43, 99, 92, 90,  0,  0,  0,  0, 0,0, 2, 7, 0,1, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50, 14, 7, 1, 28, 46, 30,  0,  0,  4,  1, 12),
        *b"STRINGS 3 ",
    ),

    // ===== Voice 7: ORCHESTRA =====
    // Algorithm 2 (idx 1), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 72, 76, 10, 32, 99, 92,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 82, 0, 2, 0), // OP6
        pack_op( 76, 73, 10, 55, 99, 92,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 80, 0, 2, 0), // OP5
        pack_op( 56, 74, 10, 45, 98, 98, 36,  0, 98,  0,  0, 0,0, 0, 7, 0,0, 72, 0, 2, 0), // OP4
        pack_op( 54, 15, 10, 47, 99, 92,  0,  0,  0,  0,  0, 0,0, 0,13, 0,0, 96, 0, 2, 0), // OP3
        pack_op( 53, 46, 32, 61, 99, 93, 90,  0,  0,  0,  0, 0,0, 0, 1, 0,0, 83, 0, 1, 0), // OP2
        pack_op( 80, 56, 10, 45, 98, 98, 36,  0, 98,  0,  0, 0,0, 0, 7, 0,0, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 84, 95, 95, 60, 50, 50, 50, 50,  1, 7, 1, 30, 63,  6,  0,  0,  4,  3, 12),
        *b"ORCHESTRA ",
    ),

    // ===== Voice 8: PIANO   1 =====
    // Algorithm 19 (idx 18), Feedback 6, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99,  0, 25,  0, 99, 75,  0,  0,  0,  0, 10, 0,0, 5, 6, 0,0, 82, 0, 1, 0), // OP6
        pack_op( 81, 58, 36, 39, 99, 14,  0,  0, 48,  0, 66, 0,0, 5, 6, 0,1, 93, 0, 1,58), // OP5
        pack_op( 81, 23, 22, 45, 99, 78,  0,  0,  0,  0,  0, 0,0, 5, 8, 0,2, 99, 0, 1, 0), // OP4
        pack_op( 81, 25, 25, 14, 99, 99, 99,  0, 47, 32, 74, 3,0, 5, 7, 0,0, 57, 0, 3, 0), // OP3
        pack_op( 99,  0, 25,  0, 99, 75,  0,  0,  0,  0, 13, 0,0, 5, 9, 0,0, 87, 0, 1, 0), // OP2
        pack_op( 81, 25, 20, 48, 99, 82,  0,  0,  0, 85,  0, 3,0, 4, 5, 0,2, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50, 18, 6, 1, 35,  0,  0,  0,  0,  0,  4, 24),
        *b"PIANO   1 ",
    ),

    // ===== Voice 9: PIANO   2 =====
    // Algorithm 18 (idx 17), Feedback 5, OKS off
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 92, 71, 58, 36, 99,  0,  0,  0, 36,  0, 98, 0,0, 3, 8, 0,1, 78, 0, 0, 0), // OP6
        pack_op( 90, 71, 33, 31, 99,  0,  0,  0, 27,  0, 26, 2,0, 3, 7, 0,1, 94, 0, 0, 0), // OP5
        pack_op( 97, 27, 10, 25, 99, 86, 48,  0,  0,  0,  0, 0,0, 3, 6, 0,1, 84, 0, 1, 0), // OP4
        pack_op( 90, 27, 20, 50, 99, 85,  0,  0, 32,  0, 27, 0,0, 5, 8, 0,1, 83, 0, 5, 0), // OP3
        pack_op( 95,  0, 25,  0, 99, 75,  0,  0,  0,  0, 10, 0,0, 2, 9, 0,1, 86, 0, 1, 0), // OP2
        pack_op( 80, 24, 10, 50, 99, 62,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,2, 94, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50, 17, 5, 0, 30,  0,  0,  0,  0,  0,  4, 12),
        *b"PIANO   2 ",
    ),

    // ===== Voice 10: PIANO   3 =====
    // Algorithm 3 (idx 2), Feedback 4, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 80, 73, 15, 10, 99, 19,  0,  0, 53,  0,  0, 0,3, 3, 9, 0,5, 84, 0, 0, 0), // OP6
        pack_op( 98, 20,  6,  2, 91, 90,  0,  0, 41,  0, 27, 3,0, 2, 5, 0,1, 87, 0, 1, 0), // OP5
        pack_op( 90, 64, 28, 45, 99, 97,  0,  0, 46,  0,  0, 0,0, 3,10, 0,2, 95, 0, 1, 0), // OP4
        pack_op( 94, 80, 19, 12, 83, 67,  0,  0, 43,  9, 20, 3,0, 3, 4, 0,3, 97, 0, 7, 0), // OP3
        pack_op( 98, 36,  6, 32, 91, 90,  0,  0, 50, 22, 50, 3,0, 2,11, 0,0, 85, 0, 1, 0), // OP2
        pack_op( 90, 30, 28, 45, 99, 95,  0,  0, 32,  0,  0, 0,0, 3, 3, 0,3, 86, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global(  0,  0,  0,  0, 50, 50, 50, 50,  2, 4, 1, 45,  0,  0,  0,  0,  0,  4, 24),
        *b"PIANO   3 ",
    ),

    // ===== Voice 11: E.PIANO 1 =====
    // Algorithm 5 (idx 4), Feedback 6, OKS off
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 95, 29, 20, 50, 99, 95,  0,  0, 41,  0, 19, 0,0, 3,14, 0,6, 79, 0, 1, 0), // OP6
        pack_op( 95, 20, 20, 50, 99, 95,  0,  0,  0,  0,  0, 0,0, 3, 0, 0,0, 99, 0, 1, 0), // OP5
        pack_op( 95, 29, 20, 50, 99, 95,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,6, 89, 0, 1, 0), // OP4
        pack_op( 95, 20, 20, 50, 99, 95,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,2, 99, 0, 1, 0), // OP3
        pack_op( 95, 50, 35, 78, 99, 75,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,7, 58, 0,14, 0), // OP2
        pack_op( 96, 25, 25, 67, 99, 75,  0,  0,  0,  0,  0, 0,0, 3,10, 0,2, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50,  4, 6, 0, 34, 33,  0,  0,  0,  4,  3, 24),
        *b"E.PIANO 1 ",
    ),

    // ===== Voice 12: GUITAR  1 =====
    // Algorithm 8 (idx 7), Feedback 7, OKS off
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 57, 99, 75, 99,  0,  0,  0, 39, 53, 20, 0,0, 0, 7, 3,6, 57, 0,12, 0), // OP6
        pack_op( 81, 87, 22, 75, 99, 92,  0,  0,  0,  0, 15, 0,0, 4, 7, 0,7, 99, 0, 3, 0), // OP5
        pack_op( 81, 87, 22, 75, 99, 92,  0,  0,  0,  0, 14, 0,0, 4, 7, 0,4, 89, 0, 3, 0), // OP4
        pack_op( 78, 87, 22, 75, 99, 92,  0,  0, 34,  9,  0, 0,0, 3, 7, 0,7, 99, 0, 1, 0), // OP3
        pack_op( 91, 25, 39, 60, 99, 86,  0,  0,  0,  0, 65, 0,0, 2, 7, 0,7, 93, 0, 3, 0), // OP2
        pack_op( 74, 85, 27, 70, 99, 95,  0,  0,  0,  0,  0, 0,0, 4, 7, 0,5, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 75, 80, 75, 60, 50, 50, 50, 50,  7, 7, 0, 35,  0,  1,  3,  0,  4,  3, 24),
        *b"GUITAR  1 ",
    ),

    // ===== Voice 13: GUITAR  2 =====
    // Algorithm 16 (idx 15), Feedback 7, OKS off
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 44,  1, 71, 99, 96, 75,  0, 60,  0, 46, 3,0, 0, 7, 0,2, 73, 0, 3, 0), // OP6
        pack_op( 99, 99, 12,  0, 99, 99, 76,  0, 60,  0,  0, 3,0, 0, 7, 0,7, 85, 0, 0, 0), // OP5
        pack_op( 92, 99, 15, 71, 99, 96, 75,  0, 60,  0,  0, 3,0, 0, 7, 0,0, 70, 0, 0, 0), // OP4
        pack_op( 99, 99, 99, 71, 99, 99, 99,  0, 39,  0, 40, 3,0, 0, 7, 0,7, 99, 0, 1,50), // OP3
        pack_op( 99, 99, 99, 42, 99, 99, 99, 99, 48,  0,  0, 3,0, 1, 7, 0,7, 87, 0, 1, 0), // OP2
        pack_op( 95, 67, 99, 71, 99, 99, 99,  0,  0, 82,  0, 3,3, 0, 7, 0,2, 86, 0, 3, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 84, 95, 95, 60, 50, 50, 50, 50, 15, 7, 0, 35,  0,  0,  0,  0,  0,  4, 24),
        *b"GUITAR  2 ",
    ),

    // ===== Voice 14: SYN-LEAD 1 =====
    // Algorithm 18 (idx 17), Feedback 7, OKS off
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 70, 60,  0, 99, 99, 97,  0, 32,  0, 21, 0,0, 3, 7, 0,0, 47, 0,17, 0), // OP6
        pack_op( 99, 99, 97,  0, 99, 65, 60,  0, 32,  0,  0, 0,0, 1, 5, 0,0, 43, 0, 3, 0), // OP5
        pack_op( 99, 92, 28, 60, 99, 90,  0,  0, 48,  0, 60, 0,0, 6, 9, 0,0, 71, 0, 2, 0), // OP4
        pack_op( 99, 87,  0,  0, 93, 90,  0,  0, 32,  0, 21, 0,0, 3, 7, 0,0, 82, 0, 1, 0), // OP3
        pack_op( 99, 95,  0,  0, 99, 96, 89,  0, 32,  0,  0, 0,0, 3, 6, 0,0, 71, 0, 1, 0), // OP2
        pack_op( 99,  0, 12, 70, 99, 95, 95,  0, 32,  0,  0, 0,0, 1, 8, 0,0, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global(  0,  0,  0,  0, 50, 50, 50, 50, 17, 7, 0, 37, 42,  0, 99,  0,  4,  4, 36),
        *b"SYN-LEAD 1",
    ),

    // ===== Voice 15: BASS    1 =====
    // Algorithm 16 (idx 15), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 94, 56, 24, 55, 93, 28,  0,  0,  0,  0,  0, 0,0, 1, 7, 0,7, 85, 0, 9, 0), // OP6
        pack_op( 99,  0,  0,  0, 99,  0,  0,  0, 52, 75,  0, 0,0, 7, 7, 0,3, 62, 0, 0, 0), // OP5
        pack_op( 90, 42,  7, 55, 90, 30,  0,  0,  0,  0,  0, 0,0, 5, 7, 0,5, 93, 0, 5, 0), // OP4
        pack_op( 88, 96, 32, 30, 79, 65,  0,  0,  0,  0,  0, 0,0, 6, 7, 0,3, 99, 0, 0, 0), // OP3
        pack_op( 99, 20,  0,  0, 99,  0,  0,  0, 41,  0,  0, 0,0, 7, 7, 0,0, 80, 0, 0, 0), // OP2
        pack_op( 95, 62, 17, 58, 99, 95, 32,  0, 36, 57, 14, 3,0, 7, 7, 0,0, 99, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50, 15, 7, 1, 35,  0,  0,  0,  0,  0,  3, 12),
        *b"BASS    1 ",
    ),

    // ===== Voice 16: BASS    2 =====
    // Algorithm 17 (idx 16), Feedback 7, OKS off
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 25, 50, 24, 55, 96, 97,  0,  0,  0,  0,  0, 0,0, 3, 8, 0,7, 87, 0, 0, 0), // OP6
        pack_op( 99, 51,  0,  0, 99, 74,  0,  0, 34,  0, 32, 0,0, 4, 7, 0,2, 75, 0, 1, 1), // OP5
        pack_op( 80, 39, 28, 53, 93, 57,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,2, 99, 0, 0, 0), // OP4
        pack_op( 73, 25, 32, 30, 97, 78,  0,  0,  0,  0,  0, 0,0, 3,14, 0,3, 68, 0, 1, 0), // OP3
        pack_op( 28, 37, 42, 50, 99,  0,  0,  0, 41,  0, 35, 0,0, 1, 7, 0,2, 80, 0, 0, 3), // OP2
        pack_op( 75, 37, 18, 63, 99, 70,  0,  0, 48,  0, 32, 0,0, 3, 7, 0,2, 99, 0, 0, 1), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50, 16, 7, 0, 31, 33,  0,  0,  0,  4,  2, 12),
        *b"BASS    2 ",
    ),

    // ===== Voice 17: E.ORGAN 1 =====
    // Algorithm 32 (idx 31), Feedback 0, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 54, 22, 90, 99,  0,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 94, 0, 3, 0), // OP6
        pack_op( 99, 80, 22, 90, 99, 99, 99,  0,  0,  0,  0, 0,0, 0, 9, 0,0, 94, 0, 1, 0), // OP5
        pack_op( 99, 80, 22, 90, 99, 99, 99,  0,  0,  0,  0, 0,0, 0,12, 0,0, 94, 0, 0, 0), // OP4
        pack_op( 99, 80, 54, 82, 99, 99, 99,  0,  0,  0,  0, 0,0, 0,11, 0,0, 94, 0, 1,50), // OP3
        pack_op( 99, 20, 22, 90, 99, 99, 97,  0,  0,  0, 10, 0,0, 0, 1, 0,0, 94, 0, 1, 1), // OP2
        pack_op( 99, 80, 22, 90, 99, 99, 99,  0,  0,  0,  0, 0,0, 0, 5, 0,0, 94, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 75, 80, 75, 60, 50, 50, 50, 50, 31, 0, 1, 35,  0,  0,  0,  0,  0,  4, 24),
        *b"E.ORGAN 1 ",
    ),

    // ===== Voice 18: PIPES   1 =====
    // Algorithm 19 (idx 18), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 72, 25, 25, 70, 99, 99, 99,  0, 46, 10,  1, 0,3, 3, 7, 0,2, 76, 0,10, 0), // OP6
        pack_op( 61, 25, 25, 61, 99, 99, 93,  0,  0,  0,  0, 0,0, 3, 7, 0,0, 97, 0, 2, 0), // OP5
        pack_op( 61, 25, 25, 50, 99, 99, 97,  0, 60, 10, 10, 0,0, 3, 7, 0,0, 88, 0, 4, 0), // OP4
        pack_op( 99, 97, 62, 47, 99, 99, 90,  0, 46, 17, 40, 3,0, 5, 7, 0,0, 75, 0, 1, 0), // OP3
        pack_op( 99, 97, 62, 47, 99, 99, 90,  0,  0,  0,  0, 0,0, 4, 7, 0,0, 90, 0, 0, 0), // OP2
        pack_op( 45, 25, 25, 36, 99, 99, 98,  0, 41,  0, 50, 0,0, 5, 7, 0,0, 99, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50, 18, 7, 1, 34, 33,  0,  0,  0,  4,  2, 12),
        *b"PIPES   1 ",
    ),

    // ===== Voice 19: HARPSICH 1 =====
    // Algorithm 5 (idx 4), Feedback 1, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 95, 72, 71, 99, 99, 97, 91, 98, 64,  0, 55, 0,0, 1, 7, 0,0, 87, 0, 6, 0), // OP6
        pack_op( 95, 28, 27, 47, 99, 90,  0,  0, 49,  0,  0, 0,0, 3, 6, 0,3, 83, 0, 4, 0), // OP5
        pack_op( 95, 72, 71, 99, 99, 97, 91, 98, 64,  0, 46, 0,0, 1, 7, 0,0, 99, 0, 3, 0), // OP4
        pack_op( 95, 28, 27, 47, 99, 90,  0,  0, 49,  0,  0, 0,0, 3, 6, 0,2, 85, 0, 1, 0), // OP3
        pack_op( 95, 72, 71, 99, 99, 97, 91, 98, 49,  0,  0, 0,0, 1, 7, 0,0, 99, 0, 0, 0), // OP2
        pack_op( 95, 28, 27, 47, 99, 90,  0,  0, 49,  0,  0, 0,0, 3, 7, 0,2, 89, 0, 4, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global(  0,  0,  0,  0, 50, 50, 50, 50,  4, 1, 1, 35,  0,  0,  0,  0,  0,  2, 24),
        *b"HARPSICH 1",
    ),

    // ===== Voice 20: CLAV    1 =====
    // Algorithm 3 (idx 2), Feedback 5, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 98, 87,  0,  0, 87, 86,  0,  0, 32,  0, 21, 0,0, 3, 7, 0,7, 78, 0, 8, 0), // OP6
        pack_op( 95, 95,  0,  0, 99, 96, 89,  0, 32,  0,  0, 0,0, 3, 5, 0,6, 99, 0, 0, 0), // OP5
        pack_op( 95, 92, 28, 60, 99, 90,  0,  0, 32,  0,  0, 0,0, 3, 7, 0,2, 99, 0, 2, 0), // OP4
        pack_op( 98, 87,  0,  0, 87, 86,  0,  0, 32,  0, 21, 0,0, 3, 7, 0,1, 71, 0, 4,50), // OP3
        pack_op( 95, 95,  0,  0, 99, 96, 89,  0, 32,  0,  0, 0,0, 3, 6, 0,1, 99, 0, 0, 0), // OP2
        pack_op( 95, 92, 28, 60, 99, 90,  0,  0, 32,  0,  0, 0,0, 3, 8, 0,3, 99, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global(  0,  0,  0,  0, 50, 50, 50, 50,  2, 5, 1, 30,  0,  0,  0,  0,  4,  2, 24),
        *b"CLAV    1 ",
    ),

    // ===== Voice 21: VIBE    1 =====
    // Algorithm 23 (idx 22), Feedback 5, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 48, 99, 50, 99, 32,  0,  0, 39, 12, 12, 0,3, 5, 7, 0,7, 57, 0,14, 0), // OP6
        pack_op( 80, 85, 24, 50, 99, 90, 42,  0,  9,  0,  0, 1,1, 3,14, 0,5, 99, 0, 1, 0), // OP5
        pack_op( 80, 85, 24, 50, 99, 90,  0,  0,  9,  0,  0, 1,1, 3, 0, 0,1, 99, 0, 1, 0), // OP4
        pack_op( 80, 85, 43, 50, 99, 74,  0,  0, 39, 12, 12, 0,3, 4, 7, 0,4, 72, 0, 3, 0), // OP3
        pack_op( 80, 85, 24, 50, 99, 90,  0,  0, 39,  4, 12, 0,3, 2, 7, 0,1, 99, 0, 1, 0), // OP2
        pack_op( 99, 28, 99, 50, 99, 25,  0,  0, 39, 12, 12, 0,3, 2, 7, 0,7, 50, 0, 4, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 99, 98, 75, 60, 50, 50, 50, 50, 22, 5, 1, 26,  0,  0,  0,  1,  0,  2, 24),
        *b"VIBE    1 ",
    ),

    // ===== Voice 22: MARIMBA =====
    // Algorithm 7 (idx 6), Feedback 0, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op(  0, 63, 55,  0, 78, 78,  0,  0, 41,  0,  0, 0,0, 0, 7, 0,2, 99, 0, 4,13), // OP6
        pack_op( 99, 75,  0,  8, 82, 48,  0,  0, 54,  0, 46, 0,0, 0, 7, 0,2, 93, 0, 0,50), // OP5
        pack_op( 99, 75,  0, 82, 82, 48,  0,  0, 54,  0, 46, 0,0, 0, 7, 0,2, 85, 0, 5, 0), // OP4
        pack_op( 95, 33, 49, 41, 99, 92,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,1, 99, 0, 0, 0), // OP3
        pack_op( 99, 72,  0,  0, 82, 48,  0,  0, 54,  0, 46, 0,0, 0, 7, 0,2, 96, 0, 3, 0), // OP2
        pack_op( 95, 40, 49, 55, 99, 92,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,0, 95, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50,  6, 0, 1, 35,  0,  0,  0,  1,  0,  3, 24),
        *b"MARIMBA   ",
    ),

    // ===== Voice 23: KOTO =====
    // Algorithm 2 (idx 1), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 82, 53, 37, 48, 99, 81,  0,  0,  0,  0,  5, 0,0, 6, 7, 0,1, 81, 0, 3, 0), // OP6
        pack_op( 91, 37, 29, 29, 99, 90,  0,  0,  0,  0,  5, 0,0, 6, 7, 0,1, 83, 0, 4, 0), // OP5
        pack_op( 90, 28, 17, 39, 99, 76,  0,  0, 10,  0, 17, 0,1, 6, 7, 0,1, 82, 0, 1, 0), // OP4
        pack_op( 94, 64, 30, 33, 99, 92,  0,  0,  0,  0,  0, 0,0, 5, 7, 0,3, 99, 0, 1, 0), // OP3
        pack_op( 99, 68, 28, 48, 99, 83,  0,  0,  0,  0, 10, 0,0, 6, 7, 0,0, 99, 0, 4, 0), // OP2
        pack_op( 94, 62, 58, 34, 99, 92,  0,  0,  0,  0,  0, 0,0, 6, 7, 0,3, 90, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 85, 99, 75,  0, 49, 50, 50, 50,  1, 7, 1, 30, 40, 17, 15,  1,  4,  2, 24),
        *b"KOTO      ",
    ),

    // ===== Voice 24: FLUTE   1 =====
    // Algorithm 16 (idx 15), Feedback 5, OKS off
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 64, 98, 61, 99, 67, 52,  0, 46,  0,  0, 0,3, 0,11, 0,2, 83, 0, 1,53), // OP6
        pack_op( 65, 38,  0, 61, 99,  0,  0,  0, 53,  0, 43, 0,0, 0, 7, 0,0, 56, 0, 2, 0), // OP5
        pack_op( 61, 25, 25, 60, 99, 99, 97,  0, 60, 10, 10, 0,0, 3, 7, 0,0,  0, 0, 2, 0), // OP4
        pack_op( 53, 38, 75, 61, 88, 44, 24,  0, 46,  0,  0, 3,0, 0, 4, 1,0, 76, 0, 1, 0), // OP3
        pack_op( 99, 97, 62, 54, 99, 99, 90,  0,  0,  0,  0, 0,0, 4,11, 0,2, 75, 0, 1, 0), // OP2
        pack_op( 61, 67, 70, 65, 93, 89, 98,  0, 41,  0,  0, 0,0, 0, 5, 0,2, 98, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 94, 67, 95, 60, 50, 50, 50, 50, 15, 5, 0, 30, 23,  8, 13,  0,  0,  1, 24),
        *b"FLUTE   1 ",
    ),

    // ===== Voice 25: ORCH-CHIME =====
    // Algorithm 5 (idx 4), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99,  0,  0,  0, 99, 99, 99,  0, 15,  0,  0, 0,1, 7, 0, 0,0, 75, 0, 1, 0), // OP6
        pack_op( 41, 42, 71, 34, 99, 99, 99,  0, 15,  0,  0, 0,1, 3,14, 0,0, 98, 0, 1, 0), // OP5
        pack_op( 80, 70,  9, 12, 88, 80,  0,  0, 15,  0,  0, 0,1, 3, 7, 0,3, 91, 0, 2,57), // OP4
        pack_op( 80, 49, 17, 30, 99, 95,  0,  0, 15,  0,  0, 0,1, 3, 7, 0,2, 99, 0, 0, 0), // OP3
        pack_op( 99,  0,  0,  0, 99, 99, 99,  0, 15,  0,  0, 0,1, 7,12, 0,0, 87, 0, 0, 0), // OP2
        pack_op( 34, 42, 71, 34, 99, 99, 99,  0, 15,  0,  0, 0,1, 3,12, 0,0, 97, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 99, 99, 99, 99, 50, 50, 50, 50,  4, 7, 1, 30,  0,  5,  0,  0,  0,  3, 24),
        *b"ORCH-CHIME",
    ),

    // ===== Voice 26: TUB BELLS =====
    // Algorithm 5 (idx 4), Feedback 7, OKS off
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 98, 91,  0, 28, 99,  0,  0,  0,  0,  0,  0, 0,0, 2, 0, 0,0, 85, 0, 2, 0), // OP6
        pack_op( 76, 78, 71, 70, 99,  0,  0,  0,  0,  0,  0, 0,0, 2, 7, 0,5, 99, 1, 2,51), // OP5
        pack_op( 98, 12, 71, 28, 99,  0, 32,  0,  0,  0,  0, 0,0, 2, 5, 0,0, 75, 0, 2,75), // OP4
        pack_op( 95, 33, 71, 25, 99,  0, 32,  0,  0,  0,  0, 0,0, 2, 2, 0,0, 99, 0, 1, 0), // OP3
        pack_op( 98, 12, 71, 28, 99,  0, 32,  0,  0,  0,  0, 0,0, 2,10, 0,0, 78, 0, 2,75), // OP2
        pack_op( 95, 33, 71, 25, 99,  0, 32,  0,  0,  0,  0, 0,0, 2, 9, 0,0, 95, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 67, 95, 95, 60, 50, 50, 50, 50,  4, 7, 0, 35,  0,  0,  0,  0,  1,  1, 24),
        *b"TUB BELLS ",
    ),

    // ===== Voice 27: STEEL DRUM =====
    // Algorithm 15 (idx 14), Feedback 5, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 49, 28, 12, 91, 82,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,0, 49, 1, 2,60), // OP6
        pack_op( 99, 40, 38,  0, 91, 82,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,0, 64, 0, 4,33), // OP5
        pack_op( 99, 44, 50, 21, 91, 82,  0,  0,  0,  0,  0, 0,0, 3,14, 0,1, 88, 0, 2, 0), // OP4
        pack_op( 99, 30, 35, 42, 99, 92,  0,  0,  0,  0,  0, 0,0, 3, 7, 0,3, 99, 0, 1, 0), // OP3
        pack_op( 99, 19, 20,  9, 99, 87,  0,  0, 57,  0, 71, 2,0, 2, 7, 0,2, 64, 0, 1,70), // OP2
        pack_op( 99, 40, 33, 38, 99, 92,  0,  0,  0,  0,  0, 0,0, 4, 7, 0,0, 99, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 50, 50, 50, 50, 50, 50, 50, 50, 14, 5, 1, 25,  0, 10, 99,  0,  4,  2, 24),
        *b"STEEL DRUM",
    ),

    // ===== Voice 28: TIMPANI =====
    // Algorithm 16 (idx 15), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 98,  2, 26, 27, 98,  0,  0,  0,  3,  0,  0, 0,2, 3, 7, 0,1, 73, 0, 0,56), // OP6
        pack_op( 99, 50, 26, 19, 99,  0,  0,  0, 80,  0,  0, 3,1, 0, 7, 0,1, 73, 0, 0, 0), // OP5
        pack_op( 99, 31, 17, 30, 99, 75,  0,  0, 80,  0,  0, 3,1, 7, 7, 0,7, 87, 0, 0,75), // OP4
        pack_op( 99, 77, 26, 23, 99, 72,  0,  0,  0,  0,  0, 0,1, 3, 4, 0,0, 85, 0, 0,36), // OP3
        pack_op( 99, 74,  0,  0, 99,  0,  0,  0, 41,  0,  0, 0,1, 1,10, 0,1, 86, 0, 0, 0), // OP2
        pack_op( 99, 36, 98, 33, 99,  0,  0,  0,  0,  0,  0, 0,3, 3, 7, 0,1, 99, 0, 0, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 99, 98, 75, 60, 50, 51, 50, 50, 15, 7, 1, 11,  0, 16,  0,  0,  0,  2, 24),
        *b"TIMPANI   ",
    ),

    // ===== Voice 29: REFS WHISL =====
    // Algorithm 18 (idx 17), Feedback 2, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 94, 56, 24, 55, 96, 78,  0,  0,  0,  0,  0, 0,0, 1, 7, 0,0, 78, 1, 5, 0), // OP6
        pack_op( 99,  0,  0,  0, 99,  0,  0,  0, 41,  0,  0, 0,0, 0, 7, 0,0, 64, 1, 4, 0), // OP5
        pack_op( 94, 68, 24, 55, 96, 89,  0,  0,  0,  0,  0, 0,0, 1, 7, 0,0, 75, 1, 7,82), // OP4
        pack_op( 60, 39,  8,  0, 99, 99, 99,  0,  0,  0,  0, 0,0, 4, 7, 0,0, 66, 1, 1,67), // OP3
        pack_op( 60, 39, 28, 45, 99, 99, 99,  0,  0,  0,  0, 0,0, 4, 7, 0,0, 93, 1, 9,53), // OP2
        pack_op( 60, 39, 28, 49, 99, 99, 99,  0,  0,  0,  0, 0,0, 4, 7, 0,1, 90, 1, 3,32), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 38, 67, 95, 60, 39, 50, 50, 50, 17, 2, 1, 99,  0,  0,  0,  1,  5,  6, 24),
        *b"REFS WHISL",
    ),

    // ===== Voice 30: VOICE   1 =====
    // Algorithm 7 (idx 6), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 99, 72, 48, 17, 99, 99, 99,  0,  0,  0,  0, 0,0, 0, 8, 0,0, 55, 0, 5, 2), // OP6
        pack_op( 35, 21, 36, 63, 99, 90, 85,  0,  0,  0,  0, 0,0, 0, 6, 0,1, 53, 0, 1, 1), // OP5
        pack_op( 72, 19, 41, 12, 48, 58, 20,  9,  0,  0,  0, 0,0, 0,10, 0,1, 99, 0, 1, 2), // OP4
        pack_op( 33, 20, 53, 39, 99, 94, 97,  0,  0,  0,  0, 0,0, 0,14, 0,3, 99, 0, 1, 0), // OP3
        pack_op( 19, 26, 53, 25, 51, 61, 76, 51,  0,  0,  0, 0,0, 0, 7, 2,2, 99, 0, 1, 0), // OP2
        pack_op( 34, 20, 53, 57, 99, 94, 97,  0,  0,  0,  0, 0,0, 0, 0, 0,0, 87, 0, 1, 0), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 18, 60, 95, 60, 48, 51, 50, 50,  6, 7, 1, 35, 35, 11,  2,  0,  0,  4, 24),
        *b"VOICE   1 ",
    ),

    // ===== Voice 31: TRAIN =====
    // Algorithm 5 (idx 4), Feedback 7, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 49, 17, 25, 53, 99, 99, 99, 98, 36,  0,  0, 3,0, 0, 7, 0,0, 99, 0, 5, 0), // OP6
        pack_op( 42, 17, 25, 53, 99, 99, 99, 99, 36,  0,  0, 3,0, 0,10, 3,0, 83, 0, 9, 0), // OP5
        pack_op( 98, 29, 28, 27, 99,  0,  0,  0, 20,  0,  0, 1,1, 0, 5, 0,0, 89, 1,10,99), // OP4
        pack_op( 98, 29, 28, 33, 99,  0,  0,  0, 99, 98,  0, 1,1, 0, 9, 0,0, 99, 1,22,57), // OP3
        pack_op( 39, 13, 12, 72, 99, 61, 66,  0, 52,  0,  0, 3,0, 5, 7, 0,0, 72, 0, 3, 1), // OP2
        pack_op( 65, 24, 19, 57, 99, 85, 85,  0, 39,  0, 98, 3,0, 3, 7, 0,0, 99, 0, 1,64), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 75, 67, 95, 60, 50, 50, 50, 50,  4, 7, 1, 39,  0,  0, 99,  0,  0,  0, 24),
        *b"TRAIN     ",
    ),

    // ===== Voice 32: TAKE OFF =====
    // Algorithm 10 (idx 9), Feedback 0, OKS on
    pack_voice(
        //        R1  R2  R3  R4  L1  L2  L3  L4  BP  LD  RD LC RC RS DT AMS KVS OL  M FC FF
        pack_op( 89, 22, 20, 41, 99, 92,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 99, 0, 0, 0), // OP6
        pack_op( 88, 24, 23, 37, 99, 90,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 96, 0, 2, 1), // OP5
        pack_op( 13, 14, 20, 30, 99, 95, 99,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 99, 0, 0, 0), // OP4
        pack_op( 76, 35, 99, 11, 67, 38, 73,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 99, 0, 6, 1), // OP3
        pack_op( 82, 80, 19, 14, 80, 95,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 96, 0, 1, 0), // OP2
        pack_op(  9, 14, 17, 34, 61, 96,  0,  0,  0,  0,  0, 0,0, 0, 7, 0,0, 99, 0, 4, 1), // OP1
        //        PR1 PR2 PR3 PR4 PL1 PL2 PL3 PL4 ALG FB OKS SPD DLY PMD AMD SYN WAV PMS TRP
        pack_global( 32, 30, 94, 16, 50,  7, 81, 99,  9, 0, 1, 65,  0,  0,  0,  1,  2,  5,  0),
        *b"TAKE OFF  ",
    ),

];

/// Name list for ROM1A voices.
pub const ROM1A_VOICE_NAMES: [&str; 32] = [
    "BRASS   1",
    "BRASS   2",
    "BRASS   3",
    "STRINGS 1",
    "STRINGS 2",
    "STRINGS 3",
    "ORCHESTRA",
    "PIANO   1",
    "PIANO   2",
    "PIANO   3",
    "E.PIANO 1",
    "GUITAR  1",
    "GUITAR  2",
    "SYN-LEAD 1",
    "BASS    1",
    "BASS    2",
    "E.ORGAN 1",
    "PIPES   1",
    "HARPSICH 1",
    "CLAV    1",
    "VIBE    1",
    "MARIMBA",
    "KOTO",
    "FLUTE   1",
    "ORCH-CHIME",
    "TUB BELLS",
    "STEEL DRUM",
    "TIMPANI",
    "REFS WHISL",
    "VOICE   1",
    "TRAIN",
    "TAKE OFF",
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
    fn test_brass_1() {
        let v = load_rom1a_voice(0).unwrap();
        assert_eq!(v.name_str().trim(), "BRASS   1");
        assert_eq!(v.algorithm, 21);
        assert_eq!(v.feedback, 7);
        assert!(v.osc_key_sync);
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
    fn test_roundtrip_matches_sysex() {
        let sysex_data = include_bytes!("../../sysex/factory/rom1a.syx");
        let voice_data = &sysex_data[6..6 + 4096];
        assert_eq!(&ROM1A_VOICE_DATA[..], voice_data,
            "ROM1A_VOICE_DATA does not match sysex file");
    }

    #[test]
    fn test_single_voice_bounds() {
        assert!(load_rom1a_voice(0).is_some());
        assert!(load_rom1a_voice(31).is_some());
        assert!(load_rom1a_voice(32).is_none());
    }
}
