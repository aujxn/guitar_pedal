use crate::constants::*;
use crate::interface::EffectMessage;
use crate::Sample;
use ringbuf::{Consumer, Producer};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

/// Wraps callback for jack::ClosureProcessHandler (fn process_block),
/// applies effects to audio stream, forwards stream to LoopManager
/// with some clock info, and listens for events from Interface to
/// toggle effects.
pub struct PlaybackManager {
    /// RingBuffer of mixed loop data to be played.
    loops: Consumer<f32>,
    /// How many samples have elapsed since start of current measure.
    sample_counter: usize,
    /// Only supports 4/4 for now.
    samples_per_measure: usize,
    /// RingBuffer of processed audio being sent to LoopManager.
    stream: Producer<Sample>,
    /// Channel to listen for effect control messages. Because this
    /// struct gets moved into the callback closure for the jack::async_client
    /// all of it's members must be sync. I guess because a receiver can be
    /// cloned that it isn't Sync? I fought rustc for a little and ended up
    /// just wrapping it with Arc<Mutex<_>>. I don't think this is necessary
    /// and I could probably just use unsafe but I'm sure there is a better
    /// solution than this. Hopefully the mutual exclusion primitives don't
    /// slow things down too much.
    effects_message_receiver: Arc<Mutex<Receiver<EffectMessage>>>,
    /// Compression on/off?
    compress: bool,
    /// Distortion on/off?
    distort: bool,
}

impl PlaybackManager {
    pub fn new(
        loops: Consumer<f32>,
        stream: Producer<Sample>,
        samples_per_measure: usize,
        effects_message_receiver: Receiver<EffectMessage>,
    ) -> Self {
        Self {
            loops,
            stream,
            samples_per_measure,
            sample_counter: 0,
            effects_message_receiver: Arc::new(Mutex::new(effects_message_receiver)),
            compress: false,
            distort: false,
        }
    }

    /// Takes a buffer frame, processes it, sends that processed output to
    /// the LoopManager, read's mixed loop audio from LoopManager's RingBuffer
    /// and mixes it with the incoming signal and writes that to the output buffer.
    pub fn process_block(&mut self, input: &[f32], output: &mut [f32]) {
        self.check_messages();
        if self.compress {
            // Calculate the compression scale
            let scale = Self::compressor(input);
            // Scale the input into output
            input
                .iter()
                .zip(output.iter_mut())
                .for_each(|(x, y)| *y = x * scale);
        } else {
            // Otherwise copy input exactly
            output.copy_from_slice(input);
        }

        if self.distort {
            // Calculate a waveshaped wet signal
            let wet = Self::distortion(output);
            // Mix ratio, TODO: allow interface to send messages to change
            let mix = 0.10;
            // Mix wet and dry
            for (x, wet) in output.iter_mut().zip(wet.iter()) {
                *x = *x * (1.0 - mix) + wet * mix;
            }
        }

        // Send audio to LoopManager
        self.send_stream(output);
        // Mix loop audio into output buffer
        self.play_loops(output);
    }

    /// Checks if the Interface is asking to toggle any effects.
    fn check_messages(&mut self) {
        if let Ok(message) = self.effects_message_receiver.lock().unwrap().try_recv() {
            match message {
                EffectMessage::ToggleDistortion => {
                    self.distort = !self.distort;
                    if self.distort {
                        println!("Distortion ON");
                    } else {
                        println!("Distortion OFF");
                    }
                }
                EffectMessage::ToggleCompression => {
                    self.compress = !self.compress;
                    if self.compress {
                        println!("Compressor ON");
                    } else {
                        println!("Compressor OFF");
                    }
                }
            }
        }
    }

    /// Sends processed audio samples to LoopManager along with some clock information.
    fn send_stream(&mut self, stream: &[f32]) {
        for sample in stream.iter() {
            let remaining_samples_in_measure = self.samples_per_measure - self.sample_counter;
            // Send a measure clock tick
            if remaining_samples_in_measure == 0 {
                self.stream
                    .push(Sample::Tick)
                    .expect("stream ring buffer full");
                self.sample_counter = 0;
            // This means this is the last buffer in the current measure.
            // Warns the LoopManager that its going to need more loop
            // samples very soon.
            } else if remaining_samples_in_measure == BUFFER_SIZE {
                self.stream
                    .push(Sample::PreTick)
                    .expect("stream ring buffer full");
            }
            self.stream
                .push(Sample::Data(*sample))
                .expect("stream ring buffer full");
            self.sample_counter += 1;
        }
    }

    /// Mix the loop audio into the output buffer.
    fn play_loops(&mut self, output: &mut [f32]) {
        for sample in output.iter_mut() {
            *sample += self.loops.pop().expect("loop buffer empty")
        }
    }

    /// Basically a rust rewrite of Bart's compressor.
    /// Might do a better algorithm that has a attack and release adjustment. TODO
    /// Because when the compressor shuts off below the threshold it
    /// sounds pretty bad (pop/click noise). Also, I am using my Jack's buffer
    /// frame size (usually 512 samples) which is pretty short for calculating peak.
    /// I could save older samples with a RingBuffer if I wanted a larger period.
    fn compressor(buffer: &[f32]) -> f32 {
        // for calculating peak amplitude.
        let threshold = -30.0;
        // compression ratio. TODO: allow interface messages to adjust this and threshold
        let ratio = 4.0;
        let peak = Self::peak(buffer);
        if peak >= threshold {
            // Not an efficient calculation but it seems like it's fast enough.
            10.0_f32.powf(
                ((peak - threshold) * (1.0 / ratio - 1.0) + threshold * (1.0 / ratio - 1.0)) / 20.0,
            )
        } else {
            1.0
        }
    }

    /// Get the peak amplitude of the buffered audio in dB.
    fn peak(buffer: &[f32]) -> f32 {
        let (max, min) = buffer
            .iter()
            .fold((buffer[0], buffer[0]), |(max, min), &x| {
                if x > max {
                    (x, min)
                } else if x < min {
                    (max, x)
                } else {
                    (max, min)
                }
            });

        // log == bad but still fast enough.
        20.0 * (max - min).log10()
    }

    /// Table based waveshaper distortion. There is some weird aliasing
    /// here that I could maybe remove with oversampling? Honestly,
    /// this distortion sounds pretty terrible but I tried using
    /// a few different waveshapers and polynomials like Chebyshev
    /// but nothing was good. The current table is based off arctangent.
    /// I think I read 6 research papers on digital simulation of
    /// analog distortion, fuzz, and overdrive circuits and I am
    /// pretty sure I know less now than when I started.
    fn distortion(buffer: &[f32]) -> [f32; BUFFER_SIZE] {
        // Hopefully this gets optimized out? It really should.
        // I tried to make is a const array but I guess iter
        // isn't a const function.
        let table: Vec<f32> = (0..1000)
            .map(|x| (x as f32 * 3.0 / 1000.0).atan() * 0.8)
            .collect();

        // Closure to do the waveshaping
        let waveshape = |x: f32| {
            let x = (x * 1000.0).trunc() as i32;
            if x >= 0 && x < 1000 {
                table[x as usize]
            } else if x < 0 && x > -1000 {
                -1.0 * table[(-x) as usize]
            } else if x > 1000 {
                table[999]
            } else {
                -1.0 * table[999]
            }
        };

        // Make the wet signal
        let mut output = [0.0; BUFFER_SIZE];
        for (x, y) in buffer.iter().zip(output.iter_mut()) {
            *y = waveshape(*x);
        }

        output
    }
}
