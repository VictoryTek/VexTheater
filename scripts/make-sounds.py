#!/usr/bin/env python3
"""Writes the launcher's click sounds to data/sounds/. Run from the repository root.

Both are synthesised here, so they are original and carry no licence: a short tick for
moving between tiles and a fuller two-tone click for choosing one.
"""
import math
import struct
import wave
from pathlib import Path

RATE = 44100


def write(name, length, partials, decay):
    """`partials` is a list of (frequency in Hz, level); `decay` is the fade's time
    constant in seconds."""
    frames = []
    for n in range(int(RATE * length)):
        t = n / RATE
        attack = min(1.0, t / 0.002)  # 2 ms ramp-in, so the start does not pop
        fade = math.exp(-t / decay)
        # Linear taper over the last 5 ms, so the end does not pop either.
        taper = min(1.0, (length - t) / 0.005)
        tone = sum(level * math.sin(2 * math.pi * freq * t) for freq, level in partials)
        frames.append(int(32767 * attack * fade * taper * tone))
    path = Path("data/sounds") / name
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(RATE)
        out.writeframes(struct.pack(f"<{len(frames)}h", *frames))


write("move.wav", 0.045, [(2000, 0.22), (4000, 0.06)], 0.007)
write("select.wav", 0.12, [(880, 0.30), (1320, 0.20)], 0.03)
