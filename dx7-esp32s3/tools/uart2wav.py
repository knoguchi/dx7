#!/usr/bin/env python3
"""Convert UART hex dump from dx7-esp32s3 QEMU to WAV file.

Usage:
  # Capture QEMU output and convert:
  timeout 300 qemu-system-xtensa ... 2>&1 | tee /tmp/uart.log
  python3 tools/uart2wav.py /tmp/uart.log /tmp/dx7.wav

  # Or pipe directly:
  timeout 300 qemu-system-xtensa ... 2>&1 | python3 tools/uart2wav.py - /tmp/dx7.wav
"""
import struct
import sys
import wave

def parse_uart(lines):
    """Parse hex audio data between ---AUDIO--- and ---END--- markers."""
    sample_rate = 44100
    channels = 2
    bits = 16
    in_audio = False
    raw_bytes = bytearray()

    for line in lines:
        line = line.strip()
        if line.startswith("---AUDIO"):
            parts = line.strip("-").split()
            # AUDIO 44100 16 2 44096
            sample_rate = int(parts[1])
            bits = int(parts[2])
            channels = int(parts[3])
            in_audio = True
            continue
        if line == "---END---":
            break
        if in_audio and len(line) > 0:
            # Each line is hex-encoded bytes: N*8 hex chars = N*4 bytes
            try:
                raw_bytes.extend(bytes.fromhex(line))
            except ValueError:
                pass  # skip non-hex lines

    return raw_bytes, sample_rate, channels, bits

def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <uart_log> <output.wav>")
        sys.exit(1)

    input_path = sys.argv[1]
    output_path = sys.argv[2]

    if input_path == "-":
        lines = sys.stdin.readlines()
    else:
        with open(input_path) as f:
            lines = f.readlines()

    raw_bytes, sample_rate, channels, bits = parse_uart(lines)

    if len(raw_bytes) == 0:
        print("No audio data found. Check that the log contains ---AUDIO--- and ---END--- markers.")
        sys.exit(1)

    n_samples = len(raw_bytes) // (channels * (bits // 8))
    duration = n_samples / sample_rate
    print(f"Decoded {n_samples} samples ({duration:.2f}s) @ {sample_rate}Hz {bits}-bit {channels}ch")

    with wave.open(output_path, "w") as w:
        w.setnchannels(channels)
        w.setsampwidth(bits // 8)
        w.setframerate(sample_rate)
        w.writeframes(raw_bytes)

    print(f"Wrote {output_path}")

if __name__ == "__main__":
    main()
