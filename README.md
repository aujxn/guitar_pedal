## Austen Nelson --- guitar_pedal

# Purpose
Does some live audio processing of guitar input. Features:

- [x] Compressor
- [x] Distortion
- [x] Looping
- [x] Metronome

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

# Dependencies
JACK Audio Connection Kit

# TODO

- [ ] Maybe improve compressor algorithm?
- [ ] Better distortion algorithm?
- [ ] Create TUI
- [ ] Make metronome use different output port?

# Tests
There aren't any formal tests but running the application and using it seems to run
with the current configuration. When I broke things it crashes fairly quickly as
this application is fairly simple. I am not sure how I would simulate usage to write
automated tests for this application.

# Report
I originally planned to do Wah as well but realized I need a pedal with some sort of
modulation and creating that hardware wasn't reasonable right now. Making the looping
mechanism turned out to be much more difficult than I expected and the solution I came
up with doesn't seem incredibly robust. I learned a lot about audio and JACK, though,
and am excited to do more audio related projects.

Overall, I am satisfied that it works as well as it does. Working with live signals is
different from any other development I have experienced. If I were to do it again I am
not sure what I would change structurally, but I am sure it could be much better.

I am very unsatisfied with the distortion effect I came up with, it sounds like garbage.
I tried a few different approaches and they were all terrible with weird aliasing. I read
a few papers about modeling effects of analog circuits but they were all beyond my
comprehension. I could try to minimize the aliasing with oversampling techniques but this
wont improve the otherwise low quality of the effect and might not even be reasonable in
a live setting. The rest of the code has some TODO comments where I had thoughts of making
changes or improvements but they are quite specific.

# License
MIT License

Copyright (c) 2020 Austen Jay Nelson

See license file.
