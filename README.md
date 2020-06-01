## Austen Nelson --- guitar_pedal

# Usage
```
cargo run --release [--bpm]
```
This program creates JACK clients with ports that must be connected using
a JACK server management tool. It expects midi input to control the looping
and effects. With the current configuration of constants in src/lib.rs:

C2 through B3 are loop "registers" pressing one of these keys will
start recording a loop at that register is it is empty, start playing a
loop if it is stopped, and stop playing a loop if it is active. This
all happens on the start of the next measure. Loop index 0, or C2, is
the metronome. To stop recording the loop a midi event for pressing the
sustain pedal is expected. A loop continues recording until the end of
the current measure. Loops are active immediately after they are recorded
and will play on the next measure.

# Settings
These can be altered by changing constants in lib.rs but
default settings are:

To toggle distortion: midi note 95 (B6)

To toggle compressor: midi note 96 (C7)

Number of loops: 24, midi notes 36 (C2) to 59 (B3)

# Purpose
Does some live audio processing of guitar input. Features:

- [x] Compressor
- [x] Distortion
- [x] Looping
- [x] Metronome

# Dependencies
JACK Audio Connection Kit

# TODO

- [ ] Maybe improve compressor algorithm?
- [ ] Create TUI
- [ ] Make metronome use different output port?

# License
MIT License

Copyright (c) 2020 Austen Jay Nelson

See license file.
